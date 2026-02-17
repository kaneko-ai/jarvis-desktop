import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

function escapeHtml(raw) {
  return String(raw ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMarkdownToHtml(md) {
  const lines = String(md ?? "").split(/\r?\n/);
  const html = [];
  let inCodeBlock = false;
  let inList = false;

  const closeList = () => {
    if (inList) {
      html.push("</ul>");
      inList = false;
    }
  };

  for (const line of lines) {
    if (line.startsWith("```")) {
      closeList();
      if (!inCodeBlock) {
        inCodeBlock = true;
        html.push("<pre><code>");
      } else {
        inCodeBlock = false;
        html.push("</code></pre>");
      }
      continue;
    }

    if (inCodeBlock) {
      html.push(escapeHtml(line) + "\n");
      continue;
    }

    if (/^\s*[-*]\s+/.test(line)) {
      if (!inList) {
        html.push("<ul>");
        inList = true;
      }
      html.push(`<li>${escapeHtml(line.replace(/^\s*[-*]\s+/, ""))}</li>`);
      continue;
    }

    closeList();

    if (/^###\s+/.test(line)) {
      html.push(`<h3>${escapeHtml(line.replace(/^###\s+/, ""))}</h3>`);
      continue;
    }
    if (/^##\s+/.test(line)) {
      html.push(`<h2>${escapeHtml(line.replace(/^##\s+/, ""))}</h2>`);
      continue;
    }
    if (/^#\s+/.test(line)) {
      html.push(`<h1>${escapeHtml(line.replace(/^#\s+/, ""))}</h1>`);
      continue;
    }

    if (line.trim() === "") {
      html.push("<br />");
      continue;
    }

    html.push(`<p>${escapeHtml(line)}</p>`);
  }

  closeList();
  if (inCodeBlock) {
    html.push("</code></pre>");
  }

  return html.join("\n");
}

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
  const [runs, setRuns] = useState([]);
  const [runsLoading, setRunsLoading] = useState(false);
  const [runsError, setRunsError] = useState("");
  const [selectedRunId, setSelectedRunId] = useState("");
  const [selectedArtifact, setSelectedArtifact] = useState("tree_md");
  const [artifactLoading, setArtifactLoading] = useState(false);
  const [artifactError, setArtifactError] = useState("");
  const [artifactView, setArtifactView] = useState(null);

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

  async function loadRuns() {
    setRunsLoading(true);
    setRunsError("");
    try {
      const rows = await invoke("list_runs");
      const list = Array.isArray(rows) ? rows : [];
      setRuns(list);
      setSelectedRunId((prev) => {
        if (prev && list.some((r) => r.run_id === prev)) return prev;
        return list[0]?.run_id ?? "";
      });
    } catch (e) {
      setRuns([]);
      setRunsError(String(e));
    } finally {
      setRunsLoading(false);
    }
  }

  async function loadArtifact(runId, artifactKey) {
    if (!runId) {
      setArtifactView(null);
      return;
    }
    setArtifactLoading(true);
    setArtifactError("");
    try {
      const res = await invoke("read_run_artifact", {
        runId,
        artifact: artifactKey,
      });
      setArtifactView(res);
    } catch (e) {
      setArtifactView(null);
      setArtifactError(String(e));
    } finally {
      setArtifactLoading(false);
    }
  }

  useEffect(() => {
    loadRuntimeConfig(false);
    loadRuns();
  }, []);

  useEffect(() => {
    loadArtifact(selectedRunId, selectedArtifact);
  }, [selectedRunId, selectedArtifact]);

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
      await loadRuns();
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

  async function onOpenSelectedRunFolder() {
    const row = runs.find((r) => r.run_id === selectedRunId);
    if (!row?.run_dir) return;
    try {
      await invoke("open_run_folder", { runDir: row.run_dir });
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

  async function onCreateConfigTemplate() {
    try {
      await invoke("create_config_if_missing");
      await loadRuntimeConfig(true);
      await loadRuns();
    } catch (e) {
      alert(String(e));
    }
  }

  const showRetryButton = status === "needs_retry" && !!lastRunRequest;
  const selectedRun = runs.find((r) => r.run_id === selectedRunId) ?? null;
  const artifactIsMissing = artifactView && artifactView.exists === false;
  const createdAtText = selectedRun?.created_at_epoch_ms
    ? new Date(selectedRun.created_at_epoch_ms).toLocaleString()
    : "-";

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
        <div style={{ fontSize: 12, marginTop: 4 }}>
          Config validation:{" "}
          <strong style={{ color: runtimeCfg?.ok ? "#1f6f3f" : "#a33" }}>
            {runtimeCfg?.ok ? "ok" : "error"}
          </strong>
          {runtimeCfg?.status ? <span style={{ marginLeft: 8 }}>status={runtimeCfg.status}</span> : null}
        </div>
        <div style={{ marginTop: 8, display: "flex", gap: 8 }}>
          <button
            onClick={onOpenConfigLocation}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Open config file location
          </button>
          <button
            onClick={onCreateConfigTemplate}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Create config template
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

      <hr style={{ margin: "18px 0" }} />

      <h3 style={{ marginBottom: 8 }}>Runs / Artifacts Viewer</h3>
      <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
        <button
          onClick={loadRuns}
          disabled={runsLoading}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          {runsLoading ? "Refreshing..." : "Refresh runs"}
        </button>
        <button
          onClick={onOpenSelectedRunFolder}
          disabled={!selectedRun}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          Open selected run folder
        </button>
      </div>

      {cfgError ? (
        <div style={{ marginBottom: 8, color: "#a33", fontSize: 12 }}>
          out_dir is invalid or unresolved. Fix config and click Reload config.
        </div>
      ) : null}

      {runsError ? (
        <div style={{ marginBottom: 8, color: "#a33", fontSize: 12 }}>{runsError}</div>
      ) : null}

      <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12 }}>
        <div style={{ border: "1px solid #ddd", borderRadius: 8, maxHeight: 260, overflow: "auto" }}>
          {runs.length === 0 ? (
            <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>No runs found.</div>
          ) : (
            runs.map((row) => (
              <button
                key={row.run_id}
                onClick={() => setSelectedRunId(row.run_id)}
                style={{
                  width: "100%",
                  textAlign: "left",
                  padding: 10,
                  border: "none",
                  borderBottom: "1px solid #eee",
                  background: row.run_id === selectedRunId ? "#eef5ff" : "white",
                  cursor: "pointer",
                }}
              >
                <div style={{ fontSize: 12, fontWeight: 600 }}>{row.run_id}</div>
                <div style={{ fontSize: 11, opacity: 0.85 }}>status={row.status}</div>
                <div style={{ fontSize: 11, opacity: 0.75 }}>paper_id={row.paper_id}</div>
              </button>
            ))
          )}
        </div>

        <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
          <div style={{ fontSize: 12, marginBottom: 6 }}>run_id: <code>{selectedRun?.run_id ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 6 }}>status: <code>{selectedRun?.status ?? "unknown"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 6 }}>created_at: <code>{createdAtText}</code></div>
          <div style={{ fontSize: 12, marginBottom: 10 }}>path: <code>{selectedRun?.run_dir ?? "-"}</code></div>

          <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
            <select
              value={selectedArtifact}
              onChange={(e) => setSelectedArtifact(e.target.value)}
              style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
            >
              <option value="tree_md">tree.md (markdown)</option>
              <option value="result_json">result.json</option>
              <option value="input_json">input.json</option>
              <option value="stdout_log">stdout.log</option>
              <option value="stderr_log">stderr.log</option>
            </select>
          </div>

          {artifactLoading ? <div style={{ fontSize: 12 }}>Loading artifact...</div> : null}
          {artifactError ? <div style={{ color: "#a33", fontSize: 12 }}>{artifactError}</div> : null}
          {artifactIsMissing ? (
            <div style={{ color: "#a33", fontSize: 12, marginBottom: 6 }}>
              missing: {artifactView.path}
            </div>
          ) : null}

          {artifactView ? (
            <div>
              <div style={{ fontSize: 11, opacity: 0.8, marginBottom: 6 }}>
                artifact_path=<code>{artifactView.path}</code> parse_status=<code>{artifactView.parse_status}</code>
              </div>
              {selectedArtifact === "tree_md" && artifactView.exists ? (
                <div
                  style={{
                    border: "1px solid #eee",
                    borderRadius: 6,
                    padding: 10,
                    maxHeight: 360,
                    overflow: "auto",
                    background: "#fff",
                    lineHeight: 1.4,
                  }}
                  dangerouslySetInnerHTML={{ __html: renderMarkdownToHtml(artifactView.content) }}
                />
              ) : (
                <textarea
                  readOnly
                  value={artifactView.content ?? ""}
                  style={{
                    width: "100%",
                    height: 280,
                    padding: 10,
                    borderRadius: 8,
                    border: "1px solid #ccc",
                    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                    fontSize: 12,
                  }}
                />
              )}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
