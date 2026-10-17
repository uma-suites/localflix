import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type UpdateInfo = {
  version: string;
  url: string;
  checksum_url: string;
};

type Stage =
  | "checking"
  | "idle"
  | "available"
  | "downloading"
  | "ready"
  | "error";

function App() {
  const [stage, setStage] = useState<Stage>("checking");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState("");

  async function checkForUpdates() {
    setStage("checking");
    setError("");

    try {
      const result = await invoke<UpdateInfo | null>("check_update");

      setUpdate(result);
      setStage(result ? "available" : "idle");
    } catch (err) {
      setError(String(err));
      setStage("error");
    }
  }

  useEffect(() => {
    const unlistenPromise = listen<{ percent: number }>(
      "update-progress",
      (event) => {
        setProgress(event.payload.percent);
      },
    );

    // Check on startup.
    void checkForUpdates();

    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  async function startDownload() {
    if (!update) return;

    setProgress(0);
    setStage("downloading");

    try {
      await invoke("download_update", {
        url: update.url,
        checksumUrl: update.checksum_url,
      });

      setStage("ready");
    } catch (err) {
      setError(String(err));
      setStage("error");
    }
  }

  async function restartNow() {
    try {
      await invoke("restart_and_update");
    } catch (err) {
      setError(String(err));
      setStage("error");
    }
  }

  return (
    <main className="app">
      <h1>Hello World 2</h1>
      <p>Version 0.2.0</p>

      <button onClick={checkForUpdates}>Check for updates</button>

      {stage === "checking" && <p>Checking for updates...</p>}

      {stage === "idle" && <p>You're up to date.</p>}

      {stage === "error" && (
        <div className="error">
          <p>Update error: {error}</p>
          <button onClick={checkForUpdates}>Try again</button>
        </div>
      )}

      {(stage === "available" ||
        stage === "downloading" ||
        stage === "ready") && (
        <div className="overlay">
          <div className="dialog">
            {stage === "available" && (
              <>
                <h2>Update Available</h2>
                <p>
                  Version {update?.version} is available. Would you like to
                  update?
                </p>

                <div className="actions">
                  <button onClick={() => setStage("idle")}>Not now</button>
                  <button onClick={startDownload}>Update now</button>
                </div>
              </>
            )}

            {stage === "downloading" && (
              <>
                <h2>Downloading Update</h2>
                <progress value={progress} max={100} />
                <p>{progress}%</p>
              </>
            )}

            {stage === "ready" && (
              <>
                <h2>Update Ready</h2>
                <p>
                  Version {update?.version} is downloaded. Restart to apply the
                  update.
                </p>

                <div className="actions">
                  <button onClick={() => setStage("idle")}>
                    Restart later
                  </button>
                  <button onClick={restartNow}>Restart now</button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </main>
  );
}

export default App;
