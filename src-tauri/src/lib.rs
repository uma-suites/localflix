
use futures_util::StreamExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::Command,
};
use tauri::{AppHandle, Emitter, Manager};

const OWNER: &str = "uma-suites";
const REPO: &str = "localflix";
const ASSET: &str = "localflix.zip";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    url: String,
    checksum_url: String,
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    percent: u64,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("hello-updater")
        .build()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_update(
    app: AppHandle,
) -> Result<Option<UpdateInfo>, String> {
    let url = format!(
        "https://api.github.com/repos/{OWNER}/{REPO}/releases/latest"
    );

    let response = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let release: Release = response
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let latest = release.tag_name.trim_start_matches('v');
    let latest_version =
        Version::parse(latest).map_err(|e| e.to_string())?;

    let current_version = Version::parse(
        &app.package_info().version.to_string()
    )
    .map_err(|e| e.to_string())?;

    if latest_version <= current_version {
        return Ok(None);
    }

    let find_asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    };

    let url = find_asset(ASSET)
        .ok_or("Release ZIP not found")?;

    let checksum_url = find_asset(
        "hello-updater.zip.sha256"
    )
    .ok_or("Checksum file not found")?;

    Ok(Some(UpdateInfo {
        version: latest.to_string(),
        url,
        checksum_url,
    }))
}

fn update_dir() -> PathBuf {
    std::env::temp_dir().join("hello-updater-download")
}

#[tauri::command]
async fn download_update(
    app: AppHandle,
    url: String,
    checksum_url: String,
) -> Result<(), String> {
    // Only accept assets from our configured repository.
    let expected_prefix = format!(
        "https://github.com/{OWNER}/{REPO}/releases/download/"
    );

    if !url.starts_with(&expected_prefix)
        || !url.ends_with("/hello-updater.zip")
        || checksum_url
            != format!("{url}.sha256")
    {
        return Err("Invalid update URL".into());
    }

    let http = client()?;

    let checksum_response = http
        .get(&checksum_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let checksum_text = checksum_response
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let expected_hash = checksum_text
        .split_whitespace()
        .next()
        .ok_or("Missing checksum")?
        .to_lowercase();

    if expected_hash.len() != 64
        || !expected_hash
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Err("Invalid SHA-256 checksum".into());
    }

    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let total = response.content_length();
    let mut stream = response.bytes_stream();

    let dir = update_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())?;

    let zip_path = dir.join("update.zip");
    let mut file = fs::File::create(&zip_path)
        .map_err(|e| e.to_string())?;

    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;

        file.write_all(&chunk)
            .map_err(|e| e.to_string())?;

        hasher.update(&chunk);
        downloaded += chunk.len() as u64;

        if let Some(total) = total {
            if total > 0 {
                let percent =
                    (downloaded * 100 / total).min(100);

                let _ = app.emit(
                    "update-progress",
                    DownloadProgress { percent },
                );
            }
        }
    }

    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let actual_hash = hex::encode(hasher.finalize());

    if actual_hash != expected_hash {
        let _ = fs::remove_file(&zip_path);
        return Err("Checksum verification failed".into());
    }

    app.emit(
        "update-progress",
        DownloadProgress { percent: 100 },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn restart_and_update(app: AppHandle) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| e.to_string())?;

    let zip_path = update_dir().join("update.zip");

    if !zip_path.exists() {
        return Err("No downloaded update found".into());
    }

    let script = r#"
param(
    [string]$Target,
    [string]$Zip,
    [int]$ParentPid
)

$ErrorActionPreference = "Stop"
$work = Join-Path $env:TEMP ("hello-update-" + [guid]::NewGuid())

try {
    New-Item -ItemType Directory -Path $work | Out-Null
    Expand-Archive -LiteralPath $Zip -DestinationPath $work

    $source = Join-Path $work "hello-updater.exe"

    if (!(Test-Path -LiteralPath $source)) {
        throw "Replacement executable missing"
    }

    # Wait for the old application to exit.
    $deadline = (Get-Date).AddSeconds(30)
    while (Get-Process -Id $ParentPid -ErrorAction SilentlyContinue) {
        if ((Get-Date) -gt $deadline) {
            throw "Application did not exit"
        }
        Start-Sleep -Milliseconds 250
    }

    # Replace the executable.
    $backup = "$Target.old"
    if (Test-Path -LiteralPath $backup) {
        Remove-Item -LiteralPath $backup -Force
    }

    Move-Item -LiteralPath $Target -Destination $backup
    try {
        Copy-Item -LiteralPath $source -Destination $Target
    } catch {
        Move-Item -LiteralPath $backup -Destination $Target
        throw
    }

    Start-Process -FilePath $Target
} catch {
    Add-Content -Path (Join-Path $env:TEMP "hello-updater-error.log") -Value $_
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
"#;

    let script_path = update_dir().join("apply-update.ps1");

    fs::write(&script_path, script)
        .map_err(|e| e.to_string())?;

    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .arg("-Target")
        .arg(&current_exe)
        .arg("-Zip")
        .arg(&zip_path)
        .arg("-ParentPid")
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|e| e.to_string())?;

    app.exit(0);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            check_update,
            download_update,
            restart_and_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
