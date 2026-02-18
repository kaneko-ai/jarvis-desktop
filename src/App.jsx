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
  const [runArtifactCatalog, setRunArtifactCatalog] = useState([]);
  const [runArtifactCatalogLoading, setRunArtifactCatalogLoading] = useState(false);
  const [runArtifactCatalogError, setRunArtifactCatalogError] = useState("");
  const [artifactCatalogByRun, setArtifactCatalogByRun] = useState({});
  const [jobs, setJobs] = useState([]);
  const [jobsLoading, setJobsLoading] = useState(false);
  const [jobsError, setJobsError] = useState("");
  const [selectedJobId, setSelectedJobId] = useState("");
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
    loadJobs();
    loadLibraryRows();
    loadLibraryStats();
  }, []);

  useEffect(() => {
    const timer = setInterval(() => {
      loadJobs();
    }, 1500);
    return () => clearInterval(timer);
  }, []);

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

  const normalizeErrors = Array.isArray(normalized?.errors) ? normalized.errors : [];
  const normalizeWarnings = Array.isArray(normalized?.warnings) ? normalized.warnings : [];
  const canRunByNormalization = normalizeErrors.length === 0 && !!normalized?.canonical;
  const canRunByPreflight = preflight?.ok === true;
  const canRunByTemplate = !!selectedTemplate && selectedTemplate.wired === true;
  const runDisabled = running || !canRunByNormalization || !canRunByPreflight || !canRunByTemplate;

  const showRetryButton = status === "needs_retry" && !!lastRunRequest;
  const selectedRun = runs.find((r) => r.run_id === selectedRunId) ?? null;
  const artifactIsMissing = artifactView && artifactView.exists === false;
  const createdAtText = selectedRun?.created_at_epoch_ms
    ? new Date(selectedRun.created_at_epoch_ms).toLocaleString()
    : "-";
  const selectedJob = jobs.find((j) => j.job_id === selectedJobId) ?? null;
  const isLibrarySearchMode = String(librarySearchQuery ?? "").trim() !== "";
  const visibleLibraryRows = isLibrarySearchMode ? librarySearchRows : libraryRows;
  const artifactKind = artifactView?.kind ?? "";
  const isHtmlArtifact = artifactKind === "html";
  const isGraphJsonArtifact = artifactKind === "graph_json";
  const graphSummary = useMemo(() => {
    if (!isGraphJsonArtifact || !artifactView?.content) {
      return null;
    }
    try {
      const parsed = JSON.parse(artifactView.content);
      const topKeys = parsed && typeof parsed === "object" && !Array.isArray(parsed)
        ? Object.keys(parsed)
        : [];
      const nodesCount = Array.isArray(parsed?.nodes) ? parsed.nodes.length : null;
      const edgesCount = Array.isArray(parsed?.edges) ? parsed.edges.length : null;
      return { topKeys, nodesCount, edgesCount };
    } catch {
      return { topKeys: [], nodesCount: null, edgesCount: null };
    }
  }, [isGraphJsonArtifact, artifactView?.content]);

  function renderRowArtifactButtons(row) {
    const runIdFromRow = row?.last_run_id;
    const catalog = runIdFromRow ? artifactCatalogByRun[runIdFromRow] : null;
    const loaded = !!catalog?.loaded;
    const hasName = (name) => !!catalog?.names?.[name];
    const hasLogs = hasName("stdout.log") || hasName("stderr.log");
    const logTarget = hasName("stdout.log") ? "stdout.log" : "stderr.log";
    const viewTarget = (catalog?.items ?? []).find((i) => i.kind === "html")
      || (catalog?.items ?? []).find((i) => i.kind === "graph_json");

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
                    <div>nodes={graphSummary?.nodesCount ?? "-"} edges={graphSummary?.edgesCount ?? "-"}</div>
                    <div>top_keys={(graphSummary?.topKeys ?? []).join(", ") || "-"}</div>
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
    </div>
  );
}
