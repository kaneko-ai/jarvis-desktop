import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

function escapeHtml(raw) {
  return String(raw ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
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

function toCanonicalLibraryQuery(node) {
  if (!node) return "";
  const candidates = [];
  const raw = node.raw ?? {};
  const push = (v) => {
    const text = String(v ?? "").trim();
    if (text) candidates.push(text);
  };

  push(node.id);
  push(raw.canonical_id);
  push(raw.paper_id);
  push(raw.id);
  push(raw.doi);
  push(raw.pmid);
  push(raw.arxiv);
  push(raw.s2);

  const ext = raw.externalIds;
  if (ext && typeof ext === "object") {
    push(ext.DOI);
    push(ext.PubMed);
    push(ext.ArXiv);
  }

  for (const c of candidates) {
    const lower = c.toLowerCase();
    if (
      lower.startsWith("doi:")
      || lower.startsWith("pmid:")
      || lower.startsWith("arxiv:")
      || lower.startsWith("s2:")
    ) {
      return c;
    }
    if (/^10\.[^\s]+/.test(c)) return `doi:${c}`;
    if (/^\d{6,12}$/.test(c)) return `pmid:${c}`;
    if (/^\d{4}\.\d{4,5}(v\d+)?$/i.test(c)) return `arxiv:${c}`;
  }

  return candidates[0] ?? "";
}

function buildAnalyzePipelineSteps() {
  return [
    {
      template_id: "TEMPLATE_TREE",
      params: { depth: 2, max_per_level: 50 },
    },
    {
      template_id: "TEMPLATE_RELATED",
      params: { depth: 1, max_per_level: 30 },
    },
    {
      template_id: "TEMPLATE_GRAPH",
      params: { k: 40, seed: 42 },
    },
    {
      template_id: "TEMPLATE_MAP",
      params: { k: 24, seed: 42 },
    },
  ];
}

function normalizePathname(pathname) {
  const raw = String(pathname ?? "").trim();
  if (!raw || raw === "/") return "/";
  return raw.endsWith("/") ? raw.slice(0, -1) : raw;
}

function screenFromPathname(pathname) {
  const normalized = normalizePathname(pathname);
  if (normalized === "/runs") return "runs";
  return "setup";
}

function pathFromScreen(screen) {
  if (screen === "runs") return "/runs";
  return "/";
}

const PIPELINE_RUN_ARTIFACT_OPTIONS = [
  { kind: "input", label: "input.json" },
  { kind: "result", label: "result.json" },
  { kind: "tree", label: "paper_graph/tree/tree.md" },
  { kind: "report", label: "report.md" },
  { kind: "warnings", label: "warnings.jsonl" },
  { kind: "evidence", label: "evidence.jsonl" },
  { kind: "claims", label: "claims.jsonl" },
  { kind: "eval_summary", label: "eval_summary.json" },
  { kind: "scores", label: "scores.json" },
  { kind: "papers", label: "papers.jsonl" },
  { kind: "run_config", label: "run_config.json" },
];

function pipelineRunStatusColor(status) {
  const key = String(status ?? "").toLowerCase();
  if (key === "success") return "#1f6f3f";
  if (key === "needs_retry") return "#8a4200";
  if (key === "failed") return "#a33";
  if (key === "missing_result") return "#555";
  return "#666";
}

export default function App() {
  const [paperId, setPaperId] = useState("arxiv:1706.03762");
  const [templates, setTemplates] = useState([]);
  const [templatesLoading, setTemplatesLoading] = useState(false);
  const [templatesError, setTemplatesError] = useState("");
  const [selectedTemplateId, setSelectedTemplateId] = useState("TEMPLATE_TREE");
  const [templateParams, setTemplateParams] = useState({});

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
  const [pipelineRootDraft, setPipelineRootDraft] = useState("");
  const [outDirDraft, setOutDirDraft] = useState("");
  const [normalized, setNormalized] = useState(null);
  const [normalizeLoading, setNormalizeLoading] = useState(false);
  const [preflight, setPreflight] = useState(null);
  const [preflightLoading, setPreflightLoading] = useState(false);
  const [preflightError, setPreflightError] = useState("");
  const [runs, setRuns] = useState([]);
  const [runsLoading, setRunsLoading] = useState(false);
  const [runsError, setRunsError] = useState("");
  const [selectedRunId, setSelectedRunId] = useState("");
  const [selectedArtifact, setSelectedArtifact] = useState("tree_md");
  const [artifactLoading, setArtifactLoading] = useState(false);
  const [artifactError, setArtifactError] = useState("");
  const [artifactView, setArtifactView] = useState(null);
  const [artifactWarnings, setArtifactWarnings] = useState([]);
  const [artifactWrap, setArtifactWrap] = useState(true);
  const [graphParsed, setGraphParsed] = useState(null);
  const [graphParseLoading, setGraphParseLoading] = useState(false);
  const [graphParseError, setGraphParseError] = useState("");
  const [graphQuery, setGraphQuery] = useState("");
  const [graphTypeFilter, setGraphTypeFilter] = useState("all");
  const [graphYearFrom, setGraphYearFrom] = useState("");
  const [graphYearTo, setGraphYearTo] = useState("");
  const [graphHasEdgesOnly, setGraphHasEdgesOnly] = useState(false);
  const [selectedGraphNodeId, setSelectedGraphNodeId] = useState("");
  const [runArtifactCatalog, setRunArtifactCatalog] = useState([]);
  const [runArtifactCatalogLoading, setRunArtifactCatalogLoading] = useState(false);
  const [runArtifactCatalogError, setRunArtifactCatalogError] = useState("");
  const [artifactCatalogByRun, setArtifactCatalogByRun] = useState({});
  const [jobs, setJobs] = useState([]);
  const [jobsLoading, setJobsLoading] = useState(false);
  const [jobsError, setJobsError] = useState("");
  const [selectedJobId, setSelectedJobId] = useState("");
  const [pipelines, setPipelines] = useState([]);
  const [pipelinesLoading, setPipelinesLoading] = useState(false);
  const [pipelinesError, setPipelinesError] = useState("");
  const [selectedPipelineId, setSelectedPipelineId] = useState("");
  const [pipelineDetail, setPipelineDetail] = useState(null);
  const [pipelineDetailLoading, setPipelineDetailLoading] = useState(false);
  const [activeScreen, setActiveScreen] = useState(() => {
    if (typeof window === "undefined") return "setup";
    return screenFromPathname(window.location.pathname);
  });
  const [pipelineRepoBusy, setPipelineRepoBusy] = useState(false);
  const [pipelineRepoError, setPipelineRepoError] = useState("");
  const [pipelineRepoStatus, setPipelineRepoStatus] = useState(null);
  const [pipelineRepoValidate, setPipelineRepoValidate] = useState(null);
  const [pipelineRepoRemoteUrl, setPipelineRepoRemoteUrl] = useState("");
  const [pipelineRepoLocalPath, setPipelineRepoLocalPath] = useState("");
  const [pipelineRepoRef, setPipelineRepoRef] = useState("main");
  const [bootstrapLogBusy, setBootstrapLogBusy] = useState(false);
  const [bootstrapLogLines, setBootstrapLogLines] = useState([]);
  const [opsNeedsAttentionOnly, setOpsNeedsAttentionOnly] = useState(false);
  const [opsAutoRetryPendingOnly, setOpsAutoRetryPendingOnly] = useState(false);
  const [desktopSettings, setDesktopSettings] = useState(null);
  const [settingsLoading, setSettingsLoading] = useState(false);
  const [settingsError, setSettingsError] = useState("");
  const [tickResult, setTickResult] = useState(null);
  const [diagnosticsRows, setDiagnosticsRows] = useState([]);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [diagnosticsError, setDiagnosticsError] = useState("");
  const [collectingDiagnostics, setCollectingDiagnostics] = useState(false);
  const [diagnosticsOneClickBusy, setDiagnosticsOneClickBusy] = useState(false);
  const [diagnosticsOneClickMessage, setDiagnosticsOneClickMessage] = useState("");
  const [latestDiagnosticId, setLatestDiagnosticId] = useState("");
  const [latestDiagnosticZipPath, setLatestDiagnosticZipPath] = useState("");
  const [latestDiagnosticFolderPath, setLatestDiagnosticFolderPath] = useState("");
  const [selectedDiagId, setSelectedDiagId] = useState("");
  const [diagReport, setDiagReport] = useState("");
  const [diagReportLoading, setDiagReportLoading] = useState(false);
  const [diagReportError, setDiagReportError] = useState("");
  const [workspaceExports, setWorkspaceExports] = useState([]);
  const [workspaceImports, setWorkspaceImports] = useState([]);
  const [workspaceLoading, setWorkspaceLoading] = useState(false);
  const [workspaceError, setWorkspaceError] = useState("");
  const [workspaceExporting, setWorkspaceExporting] = useState(false);
  const [workspaceImporting, setWorkspaceImporting] = useState(false);
  const [workspaceImportZipPath, setWorkspaceImportZipPath] = useState("");
  const [workspaceImportMode, setWorkspaceImportMode] = useState("keep_current");
  const [workspaceImportDryRun, setWorkspaceImportDryRun] = useState(false);
  const [snapshotLoading, setSnapshotLoading] = useState(false);
  const [workspaceFixingRuntime, setWorkspaceFixingRuntime] = useState(false);
  const [workspaceFixRuntimeMessage, setWorkspaceFixRuntimeMessage] = useState("");
  const [workspaceLastImportId, setWorkspaceLastImportId] = useState("");
  const [selectedWorkspaceReport, setSelectedWorkspaceReport] = useState({ scope: "", id: "" });
  const [workspaceReport, setWorkspaceReport] = useState("");
  const [workspaceReportLoading, setWorkspaceReportLoading] = useState(false);
  const [workspaceReportError, setWorkspaceReportError] = useState("");
  const autoRetryTickBusyRef = useRef(false);
  const [libraryRows, setLibraryRows] = useState([]);
  const [libraryLoading, setLibraryLoading] = useState(false);
  const [libraryError, setLibraryError] = useState("");
  const [libraryStats, setLibraryStats] = useState(null);
  const [selectedPaperKey, setSelectedPaperKey] = useState("");
  const [libraryDetail, setLibraryDetail] = useState(null);
  const [libraryFilters, setLibraryFilters] = useState({
    status: "",
    kind: "",
    tag: "",
  });
  const [tagInput, setTagInput] = useState("");
  const [libraryReindexInfo, setLibraryReindexInfo] = useState(null);
  const [librarySearchQuery, setLibrarySearchQuery] = useState("");
  const [librarySearchRows, setLibrarySearchRows] = useState([]);
  const [librarySearchLoading, setLibrarySearchLoading] = useState(false);
  const [pipelineRuns, setPipelineRuns] = useState([]);
  const [pipelineRunsLoading, setPipelineRunsLoading] = useState(false);
  const [pipelineRunsError, setPipelineRunsError] = useState("");
  const [selectedPipelineRunId, setSelectedPipelineRunId] = useState("");
  const [pipelineRunTab, setPipelineRunTab] = useState("input");
  const [pipelineRunQuery, setPipelineRunQuery] = useState("");
  const [pipelineRunText, setPipelineRunText] = useState("");
  const [pipelineRunTextLoading, setPipelineRunTextLoading] = useState(false);
  const [pipelineRunTextError, setPipelineRunTextError] = useState("");

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

  async function loadTemplates() {
    setTemplatesLoading(true);
    setTemplatesError("");
    try {
      const res = await invoke("list_task_templates");
      const list = Array.isArray(res) ? res : [];
      setTemplates(list);
      setSelectedTemplateId((prev) => {
        if (prev && list.some((t) => t.id === prev)) return prev;
        return list[0]?.id ?? "";
      });
    } catch (e) {
      setTemplates([]);
      setTemplatesError(String(e));
    } finally {
      setTemplatesLoading(false);
    }
  }

  async function loadPreflight() {
    setPreflightLoading(true);
    setPreflightError("");
    try {
      const res = await invoke("preflight_check");
      setPreflight(res);
    } catch (e) {
      setPreflight(null);
      setPreflightError(String(e));
    } finally {
      setPreflightLoading(false);
    }
  }

  async function loadRuns() {
    setRunsLoading(true);
    setRunsError("");
    try {
      const rows = await invoke("list_runs", {
        limit: 500,
        filters: null,
      });
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

  async function loadPipelineRuns() {
    setPipelineRunsLoading(true);
    setPipelineRunsError("");
    try {
      const rows = await invoke("list_pipeline_runs", { limit: 200 });
      const list = Array.isArray(rows) ? rows : [];
      setPipelineRuns(list);
      setSelectedPipelineRunId((prev) => {
        if (prev && list.some((r) => r.run_id === prev)) return prev;
        return list[0]?.run_id ?? "";
      });
    } catch (e) {
      setPipelineRuns([]);
      setPipelineRunsError(String(e));
      setSelectedPipelineRunId("");
    } finally {
      setPipelineRunsLoading(false);
    }
  }

  async function loadPipelineRunText(runId, kind) {
    if (!runId) {
      setPipelineRunText("");
      setPipelineRunTextError("");
      setPipelineRunTextLoading(false);
      return;
    }
    setPipelineRunTextLoading(true);
    setPipelineRunTextError("");
    try {
      const text = await invoke("read_run_text", { runId, kind });
      setPipelineRunText(String(text ?? ""));
    } catch (e) {
      const msg = String(e ?? "");
      setPipelineRunText("");
      if (msg.toLowerCase().includes("does not exist") || msg.toLowerCase().includes("not found")) {
        setPipelineRunTextError("存在しない");
      } else {
        setPipelineRunTextError(msg);
      }
    } finally {
      setPipelineRunTextLoading(false);
    }
  }

  async function onOpenPipelineRunDir(runId) {
    if (!runId) return;
    try {
      await invoke("open_run_dir", { runId });
    } catch (e) {
      alert(String(e));
    }
  }

  async function loadJobs() {
    setJobsLoading(true);
    setJobsError("");
    try {
      const rows = await invoke("list_jobs");
      const list = Array.isArray(rows) ? rows : [];
      setJobs(list);
      setSelectedJobId((prev) => {
        if (prev && list.some((j) => j.job_id === prev)) return prev;
        return list[0]?.job_id ?? "";
      });
    } catch (e) {
      setJobsError(String(e));
      setJobs([]);
    } finally {
      setJobsLoading(false);
    }
  }

  async function loadPipelines() {
    setPipelinesLoading(true);
    setPipelinesError("");
    try {
      const rows = await invoke("list_pipelines", {});
      const list = Array.isArray(rows) ? rows : [];
      setPipelines(list);
      setSelectedPipelineId((prev) => {
        if (prev && list.some((p) => p.pipeline_id === prev)) return prev;
        return list[0]?.pipeline_id ?? "";
      });
    } catch (e) {
      setPipelines([]);
      setPipelinesError(String(e));
    } finally {
      setPipelinesLoading(false);
    }
  }

  async function loadSettings() {
    setSettingsLoading(true);
    setSettingsError("");
    try {
      const settings = await invoke("get_settings");
      setDesktopSettings(settings ?? null);
    } catch (e) {
      setDesktopSettings(null);
      setSettingsError(String(e));
    } finally {
      setSettingsLoading(false);
    }
  }

  async function updateAutoRetryEnabled(enabled) {
    if (!desktopSettings) return;
    setSettingsError("");
    try {
      const updated = await invoke("update_settings", {
        settings: {
          ...desktopSettings,
          auto_retry_enabled: !!enabled,
        },
      });
      setDesktopSettings(updated ?? null);
    } catch (e) {
      setSettingsError(String(e));
    }
  }

  async function loadPipelineRepoStatus() {
    try {
      const status = await invoke("get_pipeline_repo_status");
      setPipelineRepoStatus(status ?? null);
      setPipelineRepoRemoteUrl(String(status?.remote_url ?? ""));
      setPipelineRepoLocalPath(String(status?.local_path ?? ""));
      setPipelineRepoRef(String(status?.git_ref ?? "main"));
    } catch (e) {
      setPipelineRepoStatus(null);
      setPipelineRepoError(String(e));
    }
  }

  async function onSavePipelineRepoSettings() {
    setPipelineRepoBusy(true);
    setPipelineRepoError("");
    try {
      const settings = await invoke("update_pipeline_repo_settings", {
        update: {
          remote_url: pipelineRepoRemoteUrl,
          local_path: pipelineRepoLocalPath,
          git_ref: pipelineRepoRef,
        },
      });
      const repo = settings?.pipeline_repo ?? {};
      setPipelineRepoRemoteUrl(String(repo.remote_url ?? pipelineRepoRemoteUrl));
      setPipelineRepoLocalPath(String(repo.local_path ?? pipelineRepoLocalPath));
      setPipelineRepoRef(String(repo.git_ref ?? pipelineRepoRef));
      await loadPipelineRepoStatus();
      await Promise.all([loadRuntimeConfig(true), loadPreflight(), loadSettings()]);
    } catch (e) {
      setPipelineRepoError(String(e));
    } finally {
      setPipelineRepoBusy(false);
    }
  }

  async function onBootstrapPipelineRepo() {
    setPipelineRepoBusy(true);
    setPipelineRepoError("");
    try {
      const status = await invoke("bootstrap_pipeline_repo");
      setPipelineRepoStatus(status ?? null);
      await Promise.all([loadRuntimeConfig(true), loadPreflight(), loadSettings(), loadPipelineRepoStatus()]);
    } catch (e) {
      setPipelineRepoError(String(e));
      await loadPipelineRepoStatus();
    } finally {
      setPipelineRepoBusy(false);
    }
  }

  function onClearBootstrapLogs() {
    setBootstrapLogLines([]);
  }

  async function onBootstrapPipelineRepoWithLogs() {
    setBootstrapLogBusy(true);
    setPipelineRepoBusy(true);
    setPipelineRepoError("");
    setBootstrapLogLines([]);

    let unlistenLog = null;
    let unlistenDone = null;
    try {
      unlistenLog = await listen("bootstrap_pipeline_repo:log", (event) => {
        const line = String(event?.payload ?? "");
        if (!line) return;
        setBootstrapLogLines((prev) => [...prev, line]);
      });

      unlistenDone = await listen("bootstrap_pipeline_repo:done", (event) => {
        const payload = event?.payload ?? {};
        const ok = payload?.ok === true;
        const message = String(payload?.message ?? "").trim();
        const suffix = message ? `: ${message}` : "";
        setBootstrapLogLines((prev) => [...prev, `[done] ${ok ? "ok" : "error"}${suffix}`]);
      });

      const status = await invoke("bootstrap_pipeline_repo_stream");
      setPipelineRepoStatus(status ?? null);
      await Promise.all([loadRuntimeConfig(true), loadPreflight(), loadSettings(), loadPipelineRepoStatus()]);
    } catch (e) {
      const msg = String(e);
      setPipelineRepoError(msg);
      setBootstrapLogLines((prev) => [...prev, `[error] ${msg}`]);
      await loadPipelineRepoStatus();
    } finally {
      if (typeof unlistenLog === "function") {
        unlistenLog();
      }
      if (typeof unlistenDone === "function") {
        unlistenDone();
      }
      setBootstrapLogBusy(false);
      setPipelineRepoBusy(false);
    }
  }

  async function onUpdatePipelineRepo() {
    setPipelineRepoBusy(true);
    setPipelineRepoError("");
    try {
      const status = await invoke("update_pipeline_repo");
      setPipelineRepoStatus(status ?? null);
      await Promise.all([loadRuntimeConfig(true), loadPreflight(), loadSettings(), loadPipelineRepoStatus()]);
    } catch (e) {
      setPipelineRepoError(String(e));
      await loadPipelineRepoStatus();
    } finally {
      setPipelineRepoBusy(false);
    }
  }

  async function onValidatePipelineRepo() {
    setPipelineRepoBusy(true);
    setPipelineRepoError("");
    try {
      const result = await invoke("validate_pipeline_repo");
      setPipelineRepoValidate(result ?? null);
    } catch (e) {
      setPipelineRepoValidate(null);
      setPipelineRepoError(String(e));
    } finally {
      setPipelineRepoBusy(false);
    }
  }

  async function onOpenPipelineRepoFolder() {
    try {
      await invoke("open_pipeline_repo_folder");
    } catch (e) {
      setPipelineRepoError(String(e));
    }
  }

  async function onOpenAuditLog() {
    try {
      await invoke("open_audit_log");
    } catch (e) {
      alert(String(e));
    }
  }

  async function tickAutoRetry() {
    if (autoRetryTickBusyRef.current) return;
    autoRetryTickBusyRef.current = true;
    try {
      const res = await invoke("tick_auto_retry");
      setTickResult(res ?? null);
      if (res?.acted) {
        await Promise.all([loadPipelines(), loadJobs(), loadRuns()]);
      }
    } catch (e) {
      setTickResult({ acted: false, reason: String(e) });
    } finally {
      autoRetryTickBusyRef.current = false;
    }
  }

  async function loadDiagnostics() {
    setDiagnosticsLoading(true);
    setDiagnosticsError("");
    try {
      const rows = await invoke("list_diagnostics");
      const list = Array.isArray(rows) ? rows : [];
      setDiagnosticsRows(list);
      setSelectedDiagId((prev) => {
        if (prev && list.some((r) => r.diag_id === prev)) return prev;
        return list[0]?.diag_id ?? "";
      });
    } catch (e) {
      setDiagnosticsRows([]);
      setDiagnosticsError(String(e));
    } finally {
      setDiagnosticsLoading(false);
    }
  }

  async function onCollectDiagnostics() {
    setCollectingDiagnostics(true);
    setDiagnosticsError("");
    try {
      const res = await invoke("collect_diagnostics", {
        opts: {
          include_audit: true,
          include_recent_runs: true,
          include_zip: true,
        },
      });
      await loadDiagnostics();
      if (res?.diag_id) {
        setSelectedDiagId(res.diag_id);
      }
    } catch (e) {
      setDiagnosticsError(String(e));
    } finally {
      setCollectingDiagnostics(false);
    }
  }

  async function onGenerateDiagnosticsZipOneClick() {
    setDiagnosticsOneClickBusy(true);
    setDiagnosticsError("");
    setDiagnosticsOneClickMessage("");
    try {
      const collected = await invoke("collect_diagnostics", {
        opts: {
          include_audit: true,
          include_recent_runs: true,
          include_zip: true,
        },
      });

      let diagId = String(collected?.diag_id ?? "").trim();
      let folderPath = String(collected?.diag_dir ?? "").trim();
      let zipPath = String(collected?.zip_path ?? "").trim();

      if (!diagId) {
        const rows = await invoke("list_diagnostics");
        const list = Array.isArray(rows) ? rows : [];
        diagId = String(list[0]?.diag_id ?? "").trim();
        if (!zipPath) {
          zipPath = String(list[0]?.zip_path ?? "").trim();
        }
      }

      if (!diagId) {
        throw new Error("diagnostics bundle ID not found after collect_diagnostics");
      }

      const openedZipPath = await invoke("open_diagnostic_zip", { diagId });
      if (String(openedZipPath ?? "").trim()) {
        zipPath = String(openedZipPath).trim();
      }

      const rows = await invoke("list_diagnostics");
      const list = Array.isArray(rows) ? rows : [];
      setDiagnosticsRows(list);
      setSelectedDiagId(diagId);
      const matched = list.find((item) => item?.diag_id === diagId) ?? null;
      if (!zipPath) {
        zipPath = String(matched?.zip_path ?? "").trim();
      }

      setLatestDiagnosticId(diagId);
      setLatestDiagnosticZipPath(zipPath);
      setLatestDiagnosticFolderPath(folderPath);
      setDiagnosticsOneClickMessage(
        zipPath
          ? `Generated diagnostics zip: ${diagId} (${zipPath})`
          : `Generated diagnostics bundle: ${diagId}`
      );
    } catch (e) {
      setDiagnosticsError(String(e));
      setDiagnosticsOneClickMessage("");
    } finally {
      setDiagnosticsOneClickBusy(false);
    }
  }

  async function onOpenLatestDiagnosticFolder() {
    if (!latestDiagnosticId) return;
    setDiagnosticsError("");
    try {
      const opened = await invoke("open_diagnostic_folder", { diagId: latestDiagnosticId });
      const path = String(opened ?? "").trim();
      if (path) {
        setLatestDiagnosticFolderPath(path);
      }
    } catch (e) {
      setDiagnosticsError(String(e));
    }
  }

  async function onOpenLatestDiagnosticZip() {
    if (!latestDiagnosticId) return;
    setDiagnosticsError("");
    try {
      const opened = await invoke("open_diagnostic_zip", { diagId: latestDiagnosticId });
      const path = String(opened ?? "").trim();
      if (path) {
        setLatestDiagnosticZipPath(path);
      }
    } catch (e) {
      setDiagnosticsError(String(e));
    }
  }

  async function onLoadDiagnosticReport(diagId) {
    if (!diagId) return;
    setDiagReportLoading(true);
    setDiagReportError("");
    try {
      const report = await invoke("read_diagnostic_report", { diagId });
      setDiagReport(String(report ?? ""));
    } catch (e) {
      setDiagReport("");
      setDiagReportError(String(e));
    } finally {
      setDiagReportLoading(false);
    }
  }

  async function onOpenDiagnosticFolder(diagId) {
    if (!diagId) return;
    try {
      await invoke("open_diagnostic_folder", { diagId });
    } catch (e) {
      alert(String(e));
    }
  }

  async function onOpenDiagnosticZip(diagId) {
    if (!diagId) return;
    try {
      await invoke("open_diagnostic_zip", { diagId });
    } catch (e) {
      alert(String(e));
    }
  }

  async function loadWorkspaceHistory() {
    setWorkspaceLoading(true);
    setWorkspaceError("");
    try {
      const [exportsRows, importsRows] = await Promise.all([
        invoke("list_workspace_exports"),
        invoke("list_workspace_imports"),
      ]);
      setWorkspaceExports(Array.isArray(exportsRows) ? exportsRows : []);
      setWorkspaceImports(Array.isArray(importsRows) ? importsRows : []);
    } catch (e) {
      setWorkspaceExports([]);
      setWorkspaceImports([]);
      setWorkspaceError(String(e));
    } finally {
      setWorkspaceLoading(false);
    }
  }

  async function onExportWorkspace() {
    setWorkspaceExporting(true);
    setWorkspaceError("");
    try {
      const res = await invoke("export_workspace", {
        opts: {
          include_audit: true,
          include_diag: false,
          audit_max_lines: 500,
          redact: true,
        },
      });
      await loadWorkspaceHistory();
      if (res?.zip_path) {
        setWorkspaceImportZipPath(String(res.zip_path));
      }
    } catch (e) {
      setWorkspaceError(String(e));
    } finally {
      setWorkspaceExporting(false);
    }
  }

  async function onImportWorkspace() {
    const zipPath = String(workspaceImportZipPath ?? "").trim();
    if (!zipPath) {
      setWorkspaceError("zip path is required");
      return;
    }
    setWorkspaceImporting(true);
    setWorkspaceError("");
    try {
      const res = await invoke("import_workspace", {
        opts: {
          zip_path: zipPath,
          mode: workspaceImportMode,
          dry_run: workspaceImportDryRun,
        },
      });
      if (res?.import_id) {
        setWorkspaceLastImportId(String(res.import_id));
      }
      setWorkspaceFixRuntimeMessage("Import completed. Click Fix runtime if pipeline_root is unresolved.");
      await Promise.all([
        loadWorkspaceHistory(),
        loadPipelines(),
        loadJobs(),
        loadRuns(),
        loadSettings(),
        loadRuntimeConfig(true),
        loadPreflight(),
      ]);
    } catch (e) {
      setWorkspaceError(String(e));
    } finally {
      setWorkspaceImporting(false);
    }
  }

  async function onFixRuntimeAfterImport() {
    setWorkspaceFixingRuntime(true);
    setWorkspaceError("");
    setCfgError("");
    setWorkspaceFixRuntimeMessage("");
    try {
      const firstRuntime = await invoke("reload_runtime_config");
      setRuntimeCfg(firstRuntime ?? null);
      const firstPreflight = await invoke("preflight_check");
      setPreflight(firstPreflight ?? null);

      let usedBootstrap = false;
      const checks = Array.isArray(firstPreflight?.checks) ? firstPreflight.checks : [];
      const pipelineRootCheck = checks.find((item) => item?.name === "pipeline_root");
      if (!pipelineRootCheck?.ok) {
        const boot = await invoke("bootstrap_pipeline_repo");
        const pipelineRoot = String(boot?.local_path ?? "").trim();
        if (!pipelineRoot) {
          throw new Error("bootstrap_pipeline_repo succeeded but local_path is empty");
        }
        const setResult = await invoke("set_config_pipeline_root", { pipeline_root: pipelineRoot });
        setRuntimeCfg(setResult ?? null);
        if (!setResult?.ok) {
          throw new Error(setResult?.message || "Failed to persist pipeline_root from bootstrap result");
        }
        usedBootstrap = true;
      }

      const finalRuntime = await invoke("reload_runtime_config");
      setRuntimeCfg(finalRuntime ?? null);
      const finalPreflight = await invoke("preflight_check");
      setPreflight(finalPreflight ?? null);
      await Promise.all([loadPipelineRepoStatus(), loadSettings(), loadRuns()]);

      if (!finalRuntime?.ok) {
        throw new Error(finalRuntime?.message || "runtime config unresolved after fix");
      }
      if (!finalPreflight?.ok) {
        throw new Error("preflight still NG after fix");
      }

      setWorkspaceFixRuntimeMessage(
        usedBootstrap
          ? "Runtime fixed: bootstrap + pipeline_root save completed."
          : "Runtime already healthy: reload + preflight are OK."
      );
    } catch (e) {
      const msg = String(e);
      setCfgError(msg);
      setWorkspaceFixRuntimeMessage(`Fix runtime failed: ${msg}`);
    } finally {
      setWorkspaceFixingRuntime(false);
    }
  }

  async function onOpenWorkspaceExportFolder(id) {
    if (!id) return;
    try {
      await invoke("open_workspace_export_folder", { exportId: id });
    } catch (e) {
      alert(String(e));
    }
  }

  async function onOpenWorkspaceExportZip(id) {
    if (!id) return;
    try {
      await invoke("open_workspace_export_zip", { exportId: id });
    } catch (e) {
      alert(String(e));
    }
  }

  async function onOpenWorkspaceImportFolder(id) {
    if (!id) return;
    try {
      await invoke("open_workspace_import_folder", { importId: id });
    } catch (e) {
      alert(String(e));
    }
  }

  async function onLoadWorkspaceReport(scope, id) {
    if (!scope || !id) return;
    setWorkspaceReportLoading(true);
    setWorkspaceReportError("");
    try {
      const command = scope === "export" ? "read_workspace_export_report" : "read_workspace_import_report";
      const argName = scope === "export" ? "exportId" : "importId";
      const report = await invoke(command, { [argName]: id });
      setWorkspaceReport(String(report ?? ""));
      setSelectedWorkspaceReport({ scope, id });
    } catch (e) {
      setWorkspaceReport("");
      setWorkspaceReportError(String(e));
    } finally {
      setWorkspaceReportLoading(false);
    }
  }

  async function onCopyZipPath(zipPath) {
    if (!zipPath) return;
    try {
      await navigator.clipboard?.writeText(zipPath);
    } catch (e) {
      alert(String(e));
    }
  }

  async function onRefreshRuntimeSnapshot() {
    setSnapshotLoading(true);
    await loadRuntimeConfig(true);
    await loadPreflight();
    await loadPipelineRepoStatus();
    await loadSettings();
    setSnapshotLoading(false);
  }

  async function loadPipelineDetail(pipelineId) {
    if (!pipelineId) {
      setPipelineDetail(null);
      return;
    }
    setPipelineDetailLoading(true);
    try {
      const detail = await invoke("get_pipeline", { pipelineId });
      setPipelineDetail(detail ?? null);
    } catch (e) {
      setPipelineDetail(null);
      setPipelinesError(String(e));
    } finally {
      setPipelineDetailLoading(false);
    }
  }

  async function onRunAnalyzePipeline() {
    const idForRun = normalized?.canonical?.trim() ? normalized.canonical : paperId;
    try {
      const pipelineId = await invoke("create_pipeline", {
        name: "Analyze Paper",
        canonicalId: idForRun,
        steps: buildAnalyzePipelineSteps(),
      });
      await invoke("start_pipeline", { pipelineId });
      await loadPipelines();
      await loadJobs();
      setSelectedPipelineId(pipelineId);
    } catch (e) {
      alert(String(e));
    }
  }

  async function onCancelPipeline(pipelineId) {
    if (!pipelineId) return;
    try {
      await invoke("cancel_pipeline", { pipelineId });
      await loadPipelines();
      await loadPipelineDetail(pipelineId);
      await loadJobs();
    } catch (e) {
      alert(String(e));
    }
  }

  async function onRetryPipelineStep(pipelineId, stepId, force = false) {
    if (!pipelineId || !stepId) return;
    try {
      await invoke("retry_pipeline_step", {
        pipelineId,
        stepId,
        force,
      });
      await loadPipelines();
      await loadPipelineDetail(pipelineId);
      await loadJobs();
    } catch (e) {
      alert(String(e));
    }
  }

  async function onResumePipeline(pipeline) {
    if (!pipeline?.pipeline_id) return;
    const steps = Array.isArray(pipeline.steps) ? pipeline.steps : [];
    const blocked = steps.find((s) => s?.status === "needs_retry")
      || steps[pipeline.current_step_index ?? 0]
      || null;
    if (!blocked?.step_id) return;
    await onRetryPipelineStep(pipeline.pipeline_id, blocked.step_id, false);
  }

  async function onOpenRunLogs(runIdFromRow) {
    if (!runIdFromRow) return;
    let target = "";
    try {
      const items = await invoke("list_run_artifacts", { runId: runIdFromRow });
      const list = Array.isArray(items) ? items : [];
      const hasStdout = list.some((i) => i?.name === "stdout.log");
      const hasStderr = list.some((i) => i?.name === "stderr.log");
      target = hasStdout ? "stdout.log" : (hasStderr ? "stderr.log" : "");
    } catch {
      target = "";
    }
    if (!target) return;
    await onOpenNamedArtifactForRun(runIdFromRow, target);
    setActiveScreen("main");
  }

  async function loadArtifact(runId, artifactKey) {
    if (!runId) {
      setArtifactView(null);
      setArtifactWarnings([]);
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
      setArtifactWarnings([]);
    } catch (e) {
      setArtifactView(null);
      setArtifactWarnings([]);
      setArtifactError(String(e));
    } finally {
      setArtifactLoading(false);
    }
  }

  function mapArtifactNameToLegacyKey(name) {
    if (name === "tree.md") return "tree_md";
    if (name === "result.json") return "result_json";
    if (name === "input.json") return "input_json";
    if (name === "stdout.log") return "stdout_log";
    if (name === "stderr.log") return "stderr_log";
    return "";
  }

  async function fetchArtifactCatalogForRun(runId, force = false) {
    if (!runId) return;
    let shouldFetch = false;
    setArtifactCatalogByRun((prev) => {
      const current = prev[runId];
      if (!force && current && (current.loading || current.loaded)) {
        return prev;
      }
      shouldFetch = true;
      return {
        ...prev,
        [runId]: {
          loading: true,
          loaded: false,
          error: "",
          items: current?.items ?? [],
          names: current?.names ?? {},
        },
      };
    });
    if (!shouldFetch) return;

    try {
      const items = await invoke("list_run_artifacts", { runId });
      const list = Array.isArray(items) ? items : [];
      const names = {};
      for (const item of list) {
        names[item.name] = true;
      }
      setArtifactCatalogByRun((prev) => ({
        ...prev,
        [runId]: {
          loading: false,
          loaded: true,
          error: "",
          items: list,
          names,
        },
      }));
    } catch (e) {
      setArtifactCatalogByRun((prev) => ({
        ...prev,
        [runId]: {
          loading: false,
          loaded: true,
          error: String(e),
          items: [],
          names: {},
        },
      }));
    }
  }

  async function loadSelectedRunArtifactCatalog(runId) {
    if (!runId) {
      setRunArtifactCatalog([]);
      setRunArtifactCatalogError("");
      return;
    }
    setRunArtifactCatalogLoading(true);
    setRunArtifactCatalogError("");
    try {
      const items = await invoke("list_run_artifacts", { runId });
      const list = Array.isArray(items) ? items : [];
      setRunArtifactCatalog(list);
      const names = {};
      for (const item of list) {
        names[item.name] = true;
      }
      setArtifactCatalogByRun((prev) => ({
        ...prev,
        [runId]: {
          loading: false,
          loaded: true,
          error: "",
          items: list,
          names,
        },
      }));
    } catch (e) {
      setRunArtifactCatalog([]);
      setRunArtifactCatalogError(String(e));
    } finally {
      setRunArtifactCatalogLoading(false);
    }
  }

  async function loadLibraryRows(nextFilters = libraryFilters) {
    setLibraryLoading(true);
    setLibraryError("");
    try {
      const payload = {};
      for (const [k, v] of Object.entries(nextFilters)) {
        if (String(v ?? "").trim() !== "") {
          payload[k] = v;
        }
      }
      const rows = await invoke("library_list", {
        filters: payload,
      });
      const list = Array.isArray(rows) ? rows : [];
      setLibraryRows(list);
      setSelectedPaperKey((prev) => {
        if (prev && list.some((r) => r.paper_key === prev)) return prev;
        return list[0]?.paper_key ?? "";
      });
    } catch (e) {
      setLibraryRows([]);
      setLibraryError(String(e));
    } finally {
      setLibraryLoading(false);
    }
  }

  async function loadLibrarySearch(query, nextFilters = libraryFilters) {
    const normalized = String(query ?? "").trim();
    if (!normalized) {
      setLibrarySearchRows([]);
      setLibrarySearchLoading(false);
      return;
    }
    setLibrarySearchLoading(true);
    setLibraryError("");
    try {
      const opts = {};
      if (String(nextFilters.status ?? "").trim()) opts.status = nextFilters.status;
      if (String(nextFilters.kind ?? "").trim()) opts.kind = nextFilters.kind;
      if (String(nextFilters.tag ?? "").trim()) opts.tag = nextFilters.tag;
      opts.limit = 300;

      const rows = await invoke("library_search", {
        query: normalized,
        opts,
      });
      const list = Array.isArray(rows) ? rows : [];
      setLibrarySearchRows(list);
      setSelectedPaperKey((prev) => {
        if (prev && list.some((r) => r.paper_key === prev)) return prev;
        return list[0]?.paper_key ?? "";
      });
    } catch (e) {
      setLibrarySearchRows([]);
      setLibraryError(String(e));
    } finally {
      setLibrarySearchLoading(false);
    }
  }

  async function loadLibraryStats() {
    try {
      const stats = await invoke("library_stats");
      setLibraryStats(stats);
    } catch {
      setLibraryStats(null);
    }
  }

  async function loadLibraryDetail(paperKey) {
    if (!paperKey) {
      setLibraryDetail(null);
      return;
    }
    try {
      const rec = await invoke("library_get", { paperKey });
      setLibraryDetail(rec);
      setTagInput(Array.isArray(rec?.tags) ? rec.tags.join(", ") : "");
    } catch (e) {
      setLibraryDetail(null);
      setLibraryError(String(e));
    }
  }

  async function onLibraryReindex() {
    setLibraryLoading(true);
    setLibraryError("");
    try {
      const info = await invoke("library_reindex", { full: true });
      await invoke("library_reload");
      setLibraryReindexInfo(info);
      if (String(librarySearchQuery).trim()) {
        await loadLibrarySearch(librarySearchQuery, libraryFilters);
      } else {
        await loadLibraryRows(libraryFilters);
      }
      await loadLibraryStats();
    } catch (e) {
      setLibraryError(String(e));
    } finally {
      setLibraryLoading(false);
    }
  }

  async function onSaveTags() {
    if (!selectedPaperKey) return;
    const tags = tagInput
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    try {
      const updated = await invoke("library_set_tags", {
        paperKey: selectedPaperKey,
        tags,
      });
      setLibraryDetail(updated);
      if (String(librarySearchQuery).trim()) {
        await loadLibrarySearch(librarySearchQuery, libraryFilters);
      } else {
        await loadLibraryRows(libraryFilters);
      }
      await loadLibraryStats();
    } catch (e) {
      setLibraryError(String(e));
    }
  }

  async function onOpenRunFromLibrary(runIdFromLibrary) {
    if (!runIdFromLibrary) return;
    await loadRuns();
    setSelectedRunId(runIdFromLibrary);
  }

  async function onOpenArtifactQuick(runIdFromLibrary, artifactName) {
    if (!runIdFromLibrary) return;
    const key = mapArtifactNameToLegacyKey(artifactName);
    if (!key) return;
    await loadRuns();
    setSelectedRunId(runIdFromLibrary);
    setSelectedArtifact(key);
  }

  async function onOpenCatalogArtifact(item) {
    if (!selectedRunId || !item?.name) return;
    const key = mapArtifactNameToLegacyKey(item.name);
    if (key) {
      setSelectedArtifact(key);
      setArtifactWarnings([]);
      return;
    }

    setArtifactLoading(true);
    setArtifactError("");
    try {
      const named = await invoke("read_run_artifact_named", {
        runId: selectedRunId,
        name: item.name,
      });
      setArtifactView({
        run_id: selectedRunId,
        artifact: item.name,
        path: item.rel_path,
        exists: true,
        kind: named?.kind ?? item.kind ?? "text",
        content: named?.content ?? "",
        parse_status: named?.truncated ? "truncated" : "ok",
      });
      setArtifactWarnings(Array.isArray(named?.warnings) ? named.warnings : []);
    } catch (e) {
      setArtifactView(null);
      setArtifactWarnings([]);
      setArtifactError(String(e));
    } finally {
      setArtifactLoading(false);
    }
  }

  async function onOpenNamedArtifactForRun(runIdFromRow, itemName) {
    if (!runIdFromRow || !itemName) return;
    await loadRuns();
    setSelectedRunId(runIdFromRow);

    const key = mapArtifactNameToLegacyKey(itemName);
    if (key) {
      setSelectedArtifact(key);
      setArtifactWarnings([]);
      return;
    }

    setArtifactLoading(true);
    setArtifactError("");
    try {
      const named = await invoke("read_run_artifact_named", {
        runId: runIdFromRow,
        name: itemName,
      });
      setArtifactView({
        run_id: runIdFromRow,
        artifact: itemName,
        path: itemName,
        exists: true,
        kind: named?.kind ?? "text",
        content: named?.content ?? "",
        parse_status: named?.truncated ? "truncated" : "ok",
      });
      setArtifactWarnings(Array.isArray(named?.warnings) ? named.warnings : []);
    } catch (e) {
      setArtifactView(null);
      setArtifactWarnings([]);
      setArtifactError(String(e));
    } finally {
      setArtifactLoading(false);
    }
  }

  useEffect(() => {
    loadRuntimeConfig(false);
    loadPreflight();
    loadTemplates();
    loadRuns();
    loadPipelineRuns();
    loadJobs();
    loadPipelines();
    loadSettings();
    loadPipelineRepoStatus();
    loadDiagnostics();
    loadWorkspaceHistory();
    loadLibraryRows();
    loadLibraryStats();
  }, []);

  useEffect(() => {
    const timer = setInterval(() => {
      loadJobs();
      loadPipelines();
    }, 1500);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    const onPopState = () => {
      setActiveScreen(screenFromPathname(window.location.pathname));
    };
    window.addEventListener("popstate", onPopState);
    return () => {
      window.removeEventListener("popstate", onPopState);
    };
  }, []);

  useEffect(() => {
    const desiredPath = pathFromScreen(activeScreen);
    const currentPath = normalizePathname(window.location.pathname);
    if (currentPath !== desiredPath) {
      window.history.replaceState(window.history.state, "", desiredPath);
    }
  }, [activeScreen]);

  useEffect(() => {
    if (activeScreen !== "ops") return;
    tickAutoRetry();
    loadPipelineRepoStatus();
    loadDiagnostics();
    loadWorkspaceHistory();
    const timer = setInterval(() => {
      tickAutoRetry();
    }, 2000);
    return () => clearInterval(timer);
  }, [activeScreen]);

  useEffect(() => {
    if (activeScreen !== "runs") return;
    loadPipelineRuns();
  }, [activeScreen]);

  useEffect(() => {
    loadPipelineDetail(selectedPipelineId);
  }, [selectedPipelineId]);

  useEffect(() => {
    if (!selectedDiagId) {
      setDiagReport("");
      setDiagReportError("");
      return;
    }
    onLoadDiagnosticReport(selectedDiagId);
  }, [selectedDiagId]);

  useEffect(() => {
    loadLibraryDetail(selectedPaperKey);
  }, [selectedPaperKey]);

  useEffect(() => {
    const q = String(librarySearchQuery ?? "").trim();
    if (!q) {
      setLibrarySearchRows([]);
      setLibrarySearchLoading(false);
      return;
    }
    setLibrarySearchLoading(true);
    const timer = setTimeout(() => {
      loadLibrarySearch(q, libraryFilters);
    }, 260);
    return () => clearTimeout(timer);
  }, [librarySearchQuery, libraryFilters.status, libraryFilters.kind, libraryFilters.tag]);

  const selectedTemplate = templates.find((t) => t.id === selectedTemplateId) ?? null;

  useEffect(() => {
    if (!selectedTemplate) {
      setTemplateParams({});
      return;
    }
    const defaults = {};
    for (const p of selectedTemplate.params ?? []) {
      defaults[p.key] = p.default_value;
    }
    setTemplateParams(defaults);
  }, [selectedTemplateId, selectedTemplate?.id]);

  useEffect(() => {
    const timer = setTimeout(async () => {
      setNormalizeLoading(true);
      try {
        const res = await invoke("normalize_identifier", { input: paperId });
        setNormalized(res);
      } catch (e) {
        setNormalized({
          kind: "unknown",
          canonical: "",
          display: "unknown",
          warnings: [],
          errors: [String(e)],
        });
      } finally {
        setNormalizeLoading(false);
      }
    }, 220);

    return () => clearTimeout(timer);
  }, [paperId]);

  useEffect(() => {
    loadArtifact(selectedRunId, selectedArtifact);
  }, [selectedRunId, selectedArtifact]);

  useEffect(() => {
    loadSelectedRunArtifactCatalog(selectedRunId);
  }, [selectedRunId]);

  useEffect(() => {
    loadPipelineRunText(selectedPipelineRunId, pipelineRunTab);
  }, [selectedPipelineRunId, pipelineRunTab]);

  useEffect(() => {
    const kind = artifactView?.kind ?? "";
    if (kind !== "graph_json" || !artifactView?.content) {
      setGraphParsed(null);
      setGraphParseLoading(false);
      setGraphParseError("");
      setSelectedGraphNodeId("");
      return;
    }

    let alive = true;
    setGraphParseLoading(true);
    setGraphParseError("");
    invoke("parse_graph_json", { content: artifactView.content })
      .then((parsed) => {
        if (!alive) return;
        setGraphParsed(parsed ?? null);
        const firstId = Array.isArray(parsed?.nodes) && parsed.nodes.length > 0 ? parsed.nodes[0].id : "";
        setSelectedGraphNodeId(firstId);
      })
      .catch((e) => {
        if (!alive) return;
        setGraphParsed(null);
        setGraphParseError(String(e));
      })
      .finally(() => {
        if (!alive) return;
        setGraphParseLoading(false);
      });

    return () => {
      alive = false;
    };
  }, [artifactView?.kind, artifactView?.content]);

  useEffect(() => {
    const searchMode = String(librarySearchQuery ?? "").trim() !== "";
    const rows = Array.isArray(searchMode ? librarySearchRows : libraryRows)
      ? searchMode
        ? librarySearchRows
        : libraryRows
      : [];
    const ids = [...new Set(rows.map((r) => r.last_run_id).filter(Boolean))].slice(0, 24);
    for (const id of ids) {
      fetchArtifactCatalogForRun(id, false);
    }
  }, [libraryRows, librarySearchRows, librarySearchQuery]);

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
      const jobId = await invoke("enqueue_job", {
        templateId: params.templateId,
        canonicalId: params.canonicalId,
        params: params.templateParams,
      });

      setStdout(`enqueued job_id=${jobId}`);
      setStderr("");
      setExitCode(0);
      setRunId(null);
      setRunDir(null);
      setStatus("queued");
      setMessage(`Job queued: ${jobId}`);
      setRetryAfterSec(null);
      setLastRunRequest(params);
      await loadJobs();
      setSelectedJobId(jobId);
    } catch (e) {
      setStderr(String(e));
      setStatus("error");
      setMessage(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function onRunTree() {
    const idForRun = normalized?.canonical?.trim() ? normalized.canonical : paperId;
    await runTree({
      templateId: selectedTemplateId,
      canonicalId: idForRun,
      templateParams,
    });
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

  async function onCancelJob(jobId) {
    try {
      await invoke("cancel_job", { jobId });
      await loadJobs();
    } catch (e) {
      alert(String(e));
    }
  }

  async function onRetryJob(jobId, force = false) {
    try {
      await invoke("retry_job", { jobId, force });
      await loadJobs();
    } catch (e) {
      alert(String(e));
    }
  }

  async function onOpenRunFromJob(job) {
    if (!job?.run_id) return;
    await loadRuns();
    setSelectedRunId(job.run_id);
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
      await loadPreflight();
      await loadRuns();
    } catch (e) {
      alert(String(e));
    }
  }

  async function onSelectPipelineRootFolder() {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "Select Pipeline Root",
      });
      if (typeof selected === "string") {
        setPipelineRootDraft(selected);
      }
    } catch (e) {
      setCfgError(String(e));
    }
  }

  async function onSelectOutDirFolder() {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "Select Out Dir",
      });
      if (typeof selected === "string") {
        setOutDirDraft(selected);
      }
    } catch (e) {
      setCfgError(String(e));
    }
  }

  async function onApplyPipelineRootOverride() {
    setCfgLoading(true);
    setCfgError("");
    try {
      const res = await invoke("set_config_pipeline_root", { pipeline_root: pipelineRootDraft });
      setRuntimeCfg(res);
      if (!res?.ok) {
        setCfgError(res?.message || "Failed to set pipeline root");
      } else {
        await loadPreflight();
        await loadRuns();
      }
    } catch (e) {
      setCfgError(String(e));
    } finally {
      setCfgLoading(false);
    }
  }

  async function onClearPipelineRootOverride() {
    setCfgLoading(true);
    setCfgError("");
    try {
      const res = await invoke("clear_config_pipeline_root");
      setRuntimeCfg(res);
      if (!res?.ok) {
        setCfgError(res?.message || "Failed to clear pipeline root");
      } else {
        await loadPreflight();
        await loadRuns();
      }
    } catch (e) {
      setCfgError(String(e));
    } finally {
      setCfgLoading(false);
    }
  }

  async function onApplyOutDirOverride() {
    setCfgLoading(true);
    setCfgError("");
    try {
      const res = await invoke("set_config_out_dir", { out_dir: outDirDraft });
      setRuntimeCfg(res);
      if (!res?.ok) {
        setCfgError(res?.message || "Failed to set out_dir");
      } else {
        await loadPreflight();
        await loadRuns();
      }
    } catch (e) {
      setCfgError(String(e));
    } finally {
      setCfgLoading(false);
    }
  }

  async function onClearOutDirOverride() {
    setCfgLoading(true);
    setCfgError("");
    try {
      const res = await invoke("clear_config_out_dir");
      setRuntimeCfg(res);
      if (!res?.ok) {
        setCfgError(res?.message || "Failed to clear out_dir");
      } else {
        await loadPreflight();
        await loadRuns();
      }
    } catch (e) {
      setCfgError(String(e));
    } finally {
      setCfgLoading(false);
    }
  }

  useEffect(() => {
    setPipelineRootDraft(runtimeCfg?.pipeline_root ?? "");
  }, [runtimeCfg?.pipeline_root]);

  useEffect(() => {
    setOutDirDraft(runtimeCfg?.out_dir ?? "");
  }, [runtimeCfg?.out_dir]);

  const normalizeErrors = Array.isArray(normalized?.errors) ? normalized.errors : [];
  const normalizeWarnings = Array.isArray(normalized?.warnings) ? normalized.warnings : [];
  const canRunByNormalization = normalizeErrors.length === 0 && !!normalized?.canonical;
  const canRunByPreflight = preflight?.ok === true;
  const preflightChecks = Array.isArray(preflight?.checks) ? preflight.checks : [];
  const preflightPipelineRoot = preflightChecks.find((x) => x?.name === "pipeline_root");
  const preflightOutDir = preflightChecks.find((x) => x?.name === "out_dir");
  const preflightPython = preflightChecks.find((x) => x?.name === "python");
  const preflightMarkers = preflightChecks.find((x) => x?.name === "pipeline_markers");
  const canRunByTemplate = !!selectedTemplate && selectedTemplate.wired === true;
  const runDisabled = running || !canRunByNormalization || !canRunByPreflight || !canRunByTemplate;
  const templateRequiredFields = Array.isArray(selectedTemplate?.required_fields)
    ? selectedTemplate.required_fields
    : [];
  const missingTemplateRequiredFields = templateRequiredFields.filter((key) => {
    const value = templateParams?.[key];
    if (value === null || value === undefined) return true;
    if (typeof value === "string" && value.trim() === "") return true;
    return false;
  });
  const runtimePipelineRootResolved = runtimeCfg?.ok === true
    && String(runtimeCfg?.pipeline_root ?? "").trim() !== ""
    && String(runtimeCfg?.pipeline_root ?? "").trim() !== "-";
  const runtimeOutDirResolved = runtimeCfg?.ok === true
    && String(runtimeCfg?.out_dir ?? "").trim() !== ""
    && String(runtimeCfg?.out_dir ?? "").trim() !== "-";
  const pipelineStartMissingRequirements = [];
  if (!selectedTemplate || !selectedTemplateId) {
    pipelineStartMissingRequirements.push("Template is not selected.");
  } else if (selectedTemplate.wired !== true) {
    pipelineStartMissingRequirements.push(`Selected template is not wired: ${selectedTemplate.id}`);
  }
  if (missingTemplateRequiredFields.length > 0) {
    pipelineStartMissingRequirements.push(
      `Missing template required fields: ${missingTemplateRequiredFields.join(", ")}`
    );
  }
  if (!canRunByNormalization) {
    pipelineStartMissingRequirements.push("Paper ID is not normalized to a canonical identifier.");
  }
  if (!runtimePipelineRootResolved) {
    pipelineStartMissingRequirements.push("pipeline_root is unresolved.");
  }
  if (!runtimeOutDirResolved) {
    pipelineStartMissingRequirements.push("out_dir is unresolved.");
  }
  if (preflightError) {
    pipelineStartMissingRequirements.push(`preflight_check failed: ${preflightError}`);
  } else if (preflight?.ok !== true) {
    pipelineStartMissingRequirements.push("preflight_check is not OK.");
  }
  const pipelineStartDisabled = pipelineStartMissingRequirements.length > 0;

  const showRetryButton = status === "needs_retry" && !!lastRunRequest;
  const pipelineRunQueryNormalized = String(pipelineRunQuery ?? "").trim().toLowerCase();
  const visiblePipelineRuns = pipelineRunQueryNormalized
    ? pipelineRuns.filter((row) => {
      const hay = `${row?.run_id ?? ""} ${row?.canonical_id ?? ""} ${row?.template_id ?? ""}`.toLowerCase();
      return hay.includes(pipelineRunQueryNormalized);
    })
    : pipelineRuns;
  const selectedRun = runs.find((r) => r.run_id === selectedRunId) ?? null;
  const selectedPipelineRun = pipelineRuns.find((r) => r.run_id === selectedPipelineRunId) ?? null;
  const artifactIsMissing = artifactView && artifactView.exists === false;
  const createdAtText = selectedRun?.created_at_epoch_ms
    ? new Date(selectedRun.created_at_epoch_ms).toLocaleString()
    : "-";
  const selectedJob = jobs.find((j) => j.job_id === selectedJobId) ?? null;
  const selectedPipelineSummary = pipelines.find((p) => p.pipeline_id === selectedPipelineId) ?? null;
  const selectedPipeline = pipelineDetail && pipelineDetail.pipeline_id === selectedPipelineId
    ? pipelineDetail
    : null;
  const opsPipelineRows = useMemo(() => {
    const rows = Array.isArray(pipelines) ? pipelines : [];
    if (!opsNeedsAttentionOnly) return rows;
    return rows.filter((p) => p?.status === "failed" || p?.status === "needs_retry");
  }, [pipelines, opsNeedsAttentionOnly]);
  const opsJobRows = useMemo(() => {
    let rows = Array.isArray(jobs) ? jobs : [];
    if (opsNeedsAttentionOnly) {
      rows = rows.filter((j) => j?.status === "failed" || j?.status === "needs_retry");
    }
    if (opsAutoRetryPendingOnly) {
      rows = rows.filter((j) => j?.status === "needs_retry" && !!j?.retry_at);
    }
    return rows;
  }, [jobs, opsNeedsAttentionOnly, opsAutoRetryPendingOnly]);
  const opsRunRows = useMemo(() => (Array.isArray(runs) ? runs : []), [runs]);
  const isLibrarySearchMode = String(librarySearchQuery ?? "").trim() !== "";
  const visibleLibraryRows = isLibrarySearchMode ? librarySearchRows : libraryRows;
  const artifactKind = artifactView?.kind ?? "";
  const isHtmlArtifact = artifactKind === "html";
  const isGraphJsonArtifact = artifactKind === "graph_json";
  const graphNodes = Array.isArray(graphParsed?.nodes) ? graphParsed.nodes : [];
  const graphEdges = Array.isArray(graphParsed?.edges) ? graphParsed.edges : [];
  const graphTypes = useMemo(() => {
    const set = new Set();
    for (const n of graphNodes) {
      const t = String(n?.node_type ?? "").trim();
      if (t) set.add(t);
    }
    return ["all", ...Array.from(set).sort((a, b) => a.localeCompare(b))];
  }, [graphNodes]);
  const graphNodeById = useMemo(() => {
    const map = new Map();
    for (const n of graphNodes) {
      map.set(n.id, n);
    }
    return map;
  }, [graphNodes]);
  const graphDegreeMap = useMemo(() => {
    const degree = new Map();
    for (const e of graphEdges) {
      const s = String(e?.source ?? "");
      const t = String(e?.target ?? "");
      if (s) degree.set(s, (degree.get(s) ?? 0) + 1);
      if (t) degree.set(t, (degree.get(t) ?? 0) + 1);
    }
    return degree;
  }, [graphEdges]);
  const graphAdjacency = useMemo(() => {
    const adj = new Map();
    for (const e of graphEdges) {
      const s = String(e?.source ?? "");
      const t = String(e?.target ?? "");
      if (!s || !t) continue;
      if (!adj.has(s)) adj.set(s, new Set());
      if (!adj.has(t)) adj.set(t, new Set());
      adj.get(s).add(t);
      adj.get(t).add(s);
    }
    return adj;
  }, [graphEdges]);
  const filteredGraphNodes = useMemo(() => {
    const q = String(graphQuery ?? "").trim().toLowerCase();
    const yFrom = String(graphYearFrom ?? "").trim() === "" ? null : Number(graphYearFrom);
    const yTo = String(graphYearTo ?? "").trim() === "" ? null : Number(graphYearTo);

    const rows = graphNodes.filter((n) => {
      const id = String(n?.id ?? "");
      const label = String(n?.label ?? "");
      if (q) {
        const hay = `${id.toLowerCase()} ${label.toLowerCase()}`;
        if (!hay.includes(q)) return false;
      }
      if (graphTypeFilter !== "all" && String(n?.node_type ?? "") !== graphTypeFilter) return false;
      if (Number.isFinite(yFrom) && Number(n?.year ?? NaN) < yFrom) return false;
      if (Number.isFinite(yTo) && Number(n?.year ?? NaN) > yTo) return false;
      if (graphHasEdgesOnly && (graphDegreeMap.get(id) ?? 0) <= 0) return false;
      return true;
    });

    rows.sort((a, b) => {
      const da = graphDegreeMap.get(a.id) ?? 0;
      const db = graphDegreeMap.get(b.id) ?? 0;
      return db - da || String(a.id).localeCompare(String(b.id));
    });
    return rows;
  }, [graphNodes, graphQuery, graphTypeFilter, graphYearFrom, graphYearTo, graphHasEdgesOnly, graphDegreeMap]);
  const selectedGraphNode = selectedGraphNodeId ? graphNodeById.get(selectedGraphNodeId) : null;
  const selectedGraphNeighbors = useMemo(() => {
    if (!selectedGraphNode) return [];
    const ids = Array.from(graphAdjacency.get(selectedGraphNode.id) ?? []);
    const nodes = ids
      .map((id) => graphNodeById.get(id))
      .filter(Boolean)
      .sort((a, b) => {
        const da = graphDegreeMap.get(a.id) ?? 0;
        const db = graphDegreeMap.get(b.id) ?? 0;
        return db - da || String(a.id).localeCompare(String(b.id));
      });
    return nodes.slice(0, 50);
  }, [selectedGraphNode, graphAdjacency, graphNodeById, graphDegreeMap]);

  function renderRowArtifactButtons(row) {
    const runIdFromRow = row?.last_run_id;
    const catalog = runIdFromRow ? artifactCatalogByRun[runIdFromRow] : null;
    const loaded = !!catalog?.loaded;
    const hasName = (name) => !!catalog?.names?.[name];
    const hasLogs = hasName("stdout.log") || hasName("stderr.log");
    const logTarget = hasName("stdout.log") ? "stdout.log" : "stderr.log";
    const preferredViz = row?.primary_viz && row.primary_viz.name
      ? { name: String(row.primary_viz.name), kind: String(row.primary_viz.kind ?? "") }
      : null;
    const primaryViewTarget = preferredViz
      ? (catalog?.items ?? []).find((i) => i.name === preferredViz.name)
      : null;
    const fallbackViewTarget = (catalog?.items ?? []).find((i) => i.kind === "html")
      || (catalog?.items ?? []).find((i) => i.kind === "graph_json");
    const viewTarget = primaryViewTarget || fallbackViewTarget;

    const makeDisabled = (name) => {
      if (!runIdFromRow) return true;
      if (!loaded) return true;
      if (name === "logs") return !hasLogs;
      return !hasName(name);
    };

    const makeTitle = (name) => {
      if (!runIdFromRow) return "not available";
      if (!loaded) return "checking availability...";
      if (name === "logs") return hasLogs ? "Open logs" : "not available";
      return hasName(name) ? `Open ${name}` : "not available";
    };

    return (
      <div style={{ marginTop: 6, display: "flex", gap: 6, flexWrap: "wrap" }}>
        <button
          title={makeTitle("tree.md")}
          onClick={(e) => {
            e.stopPropagation();
            onOpenArtifactQuick(runIdFromRow, "tree.md");
          }}
          disabled={makeDisabled("tree.md")}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          Tree
        </button>
        <button
          title={makeTitle("result.json")}
          onClick={(e) => {
            e.stopPropagation();
            onOpenArtifactQuick(runIdFromRow, "result.json");
          }}
          disabled={makeDisabled("result.json")}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          Result
        </button>
        <button
          title={makeTitle("input.json")}
          onClick={(e) => {
            e.stopPropagation();
            onOpenArtifactQuick(runIdFromRow, "input.json");
          }}
          disabled={makeDisabled("input.json")}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          Input
        </button>
        <button
          title={makeTitle("logs")}
          onClick={(e) => {
            e.stopPropagation();
            onOpenArtifactQuick(runIdFromRow, logTarget);
          }}
          disabled={makeDisabled("logs")}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          Logs
        </button>
        <button
          title={!runIdFromRow ? "not available" : (!loaded ? "checking availability..." : (viewTarget ? `Open ${viewTarget.name}` : "not available"))}
          onClick={(e) => {
            e.stopPropagation();
            if (viewTarget?.name) {
              onOpenNamedArtifactForRun(runIdFromRow, viewTarget.name);
            }
          }}
          disabled={!runIdFromRow || !loaded || !viewTarget}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          View
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onOpenRunFromLibrary(runIdFromRow);
          }}
          disabled={!runIdFromRow}
          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
        >
          Open last run
        </button>
      </div>
    );
  }

  return (
    <div style={{ fontFamily: "system-ui", padding: 16, maxWidth: 960 }}>
      <h2 style={{ marginTop: 0 }}>Javis Desktop</h2>

      <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
        <button
          onClick={() => setActiveScreen("setup")}
          style={{
            padding: "8px 12px",
            borderRadius: 8,
            border: "1px solid #333",
            background: activeScreen === "setup" ? "#eef5ff" : "white",
          }}
        >
          Setup
        </button>
        <button
          onClick={() => setActiveScreen("main")}
          style={{
            padding: "8px 12px",
            borderRadius: 8,
            border: "1px solid #333",
            background: activeScreen === "main" ? "#eef5ff" : "white",
          }}
        >
          Main
        </button>
        <button
          onClick={() => setActiveScreen("ops")}
          style={{
            padding: "8px 12px",
            borderRadius: 8,
            border: "1px solid #333",
            background: activeScreen === "ops" ? "#eef5ff" : "white",
          }}
        >
          Ops
        </button>
        <button
          onClick={() => setActiveScreen("runs")}
          style={{
            padding: "8px 12px",
            borderRadius: 8,
            border: "1px solid #333",
            background: activeScreen === "runs" ? "#eef5ff" : "white",
          }}
        >
          Runs
        </button>
      </div>

      {activeScreen === "setup" ? (
      <div style={{ border: "1px solid #ddd", borderRadius: 10, padding: 12, background: "#fafafa" }}>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Pipeline Engine</div>
        <div style={{ fontSize: 12, marginBottom: 8 }}>
          remote_url / local_path / ref を確認して、Clone/Update/Validate を実行します。
        </div>
        <div style={{ display: "grid", gap: 8 }}>
          <label style={{ display: "grid", gap: 4 }}>
            <span style={{ fontSize: 12 }}>remote_url</span>
            <input
              value={pipelineRepoRemoteUrl}
              onChange={(e) => setPipelineRepoRemoteUrl(e.target.value)}
              style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid #bbb" }}
            />
          </label>
          <label style={{ display: "grid", gap: 4 }}>
            <span style={{ fontSize: 12 }}>local_path</span>
            <input
              value={pipelineRepoLocalPath}
              onChange={(e) => setPipelineRepoLocalPath(e.target.value)}
              style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid #bbb" }}
            />
          </label>
          <label style={{ display: "grid", gap: 4 }}>
            <span style={{ fontSize: 12 }}>git_ref</span>
            <input
              value={pipelineRepoRef}
              onChange={(e) => setPipelineRepoRef(e.target.value)}
              style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid #bbb" }}
            />
          </label>
        </div>

        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 10 }}>
          <button onClick={onSavePipelineRepoSettings} disabled={pipelineRepoBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Save settings
          </button>
          <button onClick={onBootstrapPipelineRepo} disabled={pipelineRepoBusy || bootstrapLogBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Clone / Setup
          </button>
          <button onClick={onBootstrapPipelineRepoWithLogs} disabled={pipelineRepoBusy || bootstrapLogBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            {bootstrapLogBusy ? "Bootstrap (show logs)..." : "Bootstrap (show logs)"}
          </button>
          <button onClick={onUpdatePipelineRepo} disabled={pipelineRepoBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Update
          </button>
          <button onClick={onValidatePipelineRepo} disabled={pipelineRepoBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Validate
          </button>
          <button onClick={onOpenPipelineRepoFolder} disabled={pipelineRepoBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Open folder
          </button>
          <button onClick={onClearBootstrapLogs} disabled={bootstrapLogBusy} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Clear logs
          </button>
          <button onClick={() => setActiveScreen("main")} style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}>
            Continue to Main
          </button>
        </div>

        {pipelineRepoBusy || bootstrapLogBusy ? <div style={{ marginTop: 8, fontSize: 12 }}>実行中...</div> : null}
        <div
          style={{
            marginTop: 8,
            border: "1px solid #ddd",
            borderRadius: 8,
            background: "#fff",
            padding: 8,
            maxHeight: 220,
            overflow: "auto",
          }}
        >
          <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Bootstrap logs</div>
          <pre style={{ margin: 0, fontSize: 11, whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
            {bootstrapLogLines.length ? bootstrapLogLines.join("\n") : "(no logs)"}
          </pre>
        </div>
        {pipelineRepoStatus ? (
          <div style={{ marginTop: 8, fontSize: 12 }}>
            <div>status: <code>{pipelineRepoStatus.ok ? "ok" : "ng"}</code> / {pipelineRepoStatus.message}</div>
            <div>commit: <code>{pipelineRepoStatus.head_commit ?? "-"}</code></div>
            <div>last_sync: <code>{pipelineRepoStatus.last_sync_at ?? "-"}</code></div>
            <div>dirty: <code>{pipelineRepoStatus.dirty ? "yes" : "no"}</code></div>
          </div>
        ) : null}
        {pipelineRepoValidate ? (
          <div style={{ marginTop: 8, fontSize: 12 }}>
            validate: <code>{pipelineRepoValidate.ok ? "ok" : "ng"}</code>
            <div style={{ marginTop: 4 }}>
              {(Array.isArray(pipelineRepoValidate.checks) ? pipelineRepoValidate.checks : []).map((c) => (
                <div key={c.name} style={{ color: c.ok ? "#1f6f3f" : "#a33" }}>
                  {c.name}: {c.ok ? "ok" : "ng"} - {c.detail}
                </div>
              ))}
            </div>
          </div>
        ) : null}
        {pipelineRepoError ? <div style={{ marginTop: 8, color: "#a33", fontSize: 12 }}>{pipelineRepoError}</div> : null}
      </div>
      ) : activeScreen === "main" ? (
      <>

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
        <div style={{ marginTop: 10, fontSize: 12, fontWeight: 600 }}>
          Pipeline root override (config.json)
        </div>
        <div style={{ marginTop: 6, display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <input
            value={pipelineRootDraft}
            onChange={(e) => setPipelineRootDraft(e.target.value)}
            placeholder="C:\\path\\to\\jarvis-ml-pipeline"
            style={{ padding: 8, borderRadius: 8, border: "1px solid #ccc", minWidth: 380, flex: 1 }}
          />
          <button
            onClick={onSelectPipelineRootFolder}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Select folder...
          </button>
          <button
            onClick={onApplyPipelineRootOverride}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Apply
          </button>
          <button
            onClick={onClearPipelineRootOverride}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Clear
          </button>
        </div>
        <div style={{ marginTop: 10, fontSize: 12, fontWeight: 600 }}>
          Out dir override (config.json)
        </div>
        <div style={{ marginTop: 6, display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <input
            value={outDirDraft}
            onChange={(e) => setOutDirDraft(e.target.value)}
            placeholder="logs\\runs or C:\\path\\to\\runs"
            style={{ padding: 8, borderRadius: 8, border: "1px solid #ccc", minWidth: 380, flex: 1 }}
          />
          <button
            onClick={onSelectOutDirFolder}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Select folder...
          </button>
          <button
            onClick={onApplyOutDirOverride}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Apply
          </button>
          <button
            onClick={onClearOutDirOverride}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Clear
          </button>
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
            onClick={async () => {
              await loadRuntimeConfig(true);
              await loadPreflight();
            }}
            disabled={cfgLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            {cfgLoading ? "Reloading..." : "Reload config"}
          </button>
          <button
            onClick={loadPreflight}
            disabled={preflightLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            {preflightLoading ? "Preflight..." : "Run preflight"}
          </button>
        </div>
        {cfgError ? (
          <div style={{ marginTop: 8, color: "#a33", fontSize: 12 }}>
            config status=missing_dependency: {cfgError}
          </div>
        ) : null}
      </div>

      <div style={{ display: "grid", gap: 12, gridTemplateColumns: "1fr 260px" }}>
        <label style={{ display: "grid", gap: 6 }}>
          <span>Paper ID (doi:/pmid:/arxiv:/s2:)</span>
          <input
            value={paperId}
            onChange={(e) => setPaperId(e.target.value)}
            style={{ padding: 10, borderRadius: 8, border: "1px solid #ccc" }}
          />
        </label>

        <label style={{ display: "grid", gap: 6 }}>
          <span>Template</span>
          <select
            value={selectedTemplateId}
            onChange={(e) => setSelectedTemplateId(e.target.value)}
            style={{ padding: 10, borderRadius: 8, border: "1px solid #ccc" }}
            disabled={templatesLoading || templates.length === 0}
          >
            {templates.map((t) => (
              <option key={t.id} value={t.id}>
                {t.title} ({t.id}){t.wired ? "" : " - not wired"}
              </option>
            ))}
          </select>
        </label>
      </div>

      {templatesError ? <div style={{ marginTop: 8, color: "#a33", fontSize: 12 }}>{templatesError}</div> : null}

      <div
        style={{
          border: "1px solid #d2d2d2",
          borderRadius: 10,
          padding: 10,
          marginTop: 10,
          background: "#fafafa",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 6 }}>Template parameters</div>
        <div style={{ fontSize: 12, marginBottom: 6 }}>
          {selectedTemplate ? (
            <>
              <strong>{selectedTemplate.title}</strong>: {selectedTemplate.description}
              {!selectedTemplate.wired ? (
                <span style={{ marginLeft: 8, color: "#a33" }}>not wired</span>
              ) : null}
            </>
          ) : (
            "No template selected"
          )}
        </div>
        <div style={{ display: "grid", gap: 8, gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))" }}>
          {(selectedTemplate?.params ?? []).map((p) => (
            <label key={p.key} style={{ display: "grid", gap: 4 }}>
              <span style={{ fontSize: 12 }}>{p.label}</span>
              <input
                type={p.param_type === "integer" ? "number" : "text"}
                min={p.min ?? undefined}
                max={p.max ?? undefined}
                value={templateParams[p.key] ?? ""}
                onChange={(e) => {
                  const raw = e.target.value;
                  setTemplateParams((prev) => ({
                    ...prev,
                    [p.key]: p.param_type === "integer" ? Number(raw) : raw,
                  }));
                }}
                style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
              />
            </label>
          ))}
        </div>
      </div>

      <div
        style={{
          border: "1px solid #d2d2d2",
          borderRadius: 10,
          padding: 10,
          marginTop: 10,
          background: "#fafafa",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 6 }}>Identifier preview</div>
        <div style={{ fontSize: 12 }}>Detected: <code>{normalizeLoading ? "resolving..." : normalized?.kind ?? "unknown"}</code></div>
        <div style={{ fontSize: 12, marginTop: 4 }}>Canonical: <code>{normalized?.canonical ?? "-"}</code></div>
        {normalizeWarnings.length > 0 ? (
          <div style={{ marginTop: 6, color: "#8a4200", fontSize: 12 }}>
            warnings: {normalizeWarnings.join(" | ")}
          </div>
        ) : null}
        {normalizeErrors.length > 0 ? (
          <div style={{ marginTop: 6, color: "#a33", fontSize: 12 }}>
            errors: {normalizeErrors.join(" | ")}
          </div>
        ) : null}
      </div>

      <div
        style={{
          border: "1px solid #d2d2d2",
          borderRadius: 10,
          padding: 10,
          marginTop: 10,
          background: "#fafafa",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 6 }}>
          Preflight: <span style={{ color: preflight?.ok ? "#1f6f3f" : "#a33" }}>{preflight?.ok ? "OK" : "NG"}</span>
        </div>
        {preflightError ? <div style={{ color: "#a33", fontSize: 12 }}>{preflightError}</div> : null}
        {Array.isArray(preflight?.checks)
          ? preflight.checks.map((item) => (
              <details key={item.name} style={{ marginBottom: 6 }}>
                <summary style={{ fontSize: 12, cursor: "pointer" }}>
                  {item.name}: {item.ok ? "ok" : "ng"}
                </summary>
                <div style={{ fontSize: 12, marginTop: 4 }}>detail: <code>{item.detail}</code></div>
                {!item.ok ? <div style={{ fontSize: 12, color: "#a33" }}>fix_hint: {item.fix_hint}</div> : null}
              </details>
            ))
          : null}
      </div>

      <div
        style={{
          border: "1px solid #d2d2d2",
          borderRadius: 10,
          padding: 10,
          marginTop: 10,
          background: "#fafafa",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Runtime Snapshot</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
          <button
            onClick={onRefreshRuntimeSnapshot}
            disabled={snapshotLoading}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            {snapshotLoading ? "Refreshing..." : "Refresh snapshot"}
          </button>
          <button
            onClick={onOpenPipelineRepoFolder}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Open pipeline repo folder
          </button>
          <button
            onClick={onOpenConfigLocation}
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
          >
            Open config file location
          </button>
          <button
            disabled
            title="No existing command to open out_dir directly"
            style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #aaa", color: "#777" }}
          >
            Open out dir (N/A)
          </button>
        </div>

        <div style={{ fontSize: 12, marginBottom: 4 }}>
          runtime: pipeline_root=<code>{runtimeCfg?.pipeline_root ?? "-"}</code> / out_dir=<code>{runtimeCfg?.out_dir ?? "-"}</code>
        </div>
        <div style={{ fontSize: 12, marginBottom: 4 }}>
          config: path=<code>{runtimeCfg?.config_file_path ?? "-"}</code> / loaded=<code>{runtimeCfg?.config_file_loaded ? "yes" : "no"}</code>
        </div>
        <div style={{ fontSize: 12, marginBottom: 4 }}>
          preflight: pipeline_root=<code>{preflightPipelineRoot?.ok ? "ok" : "ng"}</code> / out_dir=<code>{preflightOutDir?.ok ? "ok" : "ng"}</code> / python=<code>{preflightPython?.ok ? "ok" : "ng"}</code> / markers=<code>{preflightMarkers?.ok ? "ok" : "ng"}</code>
        </div>
        <div style={{ fontSize: 12, marginBottom: 8 }}>
          repo: local=<code>{pipelineRepoStatus?.local_path ?? "-"}</code> / remote=<code>{pipelineRepoStatus?.remote_url ?? "-"}</code> / ref=<code>{pipelineRepoStatus?.git_ref ?? "-"}</code> / last_sync=<code>{pipelineRepoStatus?.last_sync_at ?? "-"}</code> / dirty=<code>{pipelineRepoStatus?.dirty ? "yes" : "no"}</code>
        </div>
        <div style={{ fontSize: 12, marginBottom: 8 }}>
          settings(auto-retry): enabled=<code>{desktopSettings?.auto_retry_enabled ? "yes" : "no"}</code> / max_job=<code>{desktopSettings?.auto_retry_max_per_job ?? "-"}</code> / max_pipeline=<code>{desktopSettings?.auto_retry_max_per_pipeline ?? "-"}</code> / base_delay=<code>{desktopSettings?.auto_retry_base_delay_seconds ?? "-"}</code> / max_delay=<code>{desktopSettings?.auto_retry_max_delay_seconds ?? "-"}</code>
        </div>

        <div style={{ display: "grid", gap: 8, gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
          <div style={{ border: "1px solid #e5e5e5", borderRadius: 6, padding: 6 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>runtimeCfg</div>
            <pre style={{ margin: 0, maxHeight: 180, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 11 }}>
{JSON.stringify(runtimeCfg ?? {}, null, 2)}
            </pre>
          </div>
          <div style={{ border: "1px solid #e5e5e5", borderRadius: 6, padding: 6 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>preflight</div>
            <pre style={{ margin: 0, maxHeight: 180, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 11 }}>
{JSON.stringify(preflight ?? {}, null, 2)}
            </pre>
          </div>
          <div style={{ border: "1px solid #e5e5e5", borderRadius: 6, padding: 6 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>pipelineRepoStatus</div>
            <pre style={{ margin: 0, maxHeight: 180, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 11 }}>
{JSON.stringify(pipelineRepoStatus ?? {}, null, 2)}
            </pre>
          </div>
          <div style={{ border: "1px solid #e5e5e5", borderRadius: 6, padding: 6 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>desktopSettings</div>
            <pre style={{ margin: 0, maxHeight: 180, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 11 }}>
{JSON.stringify(desktopSettings ?? {}, null, 2)}
            </pre>
          </div>
        </div>
      </div>

      <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
        <button
          onClick={onRunTree}
          disabled={runDisabled}
          style={{ padding: "10px 14px", borderRadius: 10, border: "1px solid #333" }}
        >
          {running ? "Running..." : "Run selected template"}
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
      <h3 style={{ marginBottom: 8 }}>Jobs</h3>
      <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
        <button
          onClick={loadJobs}
          disabled={jobsLoading}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          {jobsLoading ? "Refreshing..." : "Refresh jobs"}
        </button>
      </div>
      {jobsError ? <div style={{ color: "#a33", fontSize: 12 }}>{jobsError}</div> : null}
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1.8fr", gap: 12, marginBottom: 12 }}>
        <div style={{ border: "1px solid #ddd", borderRadius: 8, maxHeight: 240, overflow: "auto" }}>
          {jobs.length === 0 ? (
            <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>No jobs.</div>
          ) : (
            jobs.map((j) => (
              <button
                key={j.job_id}
                onClick={() => setSelectedJobId(j.job_id)}
                style={{
                  width: "100%",
                  textAlign: "left",
                  padding: 10,
                  border: "none",
                  borderBottom: "1px solid #eee",
                  background: j.job_id === selectedJobId ? "#eef5ff" : "white",
                  cursor: "pointer",
                }}
              >
                <div style={{ fontSize: 12, fontWeight: 600 }}>{j.job_id}</div>
                <div style={{ fontSize: 11 }}>status={j.status} attempt={j.attempt}</div>
                <div style={{ fontSize: 11, opacity: 0.75 }}>{j.template_id} / {j.canonical_id}</div>
              </button>
            ))
          )}
        </div>
        <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
          <div style={{ fontSize: 12, marginBottom: 4 }}>job_id: <code>{selectedJob?.job_id ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>status: <code>{selectedJob?.status ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>updated_at: <code>{selectedJob?.updated_at ?? "-"}</code></div>
          {selectedJob?.retry_at ? (
            <div style={{ fontSize: 12, marginBottom: 4 }}>
              retry_in_sec: <code>{Math.max(0, Math.floor((Number(selectedJob.retry_at) - Date.now()) / 1000))}</code>
            </div>
          ) : null}
          {selectedJob?.last_error ? (
            <div style={{ fontSize: 12, color: "#a33", marginBottom: 6 }}>error: {selectedJob.last_error}</div>
          ) : null}
          <div style={{ display: "flex", gap: 8 }}>
            <button
              onClick={() => onCancelJob(selectedJob?.job_id)}
              disabled={!selectedJob || (selectedJob.status !== "queued" && selectedJob.status !== "running")}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Cancel
            </button>
            <button
              onClick={() => onRetryJob(selectedJob?.job_id, false)}
              disabled={!selectedJob || (selectedJob.status !== "failed" && selectedJob.status !== "needs_retry")}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Retry
            </button>
            <button
              onClick={() => onRetryJob(selectedJob?.job_id, true)}
              disabled={!selectedJob}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Retry(force)
            </button>
            <button
              onClick={() => onOpenRunFromJob(selectedJob)}
              disabled={!selectedJob?.run_id}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Open run detail
            </button>
          </div>
        </div>
      </div>

      <hr style={{ margin: "18px 0" }} />
      <h3 style={{ marginBottom: 8 }}>Pipelines</h3>
      <div style={{ display: "flex", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
        <button
          onClick={onRunAnalyzePipeline}
          disabled={pipelineStartDisabled}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          Run Pipeline: Analyze Paper
        </button>
        <button
          onClick={onFixRuntimeAfterImport}
          disabled={workspaceFixingRuntime}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          {workspaceFixingRuntime ? "Fixing runtime..." : "Fix runtime"}
        </button>
        <button
          onClick={loadPipelines}
          disabled={pipelinesLoading}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          {pipelinesLoading ? "Refreshing..." : "Refresh pipelines"}
        </button>
      </div>
      {pipelineStartMissingRequirements.length > 0 ? (
        <div style={{ color: "#a33", fontSize: 12, marginBottom: 8 }}>
          Missing requirements: {pipelineStartMissingRequirements.join(" | ")}
        </div>
      ) : null}
      {pipelinesError ? <div style={{ color: "#a33", fontSize: 12, marginBottom: 8 }}>{pipelinesError}</div> : null}

      <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12, marginBottom: 12 }}>
        <div style={{ border: "1px solid #ddd", borderRadius: 8, maxHeight: 260, overflow: "auto" }}>
          {pipelines.length === 0 ? (
            <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>No pipelines.</div>
          ) : (
            pipelines.map((p) => (
              <button
                key={p.pipeline_id}
                onClick={() => setSelectedPipelineId(p.pipeline_id)}
                style={{
                  width: "100%",
                  textAlign: "left",
                  padding: 10,
                  border: "none",
                  borderBottom: "1px solid #eee",
                  background: p.pipeline_id === selectedPipelineId ? "#eef5ff" : "white",
                  cursor: "pointer",
                }}
              >
                <div style={{ fontSize: 12, fontWeight: 600 }}>{p.name}</div>
                <div style={{ fontSize: 11, opacity: 0.85 }}>status={p.status} step={Math.min((p.current_step_index ?? 0) + 1, p.total_steps ?? 0)}/{p.total_steps}</div>
                <div style={{ fontSize: 11, opacity: 0.75 }}>{p.canonical_id}</div>
                <div style={{ fontSize: 11, opacity: 0.75 }}>updated_at={p.updated_at}</div>
              </button>
            ))
          )}
        </div>

        <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
          <div style={{ fontSize: 12, marginBottom: 4 }}>pipeline_id: <code>{selectedPipeline?.pipeline_id ?? selectedPipelineSummary?.pipeline_id ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>canonical_id: <code>{selectedPipeline?.canonical_id ?? selectedPipelineSummary?.canonical_id ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 8 }}>status: <code>{selectedPipeline?.status ?? selectedPipelineSummary?.status ?? "-"}</code></div>
          {pipelineDetailLoading ? <div style={{ fontSize: 12, marginBottom: 6 }}>Loading pipeline...</div> : null}

          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
            <button
              onClick={() => onCancelPipeline(selectedPipelineId)}
              disabled={!selectedPipelineId || !selectedPipeline || selectedPipeline.status !== "running"}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Cancel pipeline
            </button>
            <button
              onClick={() => {
                const q = String(selectedPipeline?.canonical_id ?? "").trim();
                if (!q) return;
                setLibrarySearchQuery(q);
              }}
              disabled={!selectedPipeline?.canonical_id}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Search in Library
            </button>
          </div>

          {(selectedPipeline?.status === "succeeded" && selectedPipeline?.last_primary_viz) ? (
            <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8, marginBottom: 8, background: "#fafafa" }}>
              <div style={{ fontSize: 12, marginBottom: 6 }}>
                final primary_viz: <code>{selectedPipeline.last_primary_viz.kind}:{selectedPipeline.last_primary_viz.name}</code>
              </div>
              <button
                onClick={() => {
                  const steps = Array.isArray(selectedPipeline?.steps) ? selectedPipeline.steps : [];
                  const withRun = [...steps].reverse().find((s) => s?.run_id);
                  if (withRun?.run_id && selectedPipeline?.last_primary_viz?.name) {
                    onOpenNamedArtifactForRun(withRun.run_id, selectedPipeline.last_primary_viz.name);
                  }
                }}
                style={{ padding: "6px 10px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Open final visualization
              </button>
            </div>
          ) : null}

          <details open>
            <summary style={{ fontSize: 12, cursor: "pointer" }}>Steps</summary>
            <div style={{ marginTop: 8, display: "grid", gap: 6 }}>
              {(selectedPipeline?.steps ?? []).map((s) => (
                <div key={s.step_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                  <div style={{ fontSize: 12, fontWeight: 600 }}>{s.step_id} ({s.template_id})</div>
                  <div style={{ fontSize: 11 }}>status=<code>{s.status}</code> job_id=<code>{s.job_id ?? "-"}</code></div>
                  <div style={{ fontSize: 11, opacity: 0.8 }}>started_at={s.started_at ?? "-"} finished_at={s.finished_at ?? "-"}</div>
                  <div style={{ marginTop: 6, display: "flex", gap: 6, flexWrap: "wrap" }}>
                    <button
                      onClick={() => onOpenRunFromLibrary(s.run_id)}
                      disabled={!s.run_id}
                      style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                    >
                      Open run detail
                    </button>
                    <button
                      onClick={() => onRetryPipelineStep(selectedPipeline.pipeline_id, s.step_id, false)}
                      disabled={!(s.status === "failed" || s.status === "needs_retry" || s.status === "canceled")}
                      style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                    >
                      Retry step
                    </button>
                    <button
                      onClick={() => onRetryPipelineStep(selectedPipeline.pipeline_id, s.step_id, true)}
                      disabled={!selectedPipeline?.pipeline_id}
                      style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                    >
                      Retry step (force)
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </details>
        </div>
      </div>

      <hr style={{ margin: "18px 0" }} />

      <h3 style={{ marginBottom: 8 }}>Library</h3>
      <div style={{ display: "flex", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
        <input
          placeholder="Search library (canonical/title/tag/template/run/status)"
          value={librarySearchQuery}
          onChange={(e) => setLibrarySearchQuery(e.target.value)}
          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", minWidth: 320 }}
        />
        <select
          value={libraryFilters.status}
          onChange={(e) => setLibraryFilters((prev) => ({ ...prev, status: e.target.value }))}
          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
        >
          <option value="">status: all</option>
          <option value="succeeded">succeeded</option>
          <option value="failed">failed</option>
          <option value="needs_retry">needs_retry</option>
          <option value="running">running</option>
          <option value="unknown">unknown</option>
        </select>
        <select
          value={libraryFilters.kind}
          onChange={(e) => setLibraryFilters((prev) => ({ ...prev, kind: e.target.value }))}
          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
        >
          <option value="">kind: all</option>
          <option value="doi">doi</option>
          <option value="pmid">pmid</option>
          <option value="arxiv">arxiv</option>
          <option value="s2">s2</option>
          <option value="unknown">unknown</option>
        </select>
        <input
          placeholder="tag"
          value={libraryFilters.tag}
          onChange={(e) => setLibraryFilters((prev) => ({ ...prev, tag: e.target.value }))}
          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", width: 140 }}
        />
        <button
          onClick={() => loadLibraryRows(libraryFilters)}
          disabled={libraryLoading || isLibrarySearchMode}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          {libraryLoading ? "Loading..." : "Refresh list"}
        </button>
        <button
          onClick={onLibraryReindex}
          disabled={libraryLoading}
          style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
        >
          Reindex
        </button>
      </div>

      {libraryError ? <div style={{ color: "#a33", fontSize: 12, marginBottom: 8 }}>{libraryError}</div> : null}
      {libraryStats ? (
        <div style={{ fontSize: 12, marginBottom: 8, opacity: 0.9 }}>
          papers={libraryStats.total_papers} runs={libraryStats.total_runs}
          {isLibrarySearchMode ? ` | search_hits=${librarySearchRows.length}` : ""}
          {libraryReindexInfo ? ` | indexed_at=${libraryReindexInfo.updated_at}` : ""}
        </div>
      ) : null}

      <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12, marginBottom: 12 }}>
        <div style={{ border: "1px solid #ddd", borderRadius: 8, maxHeight: 260, overflow: "auto" }}>
          {librarySearchLoading ? (
            <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>Searching...</div>
          ) : visibleLibraryRows.length === 0 ? (
            <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>
              {isLibrarySearchMode ? "No search results." : "No library rows."}
            </div>
          ) : (
            visibleLibraryRows.map((row) => (
              <button
                key={row.paper_key}
                onClick={() => setSelectedPaperKey(row.paper_key)}
                style={{
                  width: "100%",
                  textAlign: "left",
                  padding: 10,
                  border: "none",
                  borderBottom: "1px solid #eee",
                  background: row.paper_key === selectedPaperKey ? "#eef5ff" : "white",
                  cursor: "pointer",
                }}
              >
                <div style={{ fontSize: 12, fontWeight: 600 }}>{row.canonical_id ?? row.paper_key}</div>
                <div style={{ fontSize: 11, opacity: 0.88 }}>{row.title ?? "(no title)"}</div>
                <div style={{ fontSize: 11 }}>
                  status={row.last_status}
                  {row.score !== undefined ? ` score=${row.score}` : ""}
                  {row.source_kind !== undefined ? ` kind=${row.source_kind ?? "unknown"}` : ""}
                </div>
                <div style={{ fontSize: 11, opacity: 0.8 }}>last_run={row.last_run_id ?? "-"}</div>
                <div style={{ fontSize: 11, opacity: 0.8 }}>updated_at={row.updated_at ?? "-"}</div>
                <div style={{ fontSize: 11, opacity: 0.75 }}>tags={(row.tags ?? []).join(", ") || "-"}</div>
                {Array.isArray(row.highlights) && row.highlights.length > 0 ? (
                  <div style={{ fontSize: 11, opacity: 0.75, marginTop: 3 }}>
                    hit: {(row.highlights ?? []).slice(0, 2).map((h) => `${h.field}:${h.snippet}`).join(" | ")}
                  </div>
                ) : null}
                {renderRowArtifactButtons(row)}
              </button>
            ))
          )}
        </div>

        <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
          <div style={{ fontSize: 12, marginBottom: 4 }}>paper_key: <code>{libraryDetail?.paper_key ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>canonical_id: <code>{libraryDetail?.canonical_id ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>last_status: <code>{libraryDetail?.last_status ?? "-"}</code></div>
          <div style={{ fontSize: 12, marginBottom: 8 }}>last_run_id: <code>{libraryDetail?.last_run_id ?? "-"}</code></div>
          <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
            <input
              value={tagInput}
              onChange={(e) => setTagInput(e.target.value)}
              placeholder="tags comma-separated"
              style={{ flex: 1, padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
            />
            <button
              onClick={onSaveTags}
              disabled={!selectedPaperKey}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Save tags
            </button>
          </div>

          <details open>
            <summary style={{ fontSize: 12, cursor: "pointer" }}>Run history</summary>
            <div style={{ marginTop: 8, display: "grid", gap: 6 }}>
              {(libraryDetail?.runs ?? []).map((r) => (
                <div key={r.run_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                  <div style={{ fontSize: 12 }}>run_id=<code>{r.run_id}</code> status=<code>{r.status}</code></div>
                  <div style={{ fontSize: 11, opacity: 0.8 }}>template={r.template_id ?? "-"}</div>
                  <div style={{ fontSize: 11, opacity: 0.8 }}>updated_at={r.updated_at}</div>
                  <button
                    onClick={() => onOpenRunFromLibrary(r.run_id)}
                    style={{ marginTop: 6, padding: "6px 10px", borderRadius: 6, border: "1px solid #333" }}
                  >
                    Open run detail
                  </button>
                </div>
              ))}
            </div>
          </details>
        </div>
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

          <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8, marginBottom: 8 }}>
            <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Artifact catalog</div>
            {runArtifactCatalogLoading ? <div style={{ fontSize: 12 }}>Loading catalog...</div> : null}
            {runArtifactCatalogError ? <div style={{ color: "#a33", fontSize: 12 }}>{runArtifactCatalogError}</div> : null}
            {!runArtifactCatalogLoading && runArtifactCatalog.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.8 }}>No artifacts found.</div>
            ) : null}
            <div style={{ display: "grid", gap: 6 }}>
              {runArtifactCatalog.map((item) => (
                <div
                  key={`${item.rel_path}:${item.name}`}
                  style={{
                    display: "grid",
                    gridTemplateColumns: "1fr auto",
                    gap: 8,
                    border: "1px solid #f0f0f0",
                    borderRadius: 6,
                    padding: 6,
                  }}
                >
                  <div style={{ fontSize: 11 }}>
                    <div style={{ fontWeight: 600 }}>{item.name}</div>
                    <div style={{ opacity: 0.8 }}>kind={item.kind} size={item.size_bytes ?? "-"}</div>
                    <div style={{ opacity: 0.8 }}>mtime={item.mtime_iso ?? "-"}</div>
                  </div>
                  <button
                    onClick={() => onOpenCatalogArtifact(item)}
                    style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                  >
                    Open
                  </button>
                </div>
              ))}
            </div>
          </div>

          {artifactLoading ? <div style={{ fontSize: 12 }}>Loading artifact...</div> : null}
          {artifactError ? <div style={{ color: "#a33", fontSize: 12 }}>{artifactError}</div> : null}
          {artifactWarnings.length > 0 ? (
            <div style={{ color: "#8a4200", fontSize: 12, marginBottom: 6 }}>
              {artifactWarnings.join(" | ")}
            </div>
          ) : null}
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
              ) : isHtmlArtifact && artifactView.exists ? (
                <div style={{ border: "1px solid #eee", borderRadius: 6, overflow: "hidden" }}>
                  <iframe
                    title="artifact-html-viewer"
                    sandbox="allow-forms"
                    srcDoc={artifactView.content ?? ""}
                    style={{ width: "100%", height: 420, border: "none", background: "#fff" }}
                  />
                </div>
              ) : isGraphJsonArtifact && artifactView.exists ? (
                <div>
                  {graphParseLoading ? <div style={{ fontSize: 12 }}>Parsing graph...</div> : null}
                  {graphParseError ? <div style={{ color: "#a33", fontSize: 12 }}>{graphParseError}</div> : null}

                  <div
                    style={{
                      border: "1px solid #eee",
                      borderRadius: 6,
                      padding: 8,
                      marginBottom: 8,
                      fontSize: 12,
                      background: "#fafafa",
                    }}
                  >
                    <div>nodes={graphParsed?.stats?.nodes_count ?? 0} edges={graphParsed?.stats?.edges_count ?? 0}</div>
                    <div>top_keys={(graphParsed?.stats?.top_level_keys ?? []).join(", ") || "-"}</div>
                    {(graphParsed?.warnings ?? []).length > 0 ? (
                      <div style={{ color: "#8a4200" }}>warnings={(graphParsed?.warnings ?? []).join(" | ")}</div>
                    ) : null}
                    {(graphParsed?.stats?.nodes_count ?? 0) > 10000 ? (
                      <div style={{ color: "#8a4200" }}>
                        large graph mode: neighbors capped to 50 for safety/performance
                      </div>
                    ) : null}
                  </div>

                  {(graphParsed?.stats?.nodes_count ?? 0) > 0 ? (
                    <div style={{ marginBottom: 8, display: "grid", gap: 8 }}>
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <input
                          placeholder="search node id/label"
                          value={graphQuery}
                          onChange={(e) => setGraphQuery(e.target.value)}
                          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", minWidth: 220 }}
                        />
                        <select
                          value={graphTypeFilter}
                          onChange={(e) => setGraphTypeFilter(e.target.value)}
                          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc" }}
                        >
                          {graphTypes.map((t) => (
                            <option key={t} value={t}>{t === "all" ? "type: all" : t}</option>
                          ))}
                        </select>
                        <input
                          placeholder="year from"
                          value={graphYearFrom}
                          onChange={(e) => setGraphYearFrom(e.target.value)}
                          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", width: 100 }}
                        />
                        <input
                          placeholder="year to"
                          value={graphYearTo}
                          onChange={(e) => setGraphYearTo(e.target.value)}
                          style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", width: 90 }}
                        />
                        <label style={{ fontSize: 12, display: "flex", alignItems: "center", gap: 4 }}>
                          <input
                            type="checkbox"
                            checked={graphHasEdgesOnly}
                            onChange={(e) => setGraphHasEdgesOnly(e.target.checked)}
                          />
                          has edges only
                        </label>
                      </div>

                      <div style={{ display: "grid", gridTemplateColumns: "1.2fr 1fr", gap: 8 }}>
                        <div style={{ border: "1px solid #eee", borderRadius: 6, maxHeight: 260, overflow: "auto" }}>
                          {filteredGraphNodes.length === 0 ? (
                            <div style={{ padding: 8, fontSize: 12, opacity: 0.8 }}>No matching nodes.</div>
                          ) : (
                            filteredGraphNodes.slice(0, 1200).map((n) => (
                              <button
                                key={n.id}
                                onClick={() => setSelectedGraphNodeId(n.id)}
                                style={{
                                  width: "100%",
                                  textAlign: "left",
                                  border: "none",
                                  borderBottom: "1px solid #f0f0f0",
                                  padding: 8,
                                  background: n.id === selectedGraphNodeId ? "#eef5ff" : "white",
                                  cursor: "pointer",
                                  fontSize: 11,
                                }}
                              >
                                <div style={{ fontWeight: 600 }}>{n.label ?? n.id}</div>
                                <div>id={n.id} type={n.node_type ?? "-"} year={n.year ?? "-"} degree={graphDegreeMap.get(n.id) ?? 0}</div>
                              </button>
                            ))
                          )}
                        </div>

                        <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                          <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Node detail</div>
                          {selectedGraphNode ? (
                            <>
                              <div style={{ fontSize: 11, marginBottom: 4 }}>id={selectedGraphNode.id}</div>
                              <div style={{ fontSize: 11, marginBottom: 4 }}>label={selectedGraphNode.label ?? "-"}</div>
                              <div style={{ fontSize: 11, marginBottom: 4 }}>type={selectedGraphNode.node_type ?? "-"}</div>
                              <div style={{ fontSize: 11, marginBottom: 4 }}>year={selectedGraphNode.year ?? "-"}</div>
                              <div style={{ fontSize: 11, marginBottom: 6 }}>
                                degree={graphDegreeMap.get(selectedGraphNode.id) ?? 0} neighbors={selectedGraphNeighbors.length}
                              </div>

                              <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 6 }}>
                                <button
                                  onClick={() => {
                                    const q = toCanonicalLibraryQuery(selectedGraphNode);
                                    if (q) setLibrarySearchQuery(q);
                                  }}
                                  style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                                >
                                  Search in Library
                                </button>
                                <button
                                  onClick={() => {
                                    if (artifactView?.run_id) setSelectedRunId(artifactView.run_id);
                                  }}
                                  style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                                >
                                  Open Run Detail
                                </button>
                              </div>

                              <details>
                                <summary style={{ fontSize: 11, cursor: "pointer" }}>Neighbors (max 50)</summary>
                                <div style={{ maxHeight: 120, overflow: "auto", marginTop: 4 }}>
                                  {selectedGraphNeighbors.map((nb) => (
                                    <button
                                      key={nb.id}
                                      onClick={() => setSelectedGraphNodeId(nb.id)}
                                      style={{
                                        width: "100%",
                                        textAlign: "left",
                                        border: "none",
                                        borderBottom: "1px solid #f0f0f0",
                                        padding: "4px 0",
                                        background: "transparent",
                                        cursor: "pointer",
                                        fontSize: 11,
                                      }}
                                    >
                                      {nb.label ?? nb.id} ({nb.id})
                                    </button>
                                  ))}
                                </div>
                              </details>

                              <textarea
                                readOnly
                                value={JSON.stringify(selectedGraphNode.raw ?? {}, null, 2)}
                                style={{
                                  width: "100%",
                                  height: 120,
                                  marginTop: 6,
                                  padding: 8,
                                  borderRadius: 6,
                                  border: "1px solid #ddd",
                                  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                                  fontSize: 11,
                                }}
                              />
                            </>
                          ) : (
                            <div style={{ fontSize: 12, opacity: 0.8 }}>Select a node.</div>
                          )}
                        </div>
                      </div>
                    </div>
                  ) : (
                    <textarea
                      readOnly
                      value={artifactView.content ?? ""}
                      wrap={artifactWrap ? "soft" : "off"}
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
              ) : (
                <div>
                  <div style={{ display: "flex", gap: 8, marginBottom: 6 }}>
                    <button
                      onClick={() => navigator.clipboard?.writeText(artifactView.content ?? "")}
                      style={{ padding: "6px 10px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                    >
                      Copy
                    </button>
                    <button
                      onClick={() => setArtifactWrap((prev) => !prev)}
                      style={{ padding: "6px 10px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                    >
                      Wrap: {artifactWrap ? "on" : "off"}
                    </button>
                  </div>
                  <textarea
                    readOnly
                    value={artifactView.content ?? ""}
                    wrap={artifactWrap ? "soft" : "off"}
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
                </div>
              )}
            </div>
          ) : null}
        </div>
      </div>
      </>
      ) : activeScreen === "runs" ? (
      <div>
        <h3 style={{ marginTop: 0, marginBottom: 8 }}>Run Explorer</h3>
        <div style={{ display: "grid", gap: 8, marginBottom: 10 }}>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              onClick={loadPipelineRuns}
              disabled={pipelineRunsLoading}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              {pipelineRunsLoading ? "Refreshing..." : "Refresh runs"}
            </button>
            <button
              onClick={() => onOpenPipelineRunDir(selectedPipelineRunId)}
              disabled={!selectedPipelineRunId}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Open selected folder
            </button>
          </div>
          <input
            value={pipelineRunQuery}
            onChange={(e) => setPipelineRunQuery(e.target.value)}
            placeholder="Search runs (run_id / canonical_id / template_id)"
            style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid #ccc", maxWidth: 520 }}
          />
        </div>

        {pipelineRunsError ? (
          <div style={{ color: "#a33", fontSize: 12, marginBottom: 8 }}>{pipelineRunsError}</div>
        ) : null}

        <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12 }}>
          <div style={{ border: "1px solid #ddd", borderRadius: 8, maxHeight: 440, overflow: "auto" }}>
            {visiblePipelineRuns.length === 0 ? (
              <div style={{ padding: 10, fontSize: 12, opacity: 0.8 }}>
                {pipelineRuns.length === 0
                  ? "No runs found under pipeline_root/logs/runs."
                  : "No runs matched the search query."}
              </div>
            ) : (
              visiblePipelineRuns.map((row) => (
                <div
                  key={row.run_id}
                  onClick={() => setSelectedPipelineRunId(row.run_id)}
                  style={{
                    borderBottom: "1px solid #eee",
                    padding: 10,
                    background: row.run_id === selectedPipelineRunId ? "#eef5ff" : "white",
                    cursor: "pointer",
                  }}
                >
                  <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}>
                    <div style={{ minWidth: 0 }}>
                      <div style={{ fontSize: 12, fontWeight: 600 }}>{row.run_id}</div>
                      <div style={{ fontSize: 11, opacity: 0.82 }}>canonical_id={row.canonical_id ?? "-"}</div>
                      <div style={{ fontSize: 11, opacity: 0.82 }}>template_id={row.template_id ?? "-"}</div>
                      <div style={{ fontSize: 11, opacity: 0.82 }}>created_at={row.created_at || "-"}</div>
                      <div style={{ marginTop: 4 }}>
                        <span
                          style={{
                            display: "inline-block",
                            padding: "2px 6px",
                            borderRadius: 999,
                            border: `1px solid ${pipelineRunStatusColor(row.status)}`,
                            color: pipelineRunStatusColor(row.status),
                            fontSize: 11,
                            fontWeight: 600,
                            background: "#fff",
                          }}
                        >
                          {row.status ?? "unknown"}
                        </span>
                      </div>
                    </div>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onOpenPipelineRunDir(row.run_id);
                      }}
                      style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11, height: "fit-content" }}
                    >
                      Open Folder
                    </button>
                  </div>
                </div>
              ))
            )}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
            <div style={{ fontSize: 12, marginBottom: 6 }}>run_id: <code>{selectedPipelineRun?.run_id ?? "-"}</code></div>
            <div style={{ fontSize: 12, marginBottom: 6 }}>canonical_id: <code>{selectedPipelineRun?.canonical_id ?? "-"}</code></div>
            <div style={{ fontSize: 12, marginBottom: 6 }}>template_id: <code>{selectedPipelineRun?.template_id ?? "-"}</code></div>
            <div style={{ fontSize: 12, marginBottom: 6 }}>created_at: <code>{selectedPipelineRun?.created_at ?? "-"}</code></div>
            <div style={{ fontSize: 12, marginBottom: 10 }}>status: <code>{selectedPipelineRun?.status ?? "unknown"}</code></div>

            <div style={{ marginBottom: 8 }}>
              <select
                value={pipelineRunTab}
                onChange={(e) => setPipelineRunTab(e.target.value)}
                style={{ padding: 8, borderRadius: 6, border: "1px solid #ccc", minWidth: 320 }}
              >
                {PIPELINE_RUN_ARTIFACT_OPTIONS.map((opt) => (
                  <option key={opt.kind} value={opt.kind}>{opt.label}</option>
                ))}
              </select>
            </div>

            {pipelineRunTextLoading ? <div style={{ fontSize: 12, marginBottom: 8 }}>Loading preview...</div> : null}
            {pipelineRunTextError ? (
              <div style={{ color: "#a33", fontSize: 12, marginBottom: 8 }}>{pipelineRunTextError}</div>
            ) : null}

            <pre
              style={{
                margin: 0,
                minHeight: 320,
                maxHeight: 520,
                overflow: "auto",
                padding: 10,
                borderRadius: 8,
                border: "1px solid #eee",
                background: "#fafafa",
                fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                fontSize: 12,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {pipelineRunText}
            </pre>
          </div>
        </div>
      </div>
      ) : (
        <div>
          <div style={{ display: "flex", gap: 8, marginBottom: 10, flexWrap: "wrap" }}>
            <button
              onClick={async () => {
                await loadPipelines();
                await loadJobs();
                await loadRuns();
                await loadSettings();
                await loadPipelineRepoStatus();
                await loadDiagnostics();
              }}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Refresh Ops
            </button>
            <button
              onClick={onOpenAuditLog}
              style={{ padding: "8px 12px", borderRadius: 8, border: "1px solid #333" }}
            >
              Open audit log
            </button>
            <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
              <input
                type="checkbox"
                checked={opsNeedsAttentionOnly}
                onChange={(e) => setOpsNeedsAttentionOnly(e.target.checked)}
              />
              Needs Attention only (failed / needs_retry)
            </label>
            <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
              <input
                type="checkbox"
                checked={opsAutoRetryPendingOnly}
                onChange={(e) => setOpsAutoRetryPendingOnly(e.target.checked)}
              />
              Auto-retry pending only
            </label>
            <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
              <input
                type="checkbox"
                checked={desktopSettings?.auto_retry_enabled === true}
                disabled={!desktopSettings || settingsLoading}
                onChange={(e) => updateAutoRetryEnabled(e.target.checked)}
              />
              Auto-retry enabled
            </label>
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12, fontSize: 12 }}>
            <div style={{ fontWeight: 600, marginBottom: 6 }}>Auto-retry policy</div>
            {desktopSettings ? (
              <div>
                max/job={desktopSettings.auto_retry_max_per_job} max/pipeline={desktopSettings.auto_retry_max_per_pipeline} base_delay={desktopSettings.auto_retry_base_delay_seconds}s max_delay={desktopSettings.auto_retry_max_delay_seconds}s
              </div>
            ) : (
              <div style={{ opacity: 0.8 }}>No settings loaded.</div>
            )}
            <div style={{ marginTop: 4, opacity: 0.8 }}>
              tick={tickResult?.reason ?? "-"} acted={tickResult?.acted ? "yes" : "no"}
            </div>
            {settingsError ? <div style={{ marginTop: 4, color: "#c00" }}>{settingsError}</div> : null}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12, fontSize: 12 }}>
            <div style={{ fontWeight: 600, marginBottom: 6 }}>Pipeline Repo</div>
            <div>remote: <code>{(pipelineRepoStatus?.remote_url ?? pipelineRepoRemoteUrl) || "-"}</code></div>
            <div>local_path: <code>{(pipelineRepoStatus?.local_path ?? pipelineRepoLocalPath) || "-"}</code></div>
            <div>ref: <code>{(pipelineRepoStatus?.git_ref ?? pipelineRepoRef) || "-"}</code></div>
            <div>commit: <code>{pipelineRepoStatus?.head_commit ?? "-"}</code></div>
            <div>last_sync: <code>{pipelineRepoStatus?.last_sync_at ?? "-"}</code></div>
            <div style={{ marginTop: 6, display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button onClick={onUpdatePipelineRepo} disabled={pipelineRepoBusy} style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}>
                Update
              </button>
              <button onClick={onValidatePipelineRepo} disabled={pipelineRepoBusy} style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}>
                Validate
              </button>
              <button onClick={onOpenPipelineRepoFolder} disabled={pipelineRepoBusy} style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}>
                Open folder
              </button>
              <button onClick={loadPipelineRepoStatus} disabled={pipelineRepoBusy} style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}>
                Reload status
              </button>
            </div>
            {pipelineRepoError ? <div style={{ marginTop: 6, color: "#c00" }}>{pipelineRepoError}</div> : null}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12 }}>
            <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8, flexWrap: "wrap" }}>
              <div style={{ fontWeight: 600 }}>Diagnostics</div>
              <button
                onClick={onCollectDiagnostics}
                disabled={collectingDiagnostics || diagnosticsOneClickBusy}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                {collectingDiagnostics ? "Collecting..." : "Collect Diagnostics"}
              </button>
              <button
                onClick={onGenerateDiagnosticsZipOneClick}
                disabled={collectingDiagnostics || diagnosticsOneClickBusy}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                {diagnosticsOneClickBusy
                  ? "Generating diagnostics zip..."
                  : "Generate diagnostics zip (for sharing)"}
              </button>
              <button
                onClick={onOpenLatestDiagnosticFolder}
                disabled={!latestDiagnosticId || collectingDiagnostics || diagnosticsOneClickBusy}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Open latest diagnostics folder
              </button>
              <button
                onClick={onOpenLatestDiagnosticZip}
                disabled={!latestDiagnosticId || collectingDiagnostics || diagnosticsOneClickBusy}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Open latest diagnostics zip
              </button>
              <button
                onClick={() => onCopyZipPath(latestDiagnosticZipPath || latestDiagnosticFolderPath)}
                disabled={!(latestDiagnosticZipPath || latestDiagnosticFolderPath)}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Copy path
              </button>
              <button
                onClick={loadDiagnostics}
                disabled={diagnosticsLoading}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Reload list
              </button>
            </div>
            {diagnosticsOneClickMessage ? (
              <div style={{ color: "#1f6f3f", fontSize: 12, marginBottom: 6 }}>
                {diagnosticsOneClickMessage}
                {latestDiagnosticId ? ` / diag_id=${latestDiagnosticId}` : ""}
              </div>
            ) : null}
            {latestDiagnosticZipPath || latestDiagnosticFolderPath ? (
              <div style={{ fontSize: 11, marginBottom: 6 }}>
                <div>latest_zip: <code>{latestDiagnosticZipPath || "-"}</code></div>
                <div>latest_folder: <code>{latestDiagnosticFolderPath || "-"}</code></div>
              </div>
            ) : null}
            {diagnosticsError ? <div style={{ color: "#c00", fontSize: 12, marginBottom: 6 }}>{diagnosticsError}</div> : null}
            {diagnosticsRows.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.8 }}>No diagnostic bundles.</div>
            ) : (
              <div style={{ display: "grid", gap: 6, marginBottom: 8 }}>
                {diagnosticsRows.map((d) => (
                  <div key={d.diag_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                    <div style={{ fontSize: 12, fontWeight: 600 }}>{d.diag_id}</div>
                    <div style={{ fontSize: 11 }}>created_at={d.created_at} size={d.size_bytes} bytes</div>
                    <div style={{ fontSize: 11 }}>bundle.zip={d.zip_path || "(none)"}</div>
                    <div style={{ display: "flex", gap: 6, marginTop: 6, flexWrap: "wrap" }}>
                      <button
                        onClick={() => setSelectedDiagId(d.diag_id)}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        View report
                      </button>
                      <button
                        onClick={() => onOpenDiagnosticFolder(d.diag_id)}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Open folder
                      </button>
                      <button
                        onClick={() => onOpenDiagnosticZip(d.diag_id)}
                        disabled={!d.zip_path}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Open zip
                      </button>
                      <button
                        onClick={() => onCopyZipPath(d.zip_path)}
                        disabled={!d.zip_path}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Copy zip path
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
            {selectedDiagId ? (
              <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Report: {selectedDiagId}</div>
                {diagReportLoading ? (
                  <div style={{ fontSize: 12, opacity: 0.8 }}>Loading report...</div>
                ) : diagReportError ? (
                  <div style={{ color: "#c00", fontSize: 12 }}>{diagReportError}</div>
                ) : (
                  <div
                    style={{ fontSize: 12, lineHeight: 1.5 }}
                    dangerouslySetInnerHTML={{ __html: renderMarkdownToHtml(diagReport || "") }}
                  />
                )}
              </div>
            ) : null}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12 }}>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>Pipelines</div>
            {opsPipelineRows.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.8 }}>No pipelines.</div>
            ) : (
              <div style={{ display: "grid", gap: 6 }}>
                {opsPipelineRows.map((p) => {
                  return (
                    <div key={p.pipeline_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                      <div style={{ fontSize: 12, fontWeight: 600 }}>{p.pipeline_id}</div>
                      <div style={{ fontSize: 11 }}>{p.name} / {p.canonical_id}</div>
                      <div style={{ fontSize: 11 }}>status={p.status} current_step={Math.min((p.current_step_index ?? 0) + 1, p.total_steps ?? 0)}/{p.total_steps} updated_at={p.updated_at}</div>
                      <div style={{ display: "flex", gap: 6, marginTop: 6, flexWrap: "wrap" }}>
                        <button
                          onClick={async () => {
                            setSelectedPipelineId(p.pipeline_id);
                            await loadPipelineDetail(p.pipeline_id);
                            setActiveScreen("main");
                          }}
                          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                        >
                          Open pipeline
                        </button>
                        <button
                          onClick={() => onCancelPipeline(p.pipeline_id)}
                          disabled={p.status !== "running"}
                          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                        >
                          Cancel
                        </button>
                        <button
                          onClick={async () => {
                            const detail = await invoke("get_pipeline", { pipelineId: p.pipeline_id });
                            await onResumePipeline(detail);
                          }}
                          disabled={p.status !== "needs_retry"}
                          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                        >
                          Resume
                        </button>
                        <button
                          onClick={async () => {
                            const detail = await invoke("get_pipeline", { pipelineId: p.pipeline_id });
                            const idx = Number(detail?.current_step_index ?? 0);
                            const step = (Array.isArray(detail?.steps) ? detail.steps : [])[idx] ?? null;
                            if (step?.step_id) {
                              await onRetryPipelineStep(p.pipeline_id, step.step_id, true);
                            }
                          }}
                          disabled={p.status !== "needs_retry" && p.status !== "failed"}
                          style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                        >
                          Retry current step (force)
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12 }}>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>Workspace</div>
            <div style={{ display: "flex", gap: 8, marginBottom: 8, flexWrap: "wrap", alignItems: "center" }}>
              <button
                onClick={onExportWorkspace}
                disabled={workspaceExporting}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                {workspaceExporting ? "Exporting..." : "Export workspace"}
              </button>
              <input
                value={workspaceImportZipPath}
                onChange={(e) => setWorkspaceImportZipPath(e.target.value)}
                placeholder="workspace.zip path"
                style={{ minWidth: 340, padding: 6, borderRadius: 6, border: "1px solid #ccc", fontSize: 12 }}
              />
              <select
                value={workspaceImportMode}
                onChange={(e) => setWorkspaceImportMode(e.target.value)}
                style={{ padding: 6, borderRadius: 6, border: "1px solid #ccc", fontSize: 12 }}
              >
                <option value="keep_current">Keep current (recommended)</option>
                <option value="replace">Replace with imported</option>
                <option value="merge">Merge (prefer imported)</option>
              </select>
              <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
                <input
                  type="checkbox"
                  checked={workspaceImportDryRun}
                  onChange={(e) => setWorkspaceImportDryRun(e.target.checked)}
                />
                dry-run
              </label>
              <button
                onClick={onImportWorkspace}
                disabled={workspaceImporting}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                {workspaceImporting ? "Importing..." : "Import workspace"}
              </button>
              <button
                onClick={loadWorkspaceHistory}
                disabled={workspaceLoading}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                Reload workspace history
              </button>
            </div>
            {workspaceError ? <div style={{ color: "#c00", fontSize: 12, marginBottom: 6 }}>{workspaceError}</div> : null}
            <div
              style={{
                display: "flex",
                gap: 8,
                marginBottom: 8,
                flexWrap: "wrap",
                alignItems: "center",
                border: "1px solid #eee",
                borderRadius: 6,
                padding: 8,
                background: "#fafafa",
              }}
            >
              <span style={{ fontSize: 12, fontWeight: 600 }}>After import</span>
              <button
                onClick={onFixRuntimeAfterImport}
                disabled={workspaceFixingRuntime || workspaceImporting}
                style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
              >
                {workspaceFixingRuntime ? "Fixing runtime..." : "Fix runtime"}
              </button>
              {workspaceLastImportId ? (
                <span style={{ fontSize: 11, opacity: 0.8 }}>
                  last_import_id=<code>{workspaceLastImportId}</code>
                </span>
              ) : null}
              {workspaceFixRuntimeMessage ? (
                <span
                  style={{
                    fontSize: 12,
                    color: workspaceFixRuntimeMessage.toLowerCase().includes("failed") ? "#a33" : "#1f6f3f",
                  }}
                >
                  {workspaceFixRuntimeMessage}
                </span>
              ) : null}
            </div>

            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
              <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Exports</div>
                {workspaceExports.length === 0 ? (
                  <div style={{ fontSize: 12, opacity: 0.8 }}>No exports.</div>
                ) : (
                  <div style={{ display: "grid", gap: 6 }}>
                    {workspaceExports.map((item) => (
                      <div key={item.id} style={{ border: "1px solid #f0f0f0", borderRadius: 6, padding: 6 }}>
                        <div style={{ fontSize: 11, fontWeight: 600 }}>{item.id}</div>
                        <div style={{ fontSize: 11 }}>zip={item.zip_path || "(none)"}</div>
                        <div style={{ display: "flex", gap: 6, marginTop: 4, flexWrap: "wrap" }}>
                          <button
                            onClick={() => onOpenWorkspaceExportFolder(item.id)}
                            style={{ padding: "3px 6px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                          >
                            Open folder
                          </button>
                          <button
                            onClick={() => onOpenWorkspaceExportZip(item.id)}
                            disabled={!item.zip_path}
                            style={{ padding: "3px 6px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                          >
                            Open zip
                          </button>
                          <button
                            onClick={() => onLoadWorkspaceReport("export", item.id)}
                            style={{ padding: "3px 6px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                          >
                            View report
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              <div style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>Imports</div>
                {workspaceImports.length === 0 ? (
                  <div style={{ fontSize: 12, opacity: 0.8 }}>No imports.</div>
                ) : (
                  <div style={{ display: "grid", gap: 6 }}>
                    {workspaceImports.map((item) => (
                      <div key={item.id} style={{ border: "1px solid #f0f0f0", borderRadius: 6, padding: 6 }}>
                        <div style={{ fontSize: 11, fontWeight: 600 }}>{item.id}</div>
                        <div style={{ fontSize: 11 }}>created_at={item.created_at}</div>
                        <div style={{ display: "flex", gap: 6, marginTop: 4, flexWrap: "wrap" }}>
                          <button
                            onClick={() => onOpenWorkspaceImportFolder(item.id)}
                            style={{ padding: "3px 6px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                          >
                            Open folder
                          </button>
                          <button
                            onClick={() => onLoadWorkspaceReport("import", item.id)}
                            style={{ padding: "3px 6px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                          >
                            View report
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>

            {selectedWorkspaceReport?.id ? (
              <div style={{ marginTop: 8, border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>
                  Workspace report ({selectedWorkspaceReport.scope}): {selectedWorkspaceReport.id}
                </div>
                {workspaceReportLoading ? (
                  <div style={{ fontSize: 12, opacity: 0.8 }}>Loading report...</div>
                ) : workspaceReportError ? (
                  <div style={{ fontSize: 12, color: "#c00" }}>{workspaceReportError}</div>
                ) : (
                  <div
                    style={{ fontSize: 12, lineHeight: 1.5 }}
                    dangerouslySetInnerHTML={{ __html: renderMarkdownToHtml(workspaceReport || "") }}
                  />
                )}
              </div>
            ) : null}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10, marginBottom: 12 }}>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>Jobs</div>
            {opsJobRows.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.8 }}>No jobs.</div>
            ) : (
              <div style={{ display: "grid", gap: 6 }}>
                {opsJobRows.map((j) => (
                  <div key={j.job_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                    <div style={{ fontSize: 12, fontWeight: 600 }}>{j.job_id}</div>
                    <div style={{ fontSize: 11 }}>{j.template_id} / {j.canonical_id}</div>
                    <div style={{ fontSize: 11 }}>status={j.status} attempt={j.attempt} updated_at={j.updated_at}</div>
                    <div style={{ fontSize: 11 }}>next_retry_at={j.retry_at || "-"} auto_retry_attempt_count={j.auto_retry_attempt_count ?? 0}</div>
                    <div style={{ display: "flex", gap: 6, marginTop: 6, flexWrap: "wrap" }}>
                      <button
                        onClick={() => onCancelJob(j.job_id)}
                        disabled={j.status !== "queued" && j.status !== "running"}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Cancel
                      </button>
                      <button
                        onClick={() => onRetryJob(j.job_id, false)}
                        disabled={j.status !== "failed" && j.status !== "needs_retry"}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Retry
                      </button>
                      <button
                        onClick={() => onOpenRunFromLibrary(j.run_id)}
                        disabled={!j.run_id}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Open run detail
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div style={{ border: "1px solid #ddd", borderRadius: 8, padding: 10 }}>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>Runs</div>
            {opsRunRows.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.8 }}>No runs.</div>
            ) : (
              <div style={{ display: "grid", gap: 6 }}>
                {opsRunRows.map((r) => (
                  <div key={r.run_id} style={{ border: "1px solid #eee", borderRadius: 6, padding: 8 }}>
                    <div style={{ fontSize: 12, fontWeight: 600 }}>{r.run_id}</div>
                    <div style={{ fontSize: 11 }}>status={r.status} paper_id={r.paper_id}</div>
                    <div style={{ fontSize: 11 }}>mtime={new Date(r.mtime_epoch_ms ?? r.created_at_epoch_ms ?? 0).toLocaleString()}</div>
                    <div style={{ display: "flex", gap: 6, marginTop: 6, flexWrap: "wrap" }}>
                      <button
                        onClick={() => {
                          if (r.primary_viz?.name) {
                            onOpenNamedArtifactForRun(r.run_id, r.primary_viz.name);
                            setActiveScreen("main");
                          }
                        }}
                        disabled={!r.primary_viz?.name}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        View primary_viz
                      </button>
                      <button
                        onClick={() => {
                          onOpenRunFromLibrary(r.run_id);
                          setActiveScreen("main");
                        }}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Open run detail
                      </button>
                      <button
                        onClick={() => onOpenRunLogs(r.run_id)}
                        style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid #333", fontSize: 11 }}
                      >
                        Open logs
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
