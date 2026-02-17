import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export default function App() {
  const [paperId, setPaperId] = useState("arxiv:1706.03762");
  const [depth, setDepth] = useState(2);
  const [maxPerLevel, setMaxPerLevel] = useState(50);

  const [running, setRunning] = useState(false);
  const [stdout, setStdout] = useState("");
  const [stderr, setStderr] = useState("");
  const [exitCode, setExitCode] = useState(null);
  const [runId, setRunId] = useState(null);
  const [runDir, setRunDir] = useState(null);
  const [status, setStatus] = useState(null);
  const [message, setMessage] = useState("");
  const [retryAfterSec, setRetryAfterSec] = useState(null);
  const [lastRunRequest, setLastRunRequest] = useState(null);

  const [runtimeCfg, setRuntimeCfg] = useState(null);
  const [cfgLoading, setCfgLoading] = useState(false);
  const [cfgError, setCfgError] = useState("");

  const combined = useMemo(() => {
    const parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push("\n[stderr]\n" + stderr);
    return parts.join("\n");
  }, [stdout, stderr]);

  async function loadRuntimeConfig(useReload = false) {
    setCfgLoading(true);
    setCfgError("");
    try {
      const res = await invoke(useReload ? "reload_runtime_config" : "get_runtime_config");
      setRuntimeCfg(res);
      if (!res?.ok) {
        setCfgError(res?.message || "Failed to resolve runtime config");
      }
    } catch (e) {
      setCfgError(String(e));
      setRuntimeCfg(null);
    } finally {
      setCfgLoading(false);
    }
  }

  useEffect(() => {
    loadRuntimeConfig(false);
  }, []);

  async function runTree(params) {
    setRunning(true);
    setStdout("");
    setStderr("");
    setExitCode(null);
    setRunId(null);
    setRunDir(null);
    setStatus(null);
    setMessage("");
    setRetryAfterSec(null);

    try {
      const res = await invoke("run_papers_tree", {
        paperId: params.paperId,
        depth: params.depth,
        maxPerLevel: params.maxPerLevel,
      });

      setStdout(res.stdout ?? "");
      setStderr(res.stderr ?? "");
      setExitCode(res.exit_code ?? null);
      setRunId(res.run_id ?? null);
      setRunDir(res.run_dir ?? null);
      setStatus(res.status ?? null);
      setMessage(res.message ?? "");
      setRetryAfterSec(res.retry_after_sec ?? null);
      setLastRunRequest(params);
    } catch (e) {
      setStderr(String(e));
      setStatus("error");
      setMessage(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function onRunTree() {
    await runTree({ paperId, depth, maxPerLevel });
  }

  async function onRetry() {
    if (!lastRunRequest || running) return;
    await runTree(lastRunRequest);
  }

  async function onOpenRunFolder() {
    if (!runDir) return;
    try {
      await invoke("open_run_folder", { runDir });
    } catch (e) {
      alert(String(e));
    }
  }

  async function onOpenConfigLocation() {
    try {
      await invoke("open_config_file_location");
    } catch (e) {
      alert(String(e));
    }
  }

  const showRetryButton = status === "needs_retry" && !!lastRunRequest;

  return (
    <div style={{ fontFamily: "system-ui", padding: 16, maxWidth: 960 }}>
      <h2 style={{ marginTop: 0 }}>Javis Desktop</h2>

      <div
        style={{
          border: "1px solid #d2d2d2",
          borderRadius: 10,
          padding: 12,
          marginBottom: 14,
          background: "#fafafa",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Runtime config</div>
        <div style={{ fontSize: 12, opacity: 0.9 }}>
          config: <code>{runtimeCfg?.config_file_path ?? "-"}</code>
        </div>
        <div style={{ fontSize: 12, opacity: 0.9 }}>
          pipeline_root: <code>{runtimeCfg?.pipeline_root ?? "-"}</code>
        </div>
        <div style={{ fontSize: 12, opacity: 0.9 }}>
          out_dir: <code>{runtimeCfg?.out_dir ?? "-"}</code>
        </div>
        <div style={{ marginTop: 8, display: "flex", gap: 8 }}>
          <button
            onClick={onOpenConfigLocation}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Open config file location
          </button>
          <button
            onClick={() => loadRuntimeConfig(true)}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            {cfgLoading ? "Reloading..." : "Reload config"}
          </button>
        </div>
        {cfgError ? (
          <div style={{ marginTop: 8, color: "#a33", fontSize: 12 }}>
            config status=missing_dependency: {cfgError}
          </div>
        ) : null}
      </div>

      <div style={{ display: "grid", gap: 12, gridTemplateColumns: "1fr 160px 160px" }}>
        <label style={{ display: "grid", gap: 6 }}>
          <span>Paper ID (doi:/pmid:/arxiv:/s2:)</span>
          <input
            value={paperId}
            onChange={(e) => setPaperId(e.target.value)}
            style={{ padding: 10, borderRadius: 8, border: "1px solid #ccc" }}
          />
        </label>

        <label style={{ display: "grid", gap: 6 }}>
          <span>Depth</span>
          <select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            style={{ padding: 10, borderRadius: 8, border: "1px solid #ccc" }}
          >
            <option value={1}>1</option>
            <option value={2}>2</option>
          </select>
        </label>

        <label style={{ display: "grid", gap: 6 }}>
          <span>Max/level</span>
          <input
            type="number"
            min={1}
            max={200}
            value={maxPerLevel}
            onChange={(e) => setMaxPerLevel(Number(e.target.value))}
            style={{ padding: 10, borderRadius: 8, border: "1px solid #ccc" }}
          />
        </label>
      </div>

      <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
        <button
          onClick={onRunTree}
          disabled={running || !paperId}
          style={{ padding: "10px 14px", borderRadius: 10, border: "1px solid #333" }}
        >
          {running ? "Running..." : "Run papers tree"}
        </button>

        <button
          onClick={onOpenRunFolder}
          disabled={!runDir}
          style={{ padding: "10px 14px", borderRadius: 10, border: "1px solid #333" }}
        >
          Open run folder
        </button>

        {showRetryButton ? (
          <button
            onClick={onRetry}
            disabled={running}
            style={{ padding: "10px 14px", borderRadius: 10, border: "1px solid #333" }}
          >
            Retry (new run_id)
          </button>
        ) : null}

        <div style={{ alignSelf: "center", opacity: 0.8 }}>
          {exitCode !== null ? <span>exit={exitCode}</span> : <span> </span>}
          {runId ? <span style={{ marginLeft: 10 }}>run_id={runId}</span> : null}
          {status ? <span style={{ marginLeft: 10 }}>status={status}</span> : null}
        </div>
      </div>

      {runDir ? (
        <div style={{ marginTop: 8, opacity: 0.85, fontSize: 12 }}>
          run_dir=<code>{runDir}</code>
        </div>
      ) : null}

      {message ? (
        <div style={{ marginTop: 8, color: status === "ok" ? "#245" : "#a33", fontSize: 13 }}>
          {message}
        </div>
      ) : null}

      {status === "needs_retry" ? (
        <div style={{ marginTop: 6, color: "#8a4200", fontSize: 13 }}>
          rate-limited; retry after{" "}
          {typeof retryAfterSec === "number" ? `${retryAfterSec.toFixed(1)} sec` : "a short interval"}.
        </div>
      ) : null}

      <div style={{ marginTop: 14 }}>
        <textarea
          readOnly
          value={combined}
          placeholder="stdout/stderr will appear here..."
          style={{
            width: "100%",
            height: 360,
            padding: 12,
            borderRadius: 10,
            border: "1px solid #ccc",
            fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
            fontSize: 12,
            whiteSpace: "pre",
          }}
        />
      </div>

      <div style={{ marginTop: 10, opacity: 0.8, fontSize: 12 }}>
        Retry button re-runs the same request with a new <code>run_id</code>.
      </div>
    </div>
  );
}
