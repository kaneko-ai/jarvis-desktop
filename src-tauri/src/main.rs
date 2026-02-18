#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io::Write};
use chrono::{DateTime, Utc};

const MAX_ARTIFACT_READ_BYTES: u64 = 3 * 1024 * 1024;
const SCHEMA_VERSION: u32 = 1;
const DIAG_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const DIAG_MAX_TOTAL_BYTES: u64 = 30 * 1024 * 1024;
const DIAG_AUDIT_TAIL_LINES: usize = 200;
const DIAG_MAX_RECENT_ITEMS: usize = 20;

#[derive(Serialize)]
struct RunResult {
    ok: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
    run_id: String,
    run_dir: String,
    status: String, // ok / needs_retry / error / missing_dependency
    message: String,
    retry_after_sec: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(non_snake_case)]
struct DesktopConfigFile {
    JARVIS_PIPELINE_ROOT: Option<String>,
    JARVIS_PIPELINE_OUT_DIR: Option<String>,
    S2_API_KEY: Option<String>,
    S2_MIN_INTERVAL_MS: Option<u64>,
    S2_MAX_RETRIES: Option<u32>,
    S2_BACKOFF_BASE_SEC: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct EnvConfig {
    pipeline_root: Option<String>,
    pipeline_out_dir: Option<String>,
    s2_api_key: Option<String>,
    s2_min_interval_ms: Option<u64>,
    s2_max_retries: Option<u32>,
    s2_backoff_base_sec: Option<f64>,
}

#[derive(Debug, Clone)]
struct RuntimeConfig {
    config_file_path: PathBuf,
    config_file_loaded: bool,
    pipeline_root: PathBuf,
    out_base_dir: PathBuf,
    s2_api_key: Option<String>,
    s2_min_interval_ms: Option<u64>,
    s2_max_retries: Option<u32>,
    s2_backoff_base_sec: Option<f64>,
}

#[derive(Serialize)]
struct RuntimeConfigView {
    ok: bool,
    status: String,
    message: String,
    config_file_path: String,
    config_file_loaded: bool,
    pipeline_root: String,
    out_dir: String,
    s2_api_key_set: bool,
    s2_min_interval_ms: Option<u64>,
    s2_max_retries: Option<u32>,
    s2_backoff_base_sec: Option<f64>,
}

#[derive(Serialize)]
struct RunListItem {
    run_id: String,
    status: String,
    created_at_epoch_ms: u64,
    mtime_epoch_ms: u64,
    paper_id: String,
    primary_viz: Option<PrimaryVizRef>,
    run_dir: String,
}

#[derive(Deserialize, Default)]
struct RunListFilter {
    query: Option<String>,
    status: Option<String>,
}

#[derive(Serialize)]
struct RunArtifactView {
    run_id: String,
    artifact: String,
    path: String,
    exists: bool,
    content: String,
    parse_status: String,
}

#[derive(Serialize, Clone)]
struct ArtifactItem {
    name: String,
    rel_path: String,
    kind: String,
    size_bytes: Option<u64>,
    mtime_iso: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
struct PrimaryVizRef {
    name: String,
    kind: String,
}

#[derive(Serialize)]
struct NamedArtifactView {
    kind: String,
    content: String,
    truncated: bool,
    warnings: Vec<String>,
}

#[derive(Clone)]
struct ArtifactSpec {
    name: &'static str,
    rel_path: &'static str,
    legacy_key: &'static str,
}

#[derive(Serialize, Clone)]
struct GraphNodeNormalized {
    id: String,
    label: Option<String>,
    node_type: Option<String>,
    year: Option<i32>,
    score: Option<f64>,
    raw: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct GraphEdgeNormalized {
    source: String,
    target: String,
    edge_type: Option<String>,
    weight: Option<f64>,
    raw: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct GraphParseStats {
    nodes_count: usize,
    edges_count: usize,
    top_level_keys: Vec<String>,
}

#[derive(Serialize, Clone)]
struct GraphParseResult {
    nodes: Vec<GraphNodeNormalized>,
    edges: Vec<GraphEdgeNormalized>,
    stats: GraphParseStats,
    warnings: Vec<String>,
}

#[derive(Serialize, Clone)]
struct NormalizedIdentifier {
    kind: String,
    canonical: String,
    display: String,
    warnings: Vec<String>,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct PreflightCheckItem {
    name: String,
    ok: bool,
    detail: String,
    fix_hint: String,
}

#[derive(Serialize)]
struct PreflightResult {
    ok: bool,
    checks: Vec<PreflightCheckItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    NeedsRetry,
    Canceled,
}

#[derive(Serialize, Deserialize, Clone)]
struct JobRecord {
    job_id: String,
    template_id: String,
    canonical_id: String,
    params: serde_json::Value,
    status: JobStatus,
    attempt: u32,
    created_at: String,
    updated_at: String,
    run_id: Option<String>,
    last_error: Option<String>,
    retry_after_seconds: Option<f64>,
    retry_at: Option<String>,
    #[serde(default)]
    auto_retry_attempt_count: u32,
}

#[derive(Default)]
struct JobRuntimeState {
    jobs: Vec<JobRecord>,
    running_job_id: Option<String>,
    running_pid: Option<u32>,
    cancel_requested: HashSet<String>,
}

#[derive(Serialize, Deserialize)]
struct JobFilePayload {
    schema_version: u32,
    jobs: Vec<JobRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PipelineStepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    NeedsRetry,
    Canceled,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PipelineStatus {
    Running,
    Succeeded,
    Failed,
    NeedsRetry,
    Canceled,
}

#[derive(Serialize, Deserialize, Clone)]
struct PipelineStep {
    step_id: String,
    template_id: String,
    params: serde_json::Value,
    job_id: Option<String>,
    status: PipelineStepStatus,
    run_id: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct PipelineRecord {
    pipeline_id: String,
    canonical_id: String,
    name: String,
    created_at: String,
    updated_at: String,
    steps: Vec<PipelineStep>,
    current_step_index: usize,
    status: PipelineStatus,
    last_primary_viz: Option<PrimaryVizRef>,
    #[serde(default)]
    auto_retry_attempt_count: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct DesktopSettings {
    auto_retry_enabled: bool,
    auto_retry_max_per_job: u32,
    auto_retry_max_per_pipeline: u32,
    auto_retry_max_delay_seconds: u64,
    auto_retry_base_delay_seconds: u64,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            auto_retry_enabled: false,
            auto_retry_max_per_job: 2,
            auto_retry_max_per_pipeline: 3,
            auto_retry_max_delay_seconds: 3600,
            auto_retry_base_delay_seconds: 30,
        }
    }
}

#[derive(Serialize)]
struct AuditAutoRetryEntry {
    ts: String,
    kind: String,
    job_id: String,
    pipeline_id: Option<String>,
    reason: String,
    next_retry_at: Option<String>,
    attempt: u32,
}

#[derive(Serialize)]
struct AutoRetryTickResult {
    acted: bool,
    job_id: Option<String>,
    pipeline_id: Option<String>,
    reason: String,
}

#[derive(Serialize, Deserialize, Default)]
struct PipelineFilePayload {
    schema_version: u32,
    pipelines: Vec<PipelineRecord>,
}

#[derive(Serialize, Deserialize)]
struct SettingsFilePayload {
    schema_version: u32,
    settings: DesktopSettings,
}

#[derive(Deserialize, Default)]
struct DiagnosticsCollectOptions {
    include_audit: Option<bool>,
    include_recent_runs: Option<bool>,
    include_zip: Option<bool>,
}

#[derive(Serialize)]
struct DiagnosticsCollectResult {
    diag_id: String,
    diag_dir: String,
    report_path: String,
    zip_path: Option<String>,
}

#[derive(Serialize)]
struct DiagnosticListItem {
    diag_id: String,
    created_at: String,
    size_bytes: u64,
    zip_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct DiagnosticFileEntry {
    rel_path: String,
    source_path: String,
    included: bool,
    size_bytes: u64,
    reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct DiagnosticJobSummary {
    job_id: String,
    status: String,
    attempt: u32,
    updated_at: String,
    retry_at: Option<String>,
    auto_retry_attempt_count: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct DiagnosticPipelineSummary {
    pipeline_id: String,
    status: String,
    current_step_index: usize,
    total_steps: usize,
    updated_at: String,
    canonical_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct DiagnosticRunSummary {
    run_id: String,
    status: String,
    mtime_epoch_ms: u64,
    canonical_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct DiagnosticSummary {
    diag_id: String,
    created_at: String,
    app_version: Option<String>,
    os: String,
    arch: String,
    out_dir: String,
    pipeline_root: String,
    python_path: String,
    include_audit: bool,
    include_recent_runs: bool,
    include_zip: bool,
    smoke_script_path: String,
    gate_commands: Vec<String>,
    jobs: Vec<DiagnosticJobSummary>,
    pipelines: Vec<DiagnosticPipelineSummary>,
    runs: Vec<DiagnosticRunSummary>,
    audit_tail: Vec<String>,
    files: Vec<DiagnosticFileEntry>,
    total_included_bytes: u64,
    max_file_bytes: u64,
    max_total_bytes: u64,
}

#[derive(Deserialize, Clone)]
struct PipelineCreateStepInput {
    template_id: String,
    params: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct PipelineListFilter {
    query: Option<String>,
    status: Option<String>,
}

#[derive(Serialize, Clone)]
struct PipelineSummary {
    pipeline_id: String,
    canonical_id: String,
    name: String,
    status: PipelineStatus,
    current_step_index: usize,
    total_steps: usize,
    updated_at: String,
    last_primary_viz: Option<PrimaryVizRef>,
}

#[derive(Serialize, Deserialize, Clone)]
struct LibraryRunEntry {
    run_id: String,
    template_id: Option<String>,
    status: String,
    primary_viz: Option<PrimaryVizRef>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct LibraryRecord {
    paper_key: String,
    canonical_id: Option<String>,
    title: Option<String>,
    year: Option<i32>,
    source_kind: Option<String>,
    tags: Vec<String>,
    runs: Vec<LibraryRunEntry>,
    primary_viz: Option<PrimaryVizRef>,
    last_run_id: Option<String>,
    last_status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct LibraryReindexResult {
    count_records: usize,
    count_runs: usize,
    updated_at: String,
}

#[derive(Serialize)]
struct LibraryRecordSummary {
    paper_key: String,
    canonical_id: Option<String>,
    title: Option<String>,
    source_kind: Option<String>,
    primary_viz: Option<PrimaryVizRef>,
    last_status: String,
    last_run_id: Option<String>,
    updated_at: String,
    tags: Vec<String>,
}

#[derive(Serialize)]
struct LibraryStats {
    total_papers: usize,
    total_runs: usize,
    status_counts: serde_json::Value,
    kind_counts: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct LibraryListFilter {
    query: Option<String>,
    status: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    year_from: Option<i32>,
    year_to: Option<i32>,
}

#[derive(Serialize, Deserialize)]
struct LibraryMeta {
    index_version: u32,
    updated_at: String,
}

#[derive(Deserialize, Default)]
struct LibrarySearchOpts {
    limit: Option<usize>,
    status: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
}

#[derive(Serialize, Clone)]
struct LibrarySearchHighlight {
    field: String,
    snippet: String,
}

#[derive(Serialize, Clone)]
struct LibrarySearchResult {
    paper_key: String,
    canonical_id: Option<String>,
    title: Option<String>,
    tags: Vec<String>,
    primary_viz: Option<PrimaryVizRef>,
    last_status: String,
    last_run_id: Option<String>,
    score: i64,
    highlights: Option<Vec<LibrarySearchHighlight>>,
    updated_at: String,
}

#[derive(Default)]
struct LibraryCacheState {
    out_dir: Option<PathBuf>,
    source_mtime_ms: u64,
    records: Vec<LibraryRecord>,
}

static JOB_RUNTIME: OnceLock<Arc<Mutex<JobRuntimeState>>> = OnceLock::new();
static LIBRARY_CACHE: OnceLock<Arc<Mutex<LibraryCacheState>>> = OnceLock::new();

#[derive(Serialize, Clone)]
struct TemplateParamDef {
    key: String,
    label: String,
    param_type: String,
    default_value: serde_json::Value,
    min: Option<i64>,
    max: Option<i64>,
}

#[derive(Serialize, Clone)]
struct TaskTemplateDef {
    id: String,
    title: String,
    description: String,
    wired: bool,
    disabled_reason: String,
    params: Vec<TemplateParamDef>,
}

fn template_registry() -> Vec<TaskTemplateDef> {
    vec![
        TaskTemplateDef {
            id: "TEMPLATE_TREE".to_string(),
            title: "Papers Tree".to_string(),
            description: "Build citation tree from canonical identifier".to_string(),
            wired: true,
            disabled_reason: "".to_string(),
            params: vec![
                TemplateParamDef {
                    key: "depth".to_string(),
                    label: "Depth".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(2),
                    min: Some(1),
                    max: Some(2),
                },
                TemplateParamDef {
                    key: "max_per_level".to_string(),
                    label: "Max per level".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(50),
                    min: Some(1),
                    max: Some(200),
                },
            ],
        },
        TaskTemplateDef {
            id: "TEMPLATE_MAP".to_string(),
            title: "Paper Map".to_string(),
            description: "Generate 3D paper map artifacts (graph/json/html)".to_string(),
            wired: true,
            disabled_reason: "".to_string(),
            params: vec![
                TemplateParamDef {
                    key: "k".to_string(),
                    label: "Neighbors (k)".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(24),
                    min: Some(10),
                    max: Some(50),
                },
                TemplateParamDef {
                    key: "seed".to_string(),
                    label: "Random seed".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(42),
                    min: Some(0),
                    max: Some(2_147_483_647),
                },
            ],
        },
        TaskTemplateDef {
            id: "TEMPLATE_RELATED".to_string(),
            title: "Related Papers".to_string(),
            description: "Expand related papers as a focused citation tree".to_string(),
            wired: true,
            disabled_reason: "".to_string(),
            params: vec![
                TemplateParamDef {
                    key: "depth".to_string(),
                    label: "Depth".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(1),
                    min: Some(1),
                    max: Some(2),
                },
                TemplateParamDef {
                    key: "max_per_level".to_string(),
                    label: "Max related per level".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(30),
                    min: Some(1),
                    max: Some(200),
                },
            ],
        },
        TaskTemplateDef {
            id: "TEMPLATE_GRAPH".to_string(),
            title: "Graph Explorer Seed".to_string(),
            description: "Generate graph/map artifacts with larger neighborhood".to_string(),
            wired: true,
            disabled_reason: "".to_string(),
            params: vec![
                TemplateParamDef {
                    key: "k".to_string(),
                    label: "Neighbors (k)".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(40),
                    min: Some(10),
                    max: Some(50),
                },
                TemplateParamDef {
                    key: "seed".to_string(),
                    label: "Random seed".to_string(),
                    param_type: "integer".to_string(),
                    default_value: serde_json::json!(42),
                    min: Some(0),
                    max: Some(2_147_483_647),
                },
            ],
        },
        TaskTemplateDef {
            id: "TEMPLATE_SUMMARY".to_string(),
            title: "Paper Summary".to_string(),
            description: "Generate summary (placeholder)".to_string(),
            wired: false,
            disabled_reason: "not wired".to_string(),
            params: vec![],
        },
    ]
}

fn find_template(id: &str) -> Option<TaskTemplateDef> {
    template_registry().into_iter().find(|t| t.id == id)
}

fn json_i64_with_default(
    value: Option<&serde_json::Value>,
    default_value: i64,
    min: i64,
    max: i64,
) -> Result<i64, String> {
    let parsed = match value {
        None => default_value,
        Some(v) if v.is_null() => default_value,
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| "expected integer parameter".to_string())?,
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid integer parameter: {s}"))?,
        Some(_) => return Err("expected integer parameter".to_string()),
    };

    if parsed < min || parsed > max {
        return Err(format!("parameter out of range: {parsed} (allowed: {min}..{max})"));
    }
    Ok(parsed)
}

fn build_template_args(
    template_id: &str,
    canonical_id: &str,
    params: &serde_json::Value,
) -> Result<(Vec<String>, serde_json::Value), String> {
    match template_id {
        "TEMPLATE_TREE" => {
            let normalized = normalize_identifier_internal(canonical_id);
            let pipeline_id = to_pipeline_identifier(&normalized)
                .map_err(|e| format!("identifier normalize error: {e}"))?;

            let obj = params.as_object();
            let depth = json_i64_with_default(
                obj.and_then(|m| m.get("depth")),
                2,
                1,
                2,
            )?;
            let max_per_level = json_i64_with_default(
                obj.and_then(|m| m.get("max_per_level")),
                50,
                1,
                200,
            )?;

            let argv = vec![
                "papers".to_string(),
                "tree".to_string(),
                "--id".to_string(),
                pipeline_id,
                "--depth".to_string(),
                depth.to_string(),
                "--max-per-level".to_string(),
                max_per_level.to_string(),
            ];

            let normalized_params = serde_json::json!({
                "depth": depth,
                "max_per_level": max_per_level,
            });

            Ok((argv, normalized_params))
        }
        "TEMPLATE_RELATED" => {
            let normalized = normalize_identifier_internal(canonical_id);
            let pipeline_id = to_pipeline_identifier(&normalized)
                .map_err(|e| format!("identifier normalize error: {e}"))?;

            let obj = params.as_object();
            let depth = json_i64_with_default(
                obj.and_then(|m| m.get("depth")),
                1,
                1,
                2,
            )?;
            let max_per_level = json_i64_with_default(
                obj.and_then(|m| m.get("max_per_level")),
                30,
                1,
                200,
            )?;

            let argv = vec![
                "papers".to_string(),
                "tree".to_string(),
                "--id".to_string(),
                pipeline_id,
                "--depth".to_string(),
                depth.to_string(),
                "--max-per-level".to_string(),
                max_per_level.to_string(),
            ];

            let normalized_params = serde_json::json!({
                "depth": depth,
                "max_per_level": max_per_level,
            });

            Ok((argv, normalized_params))
        }
        "TEMPLATE_MAP" | "TEMPLATE_GRAPH" => {
            let normalized = normalize_identifier_internal(canonical_id);
            let pipeline_id = to_pipeline_identifier(&normalized)
                .map_err(|e| format!("identifier normalize error: {e}"))?;

            let obj = params.as_object();
            let default_k = if template_id == "TEMPLATE_GRAPH" { 40 } else { 24 };
            let k = json_i64_with_default(
                obj.and_then(|m| m.get("k")),
                default_k,
                10,
                50,
            )?;
            let seed = json_i64_with_default(
                obj.and_then(|m| m.get("seed")),
                42,
                0,
                2_147_483_647,
            )?;

            let argv = vec![
                "papers".to_string(),
                "map3d".to_string(),
                "--id".to_string(),
                pipeline_id,
                "--k".to_string(),
                k.to_string(),
                "--seed".to_string(),
                seed.to_string(),
            ];

            let normalized_params = serde_json::json!({
                "k": k,
                "seed": seed,
            });

            Ok((argv, normalized_params))
        }
        other => Err(format!("template not wired: {other}")),
    }
}

fn split_url_tail(raw: &str) -> String {
    raw.split(&['?', '#'][..]).next().unwrap_or("").trim().to_string()
}

fn normalize_identifier_internal(input: &str) -> NormalizedIdentifier {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let mut s = input.trim().to_string();
    s = s.trim_matches('"').trim_matches('\'').trim().to_string();
    s = s.replace('\u{3000}', " ");
    s = s.trim().to_string();

    if s.is_empty() {
        errors.push("identifier is empty".to_string());
        return NormalizedIdentifier {
            kind: "unknown".to_string(),
            canonical: "".to_string(),
            display: "".to_string(),
            warnings,
            errors,
        };
    }

    let lower = s.to_lowercase();

    if lower.contains("doi.org/") {
        let idx = lower.find("doi.org/").unwrap_or(0);
        let tail = split_url_tail(&s[(idx + "doi.org/".len())..]);
        let doi = tail.trim_end_matches('/').trim().to_lowercase();
        if doi.is_empty() {
            errors.push("failed to parse DOI from URL".to_string());
        } else {
            warnings.push("DOI extracted from URL".to_string());
            return NormalizedIdentifier {
                kind: "doi".to_string(),
                canonical: doi.clone(),
                display: format!("doi:{doi}"),
                warnings,
                errors,
            };
        }
    }

    if lower.starts_with("doi:") {
        let doi = s[4..].trim().to_lowercase();
        if doi.is_empty() {
            errors.push("DOI prefix exists but body is empty".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "doi".to_string(),
                canonical: doi.clone(),
                display: format!("doi:{doi}"),
                warnings,
                errors,
            };
        }
    }

    if s.starts_with("10.") && s.contains('/') {
        let doi = s.replace(' ', "").to_lowercase();
        return NormalizedIdentifier {
            kind: "doi".to_string(),
            canonical: doi.clone(),
            display: format!("doi:{doi}"),
            warnings,
            errors,
        };
    }

    if lower.contains("pubmed.ncbi.nlm.nih.gov/") {
        if let Some(idx) = lower.find("pubmed.ncbi.nlm.nih.gov/") {
            let tail = split_url_tail(&s[(idx + "pubmed.ncbi.nlm.nih.gov/".len())..]);
            let pmid = tail.trim_end_matches('/').trim();
            if !pmid.is_empty() && pmid.chars().all(|c| c.is_ascii_digit()) {
                warnings.push("PMID extracted from PubMed URL".to_string());
                return NormalizedIdentifier {
                    kind: "pmid".to_string(),
                    canonical: format!("pmid:{pmid}"),
                    display: format!("pmid:{pmid}"),
                    warnings,
                    errors,
                };
            }
        }
        errors.push("failed to parse PMID from PubMed URL".to_string());
    }

    if lower.starts_with("pmid:") {
        let body = s[5..].trim();
        if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
            errors.push("pmid must be digits".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "pmid".to_string(),
                canonical: format!("pmid:{body}"),
                display: format!("pmid:{body}"),
                warnings,
                errors,
            };
        }
    }

    if s.chars().all(|c| c.is_ascii_digit()) {
        return NormalizedIdentifier {
            kind: "pmid".to_string(),
            canonical: format!("pmid:{s}"),
            display: format!("pmid:{s}"),
            warnings,
            errors,
        };
    }

    if lower.contains("arxiv.org/abs/") {
        if let Some(idx) = lower.find("arxiv.org/abs/") {
            let tail = split_url_tail(&s[(idx + "arxiv.org/abs/".len())..]);
            let id = tail.trim_end_matches('/').trim();
            if !id.is_empty() {
                warnings.push("arXiv id extracted from URL".to_string());
                return NormalizedIdentifier {
                    kind: "arxiv".to_string(),
                    canonical: format!("arxiv:{id}"),
                    display: format!("arxiv:{id}"),
                    warnings,
                    errors,
                };
            }
        }
        errors.push("failed to parse arXiv id from URL".to_string());
    }

    if lower.contains("arxiv.org/pdf/") {
        if let Some(idx) = lower.find("arxiv.org/pdf/") {
            let tail = split_url_tail(&s[(idx + "arxiv.org/pdf/".len())..]);
            let id = tail.trim_end_matches(".pdf").trim_end_matches('/').trim();
            if !id.is_empty() {
                warnings.push("arXiv id extracted from PDF URL".to_string());
                return NormalizedIdentifier {
                    kind: "arxiv".to_string(),
                    canonical: format!("arxiv:{id}"),
                    display: format!("arxiv:{id}"),
                    warnings,
                    errors,
                };
            }
        }
        errors.push("failed to parse arXiv id from PDF URL".to_string());
    }

    if lower.starts_with("arxiv:") {
        let body = s[6..].trim();
        if body.is_empty() {
            errors.push("arxiv prefix exists but body is empty".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "arxiv".to_string(),
                canonical: format!("arxiv:{body}"),
                display: format!("arxiv:{body}"),
                warnings,
                errors,
            };
        }
    }

    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '/' || c == '-')
        && (s.contains('.') || s.contains('/'))
    {
        return NormalizedIdentifier {
            kind: "arxiv".to_string(),
            canonical: format!("arxiv:{s}"),
            display: format!("arxiv:{s}"),
            warnings,
            errors,
        };
    }

    if lower.contains("semanticscholar.org/paper/") {
        let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
        if let Some(last) = parts.last() {
            let id = split_url_tail(last);
            if !id.is_empty() {
                warnings.push("S2 id extracted from URL".to_string());
                return NormalizedIdentifier {
                    kind: "s2".to_string(),
                    canonical: format!("S2PaperId:{id}"),
                    display: format!("S2PaperId:{id}"),
                    warnings,
                    errors,
                };
            }
        }
        errors.push("failed to parse Semantic Scholar id from URL".to_string());
    }

    if lower.starts_with("corpusid:") {
        let body = s[9..].trim();
        if body.is_empty() {
            errors.push("CorpusId prefix exists but body is empty".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "s2".to_string(),
                canonical: format!("CorpusId:{body}"),
                display: format!("CorpusId:{body}"),
                warnings,
                errors,
            };
        }
    }

    if lower.starts_with("s2paperid:") {
        let body = s[10..].trim();
        if body.is_empty() {
            errors.push("S2PaperId prefix exists but body is empty".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "s2".to_string(),
                canonical: format!("S2PaperId:{body}"),
                display: format!("S2PaperId:{body}"),
                warnings,
                errors,
            };
        }
    }

    if lower.starts_with("s2:") {
        let body = s[3..].trim();
        if body.is_empty() {
            errors.push("s2 prefix exists but body is empty".to_string());
        } else {
            return NormalizedIdentifier {
                kind: "s2".to_string(),
                canonical: format!("S2PaperId:{body}"),
                display: format!("S2PaperId:{body}"),
                warnings,
                errors,
            };
        }
    }

    errors.push("unknown identifier format".to_string());
    NormalizedIdentifier {
        kind: "unknown".to_string(),
        canonical: s,
        display: "unknown".to_string(),
        warnings,
        errors,
    }
}

fn to_pipeline_identifier(normalized: &NormalizedIdentifier) -> Result<String, String> {
    if !normalized.errors.is_empty() {
        return Err(normalized.errors.join("; "));
    }
    match normalized.kind.as_str() {
        "doi" => Ok(format!("doi:{}", normalized.canonical)),
        "pmid" | "arxiv" => Ok(normalized.canonical.clone()),
        "s2" => {
            if let Some(body) = normalized.canonical.strip_prefix("CorpusId:") {
                return Ok(format!("s2:CorpusId:{body}"));
            }
            if let Some(body) = normalized.canonical.strip_prefix("S2PaperId:") {
                return Ok(format!("s2:S2PaperId:{body}"));
            }
            Ok(format!("s2:{}", normalized.canonical))
        }
        _ => Err("unknown identifier kind".to_string()),
    }
}

fn make_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{}", now.as_secs(), now.subsec_nanos())
}

fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_epoch_ms_string() -> String {
    now_epoch_ms().to_string()
}

fn jobs_file_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("jobs.json")
}

fn pipelines_file_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("pipelines.json")
}

fn settings_file_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("settings.json")
}

fn audit_jsonl_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("audit.jsonl")
}

fn library_jsonl_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("library.jsonl")
}

fn library_meta_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("library_meta.json")
}

fn library_cache_state() -> Arc<Mutex<LibraryCacheState>> {
    LIBRARY_CACHE
        .get_or_init(|| Arc::new(Mutex::new(LibraryCacheState::default())))
        .clone()
}

fn library_source_mtime_ms(out_dir: &Path) -> u64 {
    let src = library_jsonl_path(out_dir);
    if !src.exists() {
        return 0;
    }
    modified_epoch_ms(&src)
}

fn cache_library_records(out_dir: &Path, records: &[LibraryRecord]) -> Result<(), String> {
    let state = library_cache_state();
    let mut guard = state
        .lock()
        .map_err(|_| "failed to lock library cache".to_string())?;
    guard.out_dir = Some(out_dir.to_path_buf());
    guard.source_mtime_ms = library_source_mtime_ms(out_dir);
    guard.records = records.to_vec();
    Ok(())
}

fn load_library_records_cached(out_dir: &Path, force_reload: bool) -> Result<Vec<LibraryRecord>, String> {
    let state = library_cache_state();
    let src_mtime = library_source_mtime_ms(out_dir);

    {
        let guard = state
            .lock()
            .map_err(|_| "failed to lock library cache".to_string())?;
        if !force_reload
            && guard.out_dir.as_deref() == Some(out_dir)
            && guard.source_mtime_ms == src_mtime
        {
            return Ok(guard.records.clone());
        }
    }

    let fresh = read_library_records(out_dir)?;
    cache_library_records(out_dir, &fresh)?;
    Ok(fresh)
}

fn to_iso_from_system_time(st: SystemTime) -> String {
    let dt: DateTime<Utc> = st.into();
    dt.to_rfc3339()
}

fn canonical_kind(canonical_id: Option<&str>) -> Option<String> {
    let c = canonical_id?.to_lowercase();
    if c.starts_with("doi:") || c.starts_with("10.") {
        Some("doi".to_string())
    } else if c.starts_with("pmid:") {
        Some("pmid".to_string())
    } else if c.starts_with("arxiv:") {
        Some("arxiv".to_string())
    } else if c.starts_with("s2:") || c.starts_with("corpusid:") || c.starts_with("s2paperid:") {
        Some("s2".to_string())
    } else {
        Some("unknown".to_string())
    }
}

fn read_library_records(out_dir: &Path) -> Result<Vec<LibraryRecord>, String> {
    let path = library_jsonl_path(out_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read library index {}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<LibraryRecord>(t) {
            rows.push(v);
        }
    }
    Ok(rows)
}

fn write_library_records(out_dir: &Path, records: &[LibraryRecord]) -> Result<(), String> {
    let path = library_jsonl_path(out_dir);
    let mut lines = Vec::with_capacity(records.len());
    for rec in records {
        lines.push(
            serde_json::to_string(rec)
                .map_err(|e| format!("failed to encode library record {}: {e}", rec.paper_key))?,
        );
    }
    let content = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    atomic_write_text(&path, &content)?;

    let meta = LibraryMeta {
        index_version: 1,
        updated_at: Utc::now().to_rfc3339(),
    };
    let meta_text = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("failed to serialize library meta: {e}"))?;
    atomic_write_text(&library_meta_path(out_dir), &meta_text)?;
    cache_library_records(out_dir, records)
}

fn tokenize_query(raw: &str) -> Vec<String> {
    raw.to_lowercase()
        .split_whitespace()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn make_highlight(field: &str, value: &str, token: &str) -> LibrarySearchHighlight {
    let lower = value.to_lowercase();
    if let Some(pos) = lower.find(token) {
        let start = pos.saturating_sub(24);
        let end = (pos + token.len() + 24).min(value.len());
        let snippet = value[start..end].trim().to_string();
        return LibrarySearchHighlight {
            field: field.to_string(),
            snippet,
        };
    }
    LibrarySearchHighlight {
        field: field.to_string(),
        snippet: value.chars().take(72).collect::<String>(),
    }
}

fn score_library_record(rec: &LibraryRecord, tokens: &[String]) -> (i64, Vec<LibrarySearchHighlight>, bool) {
    let canonical = rec.canonical_id.clone().unwrap_or_default();
    let canonical_lower = canonical.to_lowercase();
    let title = rec.title.clone().unwrap_or_default();
    let title_lower = title.to_lowercase();
    let tags_lower: Vec<String> = rec.tags.iter().map(|t| t.to_lowercase()).collect();
    let run_ids_lower: Vec<String> = rec.runs.iter().map(|r| r.run_id.to_lowercase()).collect();
    let template_ids_lower: Vec<String> = rec
        .runs
        .iter()
        .filter_map(|r| r.template_id.clone())
        .map(|t| t.to_lowercase())
        .collect();
    let statuses_lower: Vec<String> = rec.runs.iter().map(|r| r.status.to_lowercase()).collect();

    let mut score = 0i64;
    let mut highlights: Vec<LibrarySearchHighlight> = Vec::new();
    let mut matched_any = false;

    for tok in tokens {
        let mut token_matched = false;

        if !canonical_lower.is_empty() {
            if canonical_lower == *tok {
                score += 100;
                token_matched = true;
                highlights.push(make_highlight("canonical_id", &canonical, tok));
            } else if canonical_lower.contains(tok) {
                score += 60;
                token_matched = true;
                highlights.push(make_highlight("canonical_id", &canonical, tok));
            }
        }

        if !title_lower.is_empty() && title_lower.contains(tok) {
            score += 40;
            token_matched = true;
            highlights.push(make_highlight("title", &title, tok));
        }

        if tags_lower.iter().any(|t| t == tok) {
            score += 30;
            token_matched = true;
            if let Some(tag) = rec.tags.iter().find(|t| t.to_lowercase() == *tok) {
                highlights.push(make_highlight("tag", tag, tok));
            }
        }

        if run_ids_lower.iter().any(|r| r.contains(tok)) {
            score += 20;
            token_matched = true;
            if let Some(run) = rec.runs.iter().find(|r| r.run_id.to_lowercase().contains(tok)) {
                highlights.push(make_highlight("run_id", &run.run_id, tok));
            }
        }

        if template_ids_lower.iter().any(|t| t.contains(tok)) {
            score += 10;
            token_matched = true;
            if let Some(run) = rec.runs.iter().find(|r| {
                r.template_id
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(tok)
            }) {
                let text = run.template_id.clone().unwrap_or_default();
                highlights.push(make_highlight("template_id", &text, tok));
            }
        }

        if rec.last_status.to_lowercase().contains(tok)
            || statuses_lower.iter().any(|s| s.contains(tok))
        {
            token_matched = true;
            highlights.push(make_highlight("status", &rec.last_status, tok));
        }

        if token_matched {
            matched_any = true;
        }
    }

    if highlights.len() > 6 {
        highlights.truncate(6);
    }
    (score.min(10_000), highlights, matched_any)
}

fn parse_known_title(v: &serde_json::Value) -> Option<String> {
    for key in ["title", "paper_title", "name"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn parse_known_year(v: &serde_json::Value) -> Option<i32> {
    for key in ["year", "published_year"] {
        if let Some(y) = v.get(key).and_then(|x| x.as_i64()) {
            if (1900..=2200).contains(&(y as i32)) {
                return Some(y as i32);
            }
        }
    }
    None
}

fn parse_primary_viz_from_input(v: &serde_json::Value) -> Option<PrimaryVizRef> {
    let pv = v
        .get("desktop")
        .and_then(|x| x.get("primary_viz"))
        .and_then(|x| x.as_object())?;
    let name = pv
        .get("name")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())?;
    if name.is_empty() {
        return None;
    }
    let kind = pv
        .get("kind")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_lowercase())?;
    if kind != "html" && kind != "graph_json" {
        return None;
    }
    Some(PrimaryVizRef { name, kind })
}

fn extract_run_for_library(run_dir: &Path) -> Option<(String, LibraryRunEntry, Option<String>, Option<String>, Option<i32>)> {
    let run_id = run_dir.file_name()?.to_string_lossy().to_string();
    let meta = fs::metadata(run_dir).ok()?;
    let created_at = meta
        .created()
        .or_else(|_| meta.modified())
        .ok()
        .map(to_iso_from_system_time)
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let updated_at = meta
        .modified()
        .ok()
        .map(to_iso_from_system_time)
        .unwrap_or_else(|| created_at.clone());

    let input_path = run_dir.join("input.json");
    let result_path = run_dir.join("result.json");

    let mut canonical_id: Option<String> = None;
    let mut template_id: Option<String> = None;
    let mut primary_viz: Option<PrimaryVizRef> = None;
    let mut title: Option<String> = None;
    let mut year: Option<i32> = None;

    if input_path.exists() {
        if let Ok(raw) = fs::read_to_string(&input_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(s) = v
                    .get("desktop")
                    .and_then(|x| x.get("canonical_id"))
                    .and_then(|x| x.as_str())
                {
                    if !s.trim().is_empty() {
                        canonical_id = Some(s.trim().to_string());
                    }
                }
                if let Some(s) = v
                    .get("desktop")
                    .and_then(|x| x.get("template_id"))
                    .and_then(|x| x.as_str())
                {
                    if !s.trim().is_empty() {
                        template_id = Some(s.trim().to_string());
                    }
                }
                if primary_viz.is_none() {
                    primary_viz = parse_primary_viz_from_input(&v);
                }
                if canonical_id.is_none() {
                    for key in ["paper_id", "canonical_id", "id"] {
                        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                            if !s.trim().is_empty() {
                                canonical_id = Some(s.trim().to_string());
                                break;
                            }
                        }
                    }
                }
                if title.is_none() {
                    title = parse_known_title(&v);
                }
                if year.is_none() {
                    year = parse_known_year(&v);
                }
            }
        }
    }

    let mut status = "unknown".to_string();
    if result_path.exists() {
        if let Ok(raw) = fs::read_to_string(&result_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
                    let raw_status = s.trim().to_lowercase();
                    status = match raw_status.as_str() {
                        "ok" | "success" | "succeeded" => "succeeded".to_string(),
                        "error" | "failed" => "failed".to_string(),
                        "needs_retry" => "needs_retry".to_string(),
                        "running" => "running".to_string(),
                        _ => "unknown".to_string(),
                    };
                } else if let Some(ok) = v.get("ok").and_then(|x| x.as_bool()) {
                    status = if ok { "succeeded".to_string() } else { "failed".to_string() };
                }

                let (needs_retry, _retry_after) = inspect_retry_fields(&v);
                if needs_retry {
                    status = "needs_retry".to_string();
                }

                if title.is_none() {
                    title = parse_known_title(&v);
                }
                if year.is_none() {
                    year = parse_known_year(&v);
                }
            }
        }
    }

    let run = LibraryRunEntry {
        run_id: run_id.clone(),
        template_id,
        status,
        primary_viz,
        created_at,
        updated_at,
    };

    let paper_key = canonical_id
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("run:{run_id}"));
    Some((paper_key, run, canonical_id, title, year))
}

fn build_library_records(out_dir: &Path, existing: &[LibraryRecord]) -> Result<Vec<LibraryRecord>, String> {
    let mut existing_tags = std::collections::HashMap::<String, Vec<String>>::new();
    for rec in existing {
        existing_tags.insert(rec.paper_key.clone(), rec.tags.clone());
    }

    let mut grouped = std::collections::HashMap::<String, LibraryRecord>::new();
    let entries = fs::read_dir(out_dir)
        .map_err(|e| format!("failed to read runs directory {}: {e}", out_dir.display()))?;

    for entry in entries.flatten() {
        let run_dir = entry.path();
        if !run_dir.is_dir() {
            continue;
        }
        let Some((paper_key, run, canonical_id, title, year)) = extract_run_for_library(&run_dir) else {
            continue;
        };

        let now = Utc::now().to_rfc3339();
        let rec = grouped.entry(paper_key.clone()).or_insert_with(|| LibraryRecord {
            paper_key: paper_key.clone(),
            canonical_id: canonical_id.clone(),
            title: title.clone(),
            year,
            source_kind: canonical_kind(canonical_id.as_deref()),
            tags: existing_tags.get(&paper_key).cloned().unwrap_or_default(),
            runs: Vec::new(),
            primary_viz: None,
            last_run_id: None,
            last_status: "unknown".to_string(),
            created_at: now.clone(),
            updated_at: now,
        });

        if rec.canonical_id.is_none() {
            rec.canonical_id = canonical_id.clone();
            rec.source_kind = canonical_kind(rec.canonical_id.as_deref());
        }
        if rec.title.is_none() {
            rec.title = title.clone();
        }
        if rec.year.is_none() {
            rec.year = year;
        }
        rec.runs.push(run);
    }

    let mut records: Vec<LibraryRecord> = grouped
        .into_values()
        .map(|mut rec| {
            rec.runs.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.run_id.cmp(&b.run_id))
            });
            rec.last_run_id = rec.runs.first().map(|r| r.run_id.clone());
            rec.last_status = rec
                .runs
                .first()
                .map(|r| r.status.clone())
                .unwrap_or_else(|| "unknown".to_string());
            rec.updated_at = rec
                .runs
                .first()
                .map(|r| r.updated_at.clone())
                .unwrap_or_else(|| rec.updated_at.clone());
            rec.primary_viz = rec.runs.first().and_then(|r| r.primary_viz.clone());
            rec.created_at = rec
                .runs
                .iter()
                .map(|r| r.created_at.clone())
                .min()
                .unwrap_or_else(|| rec.created_at.clone());
            rec
        })
        .collect();

    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.paper_key.cmp(&b.paper_key))
    });

    Ok(records)
}

fn upsert_library_run(out_dir: &Path, run_id: &str) -> Result<(), String> {
    let mut records = load_library_records_cached(out_dir, false)?;
    for rec in &mut records {
        rec.runs.retain(|r| r.run_id != run_id);
    }
    records.retain(|r| !r.runs.is_empty());

    let run_dir = out_dir.join(run_id);
    if let Some((paper_key, run, canonical_id, title, year)) = extract_run_for_library(&run_dir) {
        let now = Utc::now().to_rfc3339();
        let run_status = run.status.clone();
        let run_primary_viz = run.primary_viz.clone();
        if let Some(rec) = records.iter_mut().find(|r| r.paper_key == paper_key) {
            rec.runs.push(run);
            rec.runs.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.run_id.cmp(&b.run_id))
            });
            rec.last_run_id = rec.runs.first().map(|r| r.run_id.clone());
            rec.last_status = rec
                .runs
                .first()
                .map(|r| r.status.clone())
                .unwrap_or_else(|| "unknown".to_string());
            rec.updated_at = rec
                .runs
                .first()
                .map(|r| r.updated_at.clone())
                .unwrap_or_else(|| now.clone());
            rec.primary_viz = rec.runs.first().and_then(|r| r.primary_viz.clone());
            if rec.canonical_id.is_none() {
                rec.canonical_id = canonical_id.clone();
            }
            if rec.title.is_none() {
                rec.title = title.clone();
            }
            if rec.year.is_none() {
                rec.year = year;
            }
            rec.source_kind = canonical_kind(rec.canonical_id.as_deref());
        } else {
            records.push(LibraryRecord {
                paper_key: paper_key.clone(),
                canonical_id: canonical_id.clone(),
                title,
                year,
                source_kind: canonical_kind(canonical_id.as_deref()),
                tags: Vec::new(),
                runs: vec![run],
                primary_viz: run_primary_viz,
                last_run_id: Some(run_id.to_string()),
                last_status: run_status,
                created_at: now.clone(),
                updated_at: now,
            });
        }
    }

    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.paper_key.cmp(&b.paper_key))
    });
    write_library_records(out_dir, &records)
}

fn atomic_write_text(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid path without parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;

    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content)
        .map_err(|e| format!("failed to write temp file {}: {e}", tmp.display()))?;

    if path.exists() {
        fs::remove_file(path)
            .map_err(|e| format!("failed to replace file {}: {e}", path.display()))?;
    }
    fs::rename(&tmp, path)
        .map_err(|e| format!("failed to move temp file to {}: {e}", path.display()))
}

fn subsystem_display_name(subsystem: &str) -> &str {
    match subsystem {
        "jobs" => "jobs.json",
        "pipelines" => "pipelines.json",
        "settings" => "settings.json",
        _ => subsystem,
    }
}

fn parse_schema_version(value: &serde_json::Value) -> Result<u32, String> {
    if let Some(n) = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
    {
        return u32::try_from(n)
            .map_err(|_| "schema_version is out of supported range".to_string());
    }
    Ok(1)
}

fn migrate_schema_value(
    _subsystem: &str,
    from_version: u32,
    to_version: u32,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match (from_version, to_version) {
        (1, 2) => Ok(value),
        _ => Err(format!(
            "no migration path from schema_version={from_version} to {to_version}"
        )),
    }
}

fn load_with_migration<T, F>(path: &Path, subsystem: &str, decode: F) -> Result<T, String>
where
    F: FnOnce(serde_json::Value) -> Result<T, String>,
{
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed to read {} {}: {e}", subsystem_display_name(subsystem), path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse {} {}: {e}", subsystem_display_name(subsystem), path.display()))?;
    if !value.is_object() {
        return Err(format!(
            "invalid {} {}: root must be an object",
            subsystem_display_name(subsystem),
            path.display()
        ));
    }

    let mut version = parse_schema_version(&value)?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "{} has unsupported schema_version={} (supported={}); subsystem is read-only",
            subsystem_display_name(subsystem),
            version,
            SCHEMA_VERSION
        ));
    }

    while version < SCHEMA_VERSION {
        let next = version + 1;
        value = migrate_schema_value(subsystem, version, next, value)?;
        version = next;
    }

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "schema_version".to_string(),
            serde_json::Value::Number(serde_json::Number::from(SCHEMA_VERSION as u64)),
        );
    }
    decode(value)
}

fn ensure_schema_writable(path: &Path, subsystem: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed to read {} {}: {e}", subsystem_display_name(subsystem), path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse {} {}: {e}", subsystem_display_name(subsystem), path.display()))?;
    let version = parse_schema_version(&value)?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "{} has unsupported schema_version={} (supported={}); refusing to modify",
            subsystem_display_name(subsystem),
            version,
            SCHEMA_VERSION
        ));
    }
    Ok(())
}

fn load_jobs_from_file(path: &Path) -> Result<Vec<JobRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let payload: JobFilePayload = load_with_migration(path, "jobs", |value| {
        serde_json::from_value::<JobFilePayload>(value)
            .map_err(|e| format!("failed to decode jobs file {}: {e}", path.display()))
    })?;
    Ok(payload.jobs)
}

fn save_jobs_to_file(path: &Path, jobs: &[JobRecord]) -> Result<(), String> {
    ensure_schema_writable(path, "jobs")?;
    let payload = JobFilePayload {
        schema_version: SCHEMA_VERSION,
        jobs: jobs.to_vec(),
    };
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("failed to serialize jobs payload: {e}"))?;
    atomic_write_text(path, &text)
}

fn load_pipelines_from_file(path: &Path) -> Result<Vec<PipelineRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let payload: PipelineFilePayload = load_with_migration(path, "pipelines", |value| {
        serde_json::from_value::<PipelineFilePayload>(value)
            .map_err(|e| format!("failed to decode pipelines file {}: {e}", path.display()))
    })?;
    Ok(payload.pipelines)
}

fn save_pipelines_to_file(path: &Path, pipelines: &[PipelineRecord]) -> Result<(), String> {
    ensure_schema_writable(path, "pipelines")?;
    let payload = PipelineFilePayload {
        schema_version: SCHEMA_VERSION,
        pipelines: pipelines.to_vec(),
    };
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("failed to serialize pipelines payload: {e}"))?;
    atomic_write_text(path, &text)
}

fn load_settings(out_dir: &Path) -> Result<DesktopSettings, String> {
    let path = settings_file_path(out_dir);
    if !path.exists() {
        let defaults = DesktopSettings::default();
        save_settings(out_dir, &defaults)?;
        return Ok(defaults);
    }
    load_with_migration(&path, "settings", |value| {
        if value.get("settings").is_some() {
            let payload = serde_json::from_value::<SettingsFilePayload>(value)
                .map_err(|e| format!("failed to decode settings file {}: {e}", path.display()))?;
            return Ok(payload.settings);
        }
        serde_json::from_value::<DesktopSettings>(value)
            .map_err(|e| format!("failed to parse legacy settings file {}: {e}", path.display()))
    })
}

fn save_settings(out_dir: &Path, settings: &DesktopSettings) -> Result<(), String> {
    let path = settings_file_path(out_dir);
    ensure_schema_writable(&path, "settings")?;
    let payload = SettingsFilePayload {
        schema_version: SCHEMA_VERSION,
        settings: settings.clone(),
    };
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("failed to serialize settings: {e}"))?;
    atomic_write_text(&path, &text)
}

fn append_audit_auto_retry(out_dir: &Path, entry: &AuditAutoRetryEntry) -> Result<(), String> {
    let path = audit_jsonl_path(out_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create audit directory {}: {e}", parent.display()))?;
    }
    let line = serde_json::to_string(entry)
        .map_err(|e| format!("failed to serialize audit entry: {e}"))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("failed to open audit log {}: {e}", path.display()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("failed to append audit log {}: {e}", path.display()))?;
    file.write_all(b"\n")
        .map_err(|e| format!("failed to append newline to audit log {}: {e}", path.display()))
}

fn compute_next_retry_at_ms(
    now_ms: u128,
    retry_after_seconds: Option<f64>,
    auto_retry_attempt_count: u32,
    settings: &DesktopSettings,
) -> String {
    let delay_seconds = if let Some(sec) = retry_after_seconds {
        sec.max(0.0).min(settings.auto_retry_max_delay_seconds as f64)
    } else {
        let exp = auto_retry_attempt_count.saturating_sub(1).min(31);
        let base = settings.auto_retry_base_delay_seconds as u128;
        let raw = base.saturating_mul(1u128 << exp);
        let capped = raw.min(settings.auto_retry_max_delay_seconds as u128);
        capped as f64
    };
    let next = now_ms as f64 + delay_seconds * 1000.0;
    format!("{:.0}", next.max(now_ms as f64))
}

fn parse_retry_at_ms(text: Option<&String>) -> Option<u128> {
    let raw = text?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<u128>().ok()
}

fn pipeline_step_status_from_job(job: &JobRecord) -> PipelineStepStatus {
    match job.status {
        JobStatus::Queued | JobStatus::Running => PipelineStepStatus::Running,
        JobStatus::Succeeded => PipelineStepStatus::Succeeded,
        JobStatus::Failed => PipelineStepStatus::Failed,
        JobStatus::NeedsRetry => PipelineStepStatus::NeedsRetry,
        JobStatus::Canceled => PipelineStepStatus::Canceled,
    }
}

fn is_needs_attention_job_status(status: &JobStatus) -> bool {
    matches!(status, JobStatus::Failed | JobStatus::NeedsRetry)
}

fn is_needs_attention_pipeline_status(status: &PipelineStatus) -> bool {
    matches!(status, PipelineStatus::Failed | PipelineStatus::NeedsRetry)
}

fn pipeline_status_text(status: &PipelineStatus) -> &'static str {
    match status {
        PipelineStatus::Running => "running",
        PipelineStatus::Succeeded => "succeeded",
        PipelineStatus::Failed => "failed",
        PipelineStatus::NeedsRetry => "needs_retry",
        PipelineStatus::Canceled => "canceled",
    }
}

fn is_pipeline_step_terminal(status: &PipelineStepStatus) -> bool {
    matches!(
        status,
        PipelineStepStatus::Succeeded
            | PipelineStepStatus::Failed
            | PipelineStepStatus::NeedsRetry
            | PipelineStepStatus::Canceled
    )
}

fn parse_run_primary_viz(run_dir: &Path) -> Option<PrimaryVizRef> {
    let input_path = run_dir.join("input.json");
    let raw = fs::read_to_string(input_path).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    parse_primary_viz_from_input(&v)
}

fn make_pipeline_id() -> String {
    format!("pipe_{}_{}", now_epoch_ms(), make_run_id())
}

fn sanitize_step_id(template_id: &str, index: usize) -> String {
    let t = template_id
        .to_lowercase()
        .replace(|c: char| !(c.is_ascii_alphanumeric() || c == '_'), "_");
    format!("step_{:02}_{}", index + 1, t)
}

fn runtime_and_jobs_path() -> Result<(RuntimeConfig, PathBuf), String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let jobs_path = jobs_file_path(&runtime.out_base_dir);
    Ok((runtime, jobs_path))
}

fn init_job_runtime() -> Result<(Arc<Mutex<JobRuntimeState>>, PathBuf), String> {
    let (_runtime, jobs_path) = runtime_and_jobs_path()?;
    let state = JOB_RUNTIME
        .get_or_init(|| Arc::new(Mutex::new(JobRuntimeState::default())))
        .clone();

    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        if guard.jobs.is_empty() {
            guard.jobs = load_jobs_from_file(&jobs_path)?;
        }
    }

    Ok((state, jobs_path))
}

fn persist_state(state: &Arc<Mutex<JobRuntimeState>>, jobs_path: &Path) -> Result<(), String> {
    let jobs = {
        let guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime for persist".to_string())?;
        guard.jobs.clone()
    };
    save_jobs_to_file(jobs_path, &jobs)
}

fn repo_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn config_file_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let trimmed = appdata.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join("jarvis-desktop")
                .join("config.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join(".config")
                .join("jarvis-desktop")
                .join("config.json");
        }
    }
    PathBuf::from("config.json")
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn absolutize(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn is_pipeline_root(path: &Path) -> bool {
    path.join("pyproject.toml").is_file()
        && path.join("jarvis_cli.py").is_file()
        && path.join("jarvis_core").is_dir()
}

fn find_pipeline_root_autodetect(repo_root: &Path) -> Option<PathBuf> {
    for ancestor in repo_root.ancestors() {
        let direct = ancestor.to_path_buf();
        if is_pipeline_root(&direct) {
            return Some(canonical_or_self(&direct));
        }

        let sibling = ancestor.join("jarvis-ml-pipeline");
        if is_pipeline_root(&sibling) {
            return Some(canonical_or_self(&sibling));
        }
    }
    None
}

fn non_empty_opt(value: Option<&str>) -> Option<String> {
    let raw = value?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn first_from_precedence(
    file_value: Option<&str>,
    env_value: Option<&str>,
    autodetect_value: Option<&str>,
) -> Option<String> {
    non_empty_opt(file_value)
        .or_else(|| non_empty_opt(env_value))
        .or_else(|| non_empty_opt(autodetect_value))
}

fn env_optional_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .and_then(|v| non_empty_opt(Some(v.as_str())))
}

fn env_optional_u64_strict(name: &str) -> Result<Option<u64>, String> {
    match std::env::var(name) {
        Ok(v) => {
            let t = v.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid numeric value in env {name}: `{t}`"))
            }
        }
        Err(_) => Ok(None),
    }
}

fn env_optional_u32_strict(name: &str) -> Result<Option<u32>, String> {
    match std::env::var(name) {
        Ok(v) => {
            let t = v.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u32>()
                    .map(Some)
                    .map_err(|_| format!("Invalid numeric value in env {name}: `{t}`"))
            }
        }
        Err(_) => Ok(None),
    }
}

fn env_optional_f64_strict(name: &str) -> Result<Option<f64>, String> {
    match std::env::var(name) {
        Ok(v) => {
            let t = v.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<f64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid numeric value in env {name}: `{t}`"))
            }
        }
        Err(_) => Ok(None),
    }
}

fn load_env_config() -> Result<EnvConfig, String> {
    Ok(EnvConfig {
        pipeline_root: env_optional_string("JARVIS_PIPELINE_ROOT"),
        pipeline_out_dir: env_optional_string("JARVIS_PIPELINE_OUT_DIR"),
        s2_api_key: env_optional_string("S2_API_KEY"),
        s2_min_interval_ms: env_optional_u64_strict("S2_MIN_INTERVAL_MS")?,
        s2_max_retries: env_optional_u32_strict("S2_MAX_RETRIES")?,
        s2_backoff_base_sec: env_optional_f64_strict("S2_BACKOFF_BASE_SEC")?,
    })
}

fn parse_u64_field_from_json(value: Option<&serde_json::Value>, key: &str) -> Result<Option<u64>, String> {
    match value {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| format!("Invalid {key}: must be a non-negative integer"))
            .map(Some),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {key}: `{t}` is not a valid integer"))
            }
        }
        Some(_) => Err(format!("Invalid {key}: must be number or numeric string")),
    }
}

fn parse_u32_field_from_json(value: Option<&serde_json::Value>, key: &str) -> Result<Option<u32>, String> {
    match parse_u64_field_from_json(value, key)? {
        None => Ok(None),
        Some(v) => u32::try_from(v)
            .map(Some)
            .map_err(|_| format!("Invalid {key}: value out of u32 range")),
    }
}

fn parse_f64_field_from_json(value: Option<&serde_json::Value>, key: &str) -> Result<Option<f64>, String> {
    match value {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| format!("Invalid {key}: must be a valid number"))
            .map(Some),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<f64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {key}: `{t}` is not a valid number"))
            }
        }
        Some(_) => Err(format!("Invalid {key}: must be number or numeric string")),
    }
}

fn read_desktop_config_file(path: &Path) -> Result<Option<DesktopConfigFile>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("Invalid config JSON at {}: {e}", path.display()))?;

    let obj = value
        .as_object()
        .ok_or_else(|| format!("Invalid config JSON at {}: root must be an object", path.display()))?;

    let cfg = DesktopConfigFile {
        JARVIS_PIPELINE_ROOT: obj
            .get("JARVIS_PIPELINE_ROOT")
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        JARVIS_PIPELINE_OUT_DIR: obj
            .get("JARVIS_PIPELINE_OUT_DIR")
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        S2_API_KEY: obj
            .get("S2_API_KEY")
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        S2_MIN_INTERVAL_MS: parse_u64_field_from_json(obj.get("S2_MIN_INTERVAL_MS"), "S2_MIN_INTERVAL_MS")?,
        S2_MAX_RETRIES: parse_u32_field_from_json(obj.get("S2_MAX_RETRIES"), "S2_MAX_RETRIES")?,
        S2_BACKOFF_BASE_SEC: parse_f64_field_from_json(obj.get("S2_BACKOFF_BASE_SEC"), "S2_BACKOFF_BASE_SEC")?,
    };

    Ok(Some(cfg))
}

fn validate_pipeline_root(source: &str, path: &Path) -> Result<PathBuf, String> {
    if is_pipeline_root(path) {
        return Ok(canonical_or_self(path));
    }
    Err(format!(
    "{source} pipeline root is invalid: {} (required: pyproject.toml, jarvis_cli.py, jarvis_core/)",
    path.display()
  ))
}

fn validate_out_dir_writable(path: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(path).map_err(|e| {
        format!(
            "out_dir is not writable (create_dir_all failed): {}: {e}",
            path.display()
        )
    })?;

    let canonical = canonical_or_self(path);
    let probe = canonical.join(".jarvis_desktop_write_probe.tmp");
    let mut f = fs::File::create(&probe)
        .map_err(|e| format!("out_dir is not writable (create probe failed): {}: {e}", canonical.display()))?;
    f.write_all(b"ok")
        .map_err(|e| format!("out_dir is not writable (write probe failed): {}: {e}", canonical.display()))?;
    let _ = fs::remove_file(&probe);
    Ok(canonical)
}

fn resolve_runtime_config(repo_root: &Path) -> Result<RuntimeConfig, String> {
    let cfg_path = config_file_path();
    let file_cfg_opt = read_desktop_config_file(&cfg_path)?;
    let file_cfg = file_cfg_opt.clone().unwrap_or_default();
    let env_cfg = load_env_config()?;

    let autodetect_candidate =
        find_pipeline_root_autodetect(repo_root).map(|p| p.to_string_lossy().to_string());
    let selected_root = first_from_precedence(
        file_cfg.JARVIS_PIPELINE_ROOT.as_deref(),
        env_cfg.pipeline_root.as_deref(),
        autodetect_candidate.as_deref(),
    );

    let pipeline_root = if let Some(root_text) = selected_root {
        let candidate = PathBuf::from(root_text);
        if non_empty_opt(file_cfg.JARVIS_PIPELINE_ROOT.as_deref()).is_some() {
            validate_pipeline_root("config file", &candidate)?
        } else if env_cfg.pipeline_root.is_some() {
            validate_pipeline_root("environment variable JARVIS_PIPELINE_ROOT", &candidate)?
        } else {
            validate_pipeline_root("auto-detected", &candidate)?
        }
    } else {
        return Err(format!(
      "Pipeline root not found. Configure JARVIS_PIPELINE_ROOT in {} or environment variable.",
      cfg_path.display()
    ));
    };

    let selected_out_dir = first_from_precedence(
        file_cfg.JARVIS_PIPELINE_OUT_DIR.as_deref(),
        env_cfg.pipeline_out_dir.as_deref(),
        Some("logs/runs"),
    )
    .unwrap_or_else(|| "logs/runs".to_string());

    let out_candidate = PathBuf::from(selected_out_dir);
    let out_abs = absolutize(&out_candidate, &pipeline_root);
    let out_abs = validate_out_dir_writable(&out_abs)?;

    let s2_api_key = non_empty_opt(file_cfg.S2_API_KEY.as_deref()).or(env_cfg.s2_api_key);
    let s2_min_interval_ms = file_cfg.S2_MIN_INTERVAL_MS.or(env_cfg.s2_min_interval_ms);
    let s2_max_retries = file_cfg.S2_MAX_RETRIES.or(env_cfg.s2_max_retries);
    let s2_backoff_base_sec = file_cfg.S2_BACKOFF_BASE_SEC.or(env_cfg.s2_backoff_base_sec);

    Ok(RuntimeConfig {
        config_file_path: cfg_path,
        config_file_loaded: file_cfg_opt.is_some(),
        pipeline_root,
        out_base_dir: out_abs,
        s2_api_key,
        s2_min_interval_ms,
        s2_max_retries,
        s2_backoff_base_sec,
    })
}

fn runtime_config_view_from_result(result: Result<RuntimeConfig, String>) -> RuntimeConfigView {
    match result {
        Ok(cfg) => RuntimeConfigView {
            ok: true,
            status: "ok".to_string(),
            message: "Runtime config resolved.".to_string(),
            config_file_path: cfg.config_file_path.to_string_lossy().to_string(),
            config_file_loaded: cfg.config_file_loaded,
            pipeline_root: cfg.pipeline_root.to_string_lossy().to_string(),
            out_dir: cfg.out_base_dir.to_string_lossy().to_string(),
            s2_api_key_set: cfg.s2_api_key.is_some(),
            s2_min_interval_ms: cfg.s2_min_interval_ms,
            s2_max_retries: cfg.s2_max_retries,
            s2_backoff_base_sec: cfg.s2_backoff_base_sec,
        },
        Err(e) => RuntimeConfigView {
            ok: false,
            status: "missing_dependency".to_string(),
            message: e,
            config_file_path: config_file_path().to_string_lossy().to_string(),
            config_file_loaded: false,
            pipeline_root: "".to_string(),
            out_dir: "".to_string(),
            s2_api_key_set: false,
            s2_min_interval_ms: None,
            s2_max_retries: None,
            s2_backoff_base_sec: None,
        },
    }
}

fn preflight_item(name: &str, ok: bool, detail: String, fix_hint: &str) -> PreflightCheckItem {
    PreflightCheckItem {
        name: name.to_string(),
        ok,
        detail,
        fix_hint: fix_hint.to_string(),
    }
}

fn run_preflight_checks() -> PreflightResult {
    let root = repo_root();
    let cfg_path = config_file_path();

    let mut checks = Vec::new();

    let file_cfg_res = read_desktop_config_file(&cfg_path);
    let file_cfg = match file_cfg_res {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            checks.push(preflight_item(
                "config_file",
                false,
                e,
                "Fix JSON format in config file or recreate template from app.",
            ));
            DesktopConfigFile::default()
        }
    };

    let env_cfg_res = load_env_config();
    let env_cfg = match env_cfg_res {
        Ok(v) => v,
        Err(e) => {
            checks.push(preflight_item(
                "environment",
                false,
                e,
                "Remove invalid numeric env values (S2_*).",
            ));
            EnvConfig::default()
        }
    };

    let autodetect_candidate =
        find_pipeline_root_autodetect(&root).map(|p| p.to_string_lossy().to_string());
    let selected_root = first_from_precedence(
        file_cfg.JARVIS_PIPELINE_ROOT.as_deref(),
        env_cfg.pipeline_root.as_deref(),
        autodetect_candidate.as_deref(),
    );

    let mut pipeline_root_valid: Option<PathBuf> = None;
    match selected_root {
        None => checks.push(preflight_item(
            "pipeline_root",
            false,
            format!(
                "Pipeline root is not resolved. config path: {}",
                cfg_path.display()
            ),
            "Set JARVIS_PIPELINE_ROOT in config or environment.",
        )),
        Some(root_text) => {
            let candidate = PathBuf::from(&root_text);
            if !candidate.exists() {
                checks.push(preflight_item(
                    "pipeline_root",
                    false,
                    format!("Pipeline root does not exist: {}", candidate.display()),
                    "Set existing pipeline root path.",
                ));
            } else {
                match validate_pipeline_root("resolved", &candidate) {
                    Ok(p) => {
                        checks.push(preflight_item(
                            "pipeline_root",
                            true,
                            format!("Resolved: {}", p.display()),
                            "",
                        ));
                        pipeline_root_valid = Some(p);
                    }
                    Err(e) => checks.push(preflight_item(
                        "pipeline_root",
                        false,
                        e,
                        "Ensure pipeline root has pyproject.toml, jarvis_cli.py, jarvis_core/.",
                    )),
                }
            }
        }
    }

    if let Some(ref pipeline_root) = pipeline_root_valid {
        let selected_out_dir = first_from_precedence(
            file_cfg.JARVIS_PIPELINE_OUT_DIR.as_deref(),
            env_cfg.pipeline_out_dir.as_deref(),
            Some("logs/runs"),
        )
        .unwrap_or_else(|| "logs/runs".to_string());
        let out_abs = absolutize(&PathBuf::from(selected_out_dir), pipeline_root);
        match validate_out_dir_writable(&out_abs) {
            Ok(canonical) => checks.push(preflight_item(
                "out_dir",
                true,
                format!("Writable: {}", canonical.display()),
                "",
            )),
            Err(e) => checks.push(preflight_item(
                "out_dir",
                false,
                e,
                "Fix JARVIS_PIPELINE_OUT_DIR or directory permissions.",
            )),
        }

        let (python_cmd, warnings) = choose_python(&root, pipeline_root);
        match check_python_runnable(&python_cmd, pipeline_root) {
            Ok(_) => {
                let mut detail = format!("python executable: {python_cmd}");
                if !warnings.is_empty() {
                    detail = format!("{detail}; {}", warnings.join(" | "));
                }
                checks.push(preflight_item("python", true, detail, ""));
            }
            Err(e) => checks.push(preflight_item(
                "python",
                false,
                e,
                "Prepare python venv under src-tauri/.venv or pipeline/.venv.",
            )),
        }

        let mut marker_missing = Vec::new();
        for marker in ["pyproject.toml", "jarvis_cli.py", "jarvis_core"] {
            let exists = pipeline_root.join(marker).exists();
            if !exists {
                marker_missing.push(marker.to_string());
            }
        }
        if marker_missing.is_empty() {
            checks.push(preflight_item(
                "pipeline_markers",
                true,
                format!("markers OK at {}", pipeline_root.display()),
                "",
            ));
        } else {
            checks.push(preflight_item(
                "pipeline_markers",
                false,
                format!("missing markers: {}", marker_missing.join(", ")),
                "Point pipeline_root to a valid jarvis-ml-pipeline checkout.",
            ));
        }
    } else {
        checks.push(preflight_item(
            "out_dir",
            false,
            "pipeline_root unresolved; out_dir check skipped".to_string(),
            "Fix pipeline_root first.",
        ));
        checks.push(preflight_item(
            "python",
            false,
            "pipeline_root unresolved; python check skipped".to_string(),
            "Fix pipeline_root first.",
        ));
        checks.push(preflight_item(
            "pipeline_markers",
            false,
            "pipeline_root unresolved; marker check skipped".to_string(),
            "Fix pipeline_root first.",
        ));
    }

    let ok = checks.iter().all(|c| c.ok);
    PreflightResult { ok, checks }
}

fn ensure_config_file_template(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory {}: {e}",
                parent.to_string_lossy()
            )
        })?;
    }
    let template = r#"{
  "JARVIS_PIPELINE_ROOT": "C:\\Users\\<user>\\Documents\\jarvis-work\\jarvis-ml-pipeline",
  "JARVIS_PIPELINE_OUT_DIR": "logs/runs",
  "S2_API_KEY": "",
  "S2_MIN_INTERVAL_MS": 1000,
  "S2_MAX_RETRIES": 6,
  "S2_BACKOFF_BASE_SEC": 0.5
}
"#;
    std::fs::write(path, template)
        .map_err(|e| format!("Failed to create config template {}: {e}", path.display()))
}

fn extract_retry_after_seconds(raw: &str) -> Option<f64> {
    let lower = raw.to_lowercase();
    for needle in [
        "retry-after",
        "retry_after",
        "retry after",
        "wait_seconds=",
        "wait_seconds:",
    ] {
        if let Some(idx) = lower.find(needle) {
            let start = idx + needle.len();
            if let Some(value) = parse_first_float(&raw[start..]) {
                return Some(value);
            }
        }
    }
    None
}

fn parse_first_float(input: &str) -> Option<f64> {
    let mut found = String::new();
    let mut started = false;
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            found.push(ch);
            started = true;
            continue;
        }
        if started {
            break;
        }
    }
    if found.is_empty() {
        None
    } else {
        found.parse::<f64>().ok()
    }
}

fn choose_python(repo_root: &Path, pipeline_root: &Path) -> (String, Vec<String>) {
    let mut warnings = Vec::new();
    let tauri_venv = repo_root
        .join("src-tauri")
        .join(".venv")
        .join("Scripts")
        .join("python.exe");
    if tauri_venv.is_file() {
        return (tauri_venv.to_string_lossy().to_string(), warnings);
    }

    let pipeline_venv = pipeline_root
        .join(".venv")
        .join("Scripts")
        .join("python.exe");
    if pipeline_venv.is_file() {
        return (pipeline_venv.to_string_lossy().to_string(), warnings);
    }

    warnings.push("Project venv python not found. Falling back to system `python`.".to_string());
    ("python".to_string(), warnings)
}

fn canonicalize_existing_dir(path: &Path, rule: &str) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{rule}: path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!(
            "{rule}: path is not a directory: {}",
            path.display()
        ));
    }
    path.canonicalize()
        .map_err(|e| format!("{rule}: canonicalize failed for {}: {e}", path.display()))
}

fn has_disallowed_windows_prefix(raw: &str) -> bool {
    // Block UNC/device-prefixed inputs early to avoid path traversal quirks on Windows.
    if !cfg!(windows) {
        return false;
    }
    let t = raw.trim();
    t.starts_with(r"\\?\")
        || t.starts_with(r"\\.\")
        || t.starts_with(r"\\")
        || t.to_ascii_lowercase().starts_with(r"\\?\unc\")
}

fn check_python_runnable(python_cmd: &str, pipeline_root: &Path) -> Result<(), String> {
    let out = Command::new(python_cmd)
        .arg("--version")
        .current_dir(pipeline_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run python preflight (`{python_cmd} --version`): {e}"))?;

    if out.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    Err(format!(
        "python preflight failed (`{python_cmd} --version`). stdout={stdout} stderr={stderr}"
    ))
}

fn read_status(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let all = format!("{stdout}\n{stderr}").to_lowercase();
    let has_retry_signal = all.contains("status: needs_retry")
        || all.contains("\"status\": \"needs_retry\"")
        || all.contains("s2_retry_exhausted")
        || all.contains("status=429")
        || all.contains(" 429 ")
        || all.contains("http 429")
        || all.contains("retry exhausted");
    if has_retry_signal {
        return "needs_retry".to_string();
    }

    if exit_code != 0 {
        return "error".to_string();
    }
    "ok".to_string()
}

fn first_non_empty_line(raw: &str) -> Option<String> {
    raw.lines()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
}

fn build_status_message(
    status: &str,
    stdout: &str,
    stderr: &str,
    retry_after_sec: Option<f64>,
) -> String {
    if status == "needs_retry" {
        if let Some(sec) = retry_after_sec {
            return format!(
        "Semantic Scholar is rate-limited or temporarily unavailable. Retry after {:.1} sec.",
        sec
      );
        }
        return "Semantic Scholar request needs retry due to transient API/network failure."
            .to_string();
    }
    if status == "error" {
        return first_non_empty_line(stderr)
            .or_else(|| first_non_empty_line(stdout))
            .unwrap_or_else(|| "Pipeline execution failed.".to_string());
    }
    if status == "missing_dependency" {
        return first_non_empty_line(stderr)
            .unwrap_or_else(|| "Missing dependency detected.".to_string());
    }
    "Pipeline run completed.".to_string()
}

fn parse_f64_loose(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn inspect_retry_fields(value: &serde_json::Value) -> (bool, Option<f64>) {
    let mut needs_retry = false;
    let mut retry_after: Option<f64> = None;

    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = k.to_lowercase();
                if key == "status" {
                    if let Some(s) = v.as_str() {
                        if s.eq_ignore_ascii_case("needs_retry") {
                            needs_retry = true;
                        }
                    }
                }
                if key == "http_status" || key == "error_code" {
                    if let Some(n) = v.as_i64() {
                        if n == 429 {
                            needs_retry = true;
                        }
                    } else if let Some(s) = v.as_str() {
                        if s.trim() == "429" {
                            needs_retry = true;
                        }
                    }
                }
                if key == "retry_after_seconds" || key == "retry_after" {
                    if let Some(sec) = parse_f64_loose(v) {
                        retry_after = Some(sec.max(0.0));
                        needs_retry = true;
                    }
                }

                let (nested_retry, nested_after) = inspect_retry_fields(v);
                if nested_retry {
                    needs_retry = true;
                }
                if retry_after.is_none() {
                    retry_after = nested_after;
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                let (nested_retry, nested_after) = inspect_retry_fields(v);
                if nested_retry {
                    needs_retry = true;
                }
                if retry_after.is_none() {
                    retry_after = nested_after;
                }
            }
        }
        _ => {}
    }

    (needs_retry, retry_after)
}

fn infer_newest_run_id_after(out_dir: &Path, started_ms: u128) -> Option<String> {
    let mut candidates: Vec<(u64, String)> = Vec::new();
    let entries = fs::read_dir(out_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let ts = modified_epoch_ms(&path);
        if u128::from(ts) + 1 < started_ms {
            continue;
        }
        let run_id = path.file_name()?.to_string_lossy().to_string();
        candidates.push((ts, run_id));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.first().map(|(_, run_id)| run_id.clone())
}

fn sort_jobs_for_display(rows: &mut [JobRecord]) {
    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.job_id.cmp(&b.job_id))
    });
}

fn sort_runs_for_display(rows: &mut [RunListItem]) {
    rows.sort_by(|a, b| {
        b.mtime_epoch_ms
            .cmp(&a.mtime_epoch_ms)
            .then_with(|| a.run_id.cmp(&b.run_id))
    });
}

fn classify_job_status(
    run_result: &RunResult,
    runtime: &RuntimeConfig,
    run_id: &str,
    canceled: bool,
) -> (JobStatus, Option<f64>, Option<String>) {
    if canceled {
        return (JobStatus::Canceled, None, None);
    }

    let run_dir = runtime.out_base_dir.join(run_id);
    let result_path = run_dir.join("result.json");
    if result_path.exists() {
        if let Ok(raw) = fs::read_to_string(&result_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                let (needs_retry, retry_after) = inspect_retry_fields(&v);
                if needs_retry {
                    return (JobStatus::NeedsRetry, retry_after, Some("needs retry from result.json".to_string()));
                }
                if let Some(status) = v.get("status").and_then(|x| x.as_str()) {
                    if status.eq_ignore_ascii_case("ok") {
                        return (JobStatus::Succeeded, None, None);
                    }
                }
            }
        }
    }

    if run_result.status == "needs_retry" {
        return (
            JobStatus::NeedsRetry,
            run_result.retry_after_sec,
            Some(run_result.message.clone()),
        );
    }

    if run_result.ok {
        (JobStatus::Succeeded, None, None)
    } else {
        (JobStatus::Failed, None, Some(run_result.message.clone()))
    }
}

fn apply_job_result(
    state: &Arc<Mutex<JobRuntimeState>>,
    jobs_path: &Path,
    job_id: &str,
    run_result: &RunResult,
) -> Result<(), String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let settings = load_settings(&runtime.out_base_dir).unwrap_or_default();
    let (run_id_for_index, status_for_index);

    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        let idx = guard
            .jobs
            .iter()
            .position(|j| j.job_id == job_id)
            .ok_or_else(|| format!("job not found: {job_id}"))?;

        let mut run_id = guard.jobs[idx].run_id.clone();
        if run_id.is_none() && !run_result.run_id.trim().is_empty() {
            run_id = Some(run_result.run_id.clone());
        }
        if run_id.is_none() {
            run_id = infer_newest_run_id_after(&runtime.out_base_dir, now_epoch_ms());
        }

        let canceled = guard.cancel_requested.contains(job_id);
        let resolved_run_id = run_id.clone().unwrap_or_default();
        let (status, retry_after, err) = classify_job_status(run_result, &runtime, &resolved_run_id, canceled);

        let updated_at = now_epoch_ms_string();
        let retry_at = if status == JobStatus::NeedsRetry {
            let next_attempt_idx = guard.jobs[idx].auto_retry_attempt_count.saturating_add(1);
            Some(compute_next_retry_at_ms(
                now_epoch_ms(),
                retry_after,
                next_attempt_idx,
                &settings,
            ))
        } else {
            None
        };

        guard.jobs[idx].status = status;
        guard.jobs[idx].updated_at = updated_at;
        guard.jobs[idx].run_id = run_id;
        guard.jobs[idx].retry_after_seconds = retry_after;
        guard.jobs[idx].retry_at = retry_at;
        guard.jobs[idx].last_error = err;

        run_id_for_index = guard.jobs[idx].run_id.clone();
        status_for_index = Some(guard.jobs[idx].status.clone());

        guard.running_job_id = None;
        guard.running_pid = None;
        guard.cancel_requested.remove(job_id);
    }

    persist_state(state, jobs_path)?;

    if let (Some(run_id), Some(status)) = (run_id_for_index, status_for_index) {
        if status == JobStatus::Succeeded || status == JobStatus::Failed || status == JobStatus::NeedsRetry {
            let _ = upsert_library_run(&runtime.out_base_dir, &run_id);
        }
    }

    let _ = reconcile_pipelines_with_jobs(&runtime.out_base_dir, state, jobs_path, Some(job_id));
    let _ = start_job_worker_if_needed();

    Ok(())
}

fn apply_mock_transition(
    job: &mut JobRecord,
    status: JobStatus,
    run_id: Option<String>,
    last_error: Option<String>,
    retry_after_seconds: Option<f64>,
) {
    job.status = status;
    job.updated_at = now_epoch_ms_string();
    job.run_id = run_id;
    job.last_error = last_error;
    job.retry_after_seconds = retry_after_seconds;
    job.retry_at = retry_after_seconds.map(|sec| {
        let at = now_epoch_ms() as f64 + sec.max(0.0) * 1000.0;
        format!("{:.0}", at)
    });
}

#[tauri::command]
fn library_reindex(full: Option<bool>) -> Result<LibraryReindexResult, String> {
    let _full = full.unwrap_or(false);
    let (runtime, _) = runtime_and_jobs_path()?;
    let out_dir = runtime.out_base_dir.clone();
    let existing = load_library_records_cached(&out_dir, false)?;
    let records = build_library_records(&out_dir, &existing)?;
    let count_runs = records.iter().map(|r| r.runs.len()).sum();
    write_library_records(&out_dir, &records)?;
    Ok(LibraryReindexResult {
        count_records: records.len(),
        count_runs,
        updated_at: Utc::now().to_rfc3339(),
    })
}

#[tauri::command]
fn library_reload() -> Result<LibraryReindexResult, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let records = load_library_records_cached(&runtime.out_base_dir, true)?;
    let count_runs = records.iter().map(|r| r.runs.len()).sum();
    Ok(LibraryReindexResult {
        count_records: records.len(),
        count_runs,
        updated_at: Utc::now().to_rfc3339(),
    })
}

#[tauri::command]
fn library_list(filters: Option<LibraryListFilter>) -> Result<Vec<LibraryRecordSummary>, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let records = load_library_records_cached(&runtime.out_base_dir, false)?;
    let f = filters.unwrap_or_default();
    let query = f.query.unwrap_or_default().to_lowercase();
    let status = f.status.unwrap_or_default().to_lowercase();
    let kind = f.kind.unwrap_or_default().to_lowercase();
    let tag = f.tag.unwrap_or_default().to_lowercase();

    let mut out = Vec::new();
    for rec in records {
        if !query.is_empty() {
            let hay = format!(
                "{} {}",
                rec.canonical_id.clone().unwrap_or_default().to_lowercase(),
                rec.title.clone().unwrap_or_default().to_lowercase()
            );
            if !hay.contains(&query) {
                continue;
            }
        }
        if !status.is_empty() && rec.last_status.to_lowercase() != status {
            continue;
        }
        if !kind.is_empty() {
            let k = rec.source_kind.clone().unwrap_or_default().to_lowercase();
            if k != kind {
                continue;
            }
        }
        if !tag.is_empty() {
            let has = rec.tags.iter().any(|t| t.to_lowercase() == tag);
            if !has {
                continue;
            }
        }
        if let Some(from) = f.year_from {
            if rec.year.unwrap_or(i32::MIN) < from {
                continue;
            }
        }
        if let Some(to) = f.year_to {
            if rec.year.unwrap_or(i32::MAX) > to {
                continue;
            }
        }

        out.push(LibraryRecordSummary {
            paper_key: rec.paper_key,
            canonical_id: rec.canonical_id,
            title: rec.title,
            source_kind: rec.source_kind,
            primary_viz: rec.primary_viz,
            last_status: rec.last_status,
            last_run_id: rec.last_run_id,
            updated_at: rec.updated_at,
            tags: rec.tags,
        });
    }
    Ok(out)
}

#[tauri::command]
fn library_search(query: String, opts: Option<LibrarySearchOpts>) -> Result<Vec<LibrarySearchResult>, String> {
    let tokens = tokenize_query(&query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let (runtime, _) = runtime_and_jobs_path()?;
    let records = load_library_records_cached(&runtime.out_base_dir, false)?;
    let options = opts.unwrap_or_default();
    let status_filter = options.status.unwrap_or_default().to_lowercase();
    let kind_filter = options.kind.unwrap_or_default().to_lowercase();
    let tag_filter = options.tag.unwrap_or_default().to_lowercase();
    let limit = options.limit.unwrap_or(200).clamp(1, 1000);

    let mut out = Vec::new();
    for rec in records {
        if !status_filter.is_empty() && rec.last_status.to_lowercase() != status_filter {
            continue;
        }
        if !kind_filter.is_empty() {
            let k = rec.source_kind.clone().unwrap_or_default().to_lowercase();
            if k != kind_filter {
                continue;
            }
        }
        if !tag_filter.is_empty() {
            let has = rec.tags.iter().any(|t| t.to_lowercase() == tag_filter);
            if !has {
                continue;
            }
        }

        let (score, highlights, matched_any) = score_library_record(&rec, &tokens);
        if !matched_any {
            continue;
        }

        out.push(LibrarySearchResult {
            paper_key: rec.paper_key,
            canonical_id: rec.canonical_id,
            title: rec.title,
            tags: rec.tags,
            primary_viz: rec.primary_viz,
            last_status: rec.last_status,
            last_run_id: rec.last_run_id,
            score,
            highlights: if highlights.is_empty() { None } else { Some(highlights) },
            updated_at: rec.updated_at,
        });
    }

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.paper_key.cmp(&b.paper_key))
    });
    if out.len() > limit {
        out.truncate(limit);
    }
    Ok(out)
}

#[tauri::command]
fn library_get(paper_key: String) -> Result<LibraryRecord, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let records = load_library_records_cached(&runtime.out_base_dir, false)?;
    records
        .into_iter()
        .find(|r| r.paper_key == paper_key)
        .ok_or_else(|| format!("paper_key not found: {paper_key}"))
}

#[tauri::command]
fn library_set_tags(paper_key: String, tags: Vec<String>) -> Result<LibraryRecord, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let mut records = load_library_records_cached(&runtime.out_base_dir, false)?;
    let idx = records
        .iter()
        .position(|r| r.paper_key == paper_key)
        .ok_or_else(|| format!("paper_key not found: {paper_key}"))?;

    let mut cleaned: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    cleaned.sort();
    cleaned.dedup();

    records[idx].tags = cleaned;
    records[idx].updated_at = Utc::now().to_rfc3339();
    let out = records[idx].clone();
    write_library_records(&runtime.out_base_dir, &records)?;
    Ok(out)
}

#[tauri::command]
fn library_stats() -> Result<LibraryStats, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let records = load_library_records_cached(&runtime.out_base_dir, false)?;

    let mut status_counts = serde_json::Map::new();
    let mut kind_counts = serde_json::Map::new();
    let mut total_runs = 0usize;

    for rec in &records {
        total_runs += rec.runs.len();
        let status_key = rec.last_status.clone();
        let v = status_counts
            .entry(status_key)
            .or_insert(serde_json::Value::from(0));
        let n = v.as_i64().unwrap_or(0) + 1;
        *v = serde_json::Value::from(n);

        let kind_key = rec
            .source_kind
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let kv = kind_counts
            .entry(kind_key)
            .or_insert(serde_json::Value::from(0));
        let kn = kv.as_i64().unwrap_or(0) + 1;
        *kv = serde_json::Value::from(kn);
    }

    Ok(LibraryStats {
        total_papers: records.len(),
        total_runs,
        status_counts: serde_json::Value::Object(status_counts),
        kind_counts: serde_json::Value::Object(kind_counts),
    })
}

fn start_job_worker_if_needed() -> Result<(), String> {
    let (state, jobs_path) = init_job_runtime()?;
    static WORKER_STARTED: OnceLock<()> = OnceLock::new();
    if WORKER_STARTED.get().is_some() {
        return Ok(());
    }

    let worker_state = state.clone();
    let worker_jobs_path = jobs_path.clone();
    thread::spawn(move || loop {
        let next_job = {
            let mut guard = match worker_state.lock() {
                Ok(g) => g,
                Err(_) => {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            if guard.running_job_id.is_some() {
                None
            } else {
                let next_idx = guard.jobs.iter().position(|j| j.status == JobStatus::Queued);
                if let Some(idx) = next_idx {
                    guard.jobs[idx].status = JobStatus::Running;
                    guard.jobs[idx].attempt = guard.jobs[idx].attempt.saturating_add(1);
                    guard.jobs[idx].updated_at = now_epoch_ms_string();
                    guard.running_job_id = Some(guard.jobs[idx].job_id.clone());
                    Some(guard.jobs[idx].clone())
                } else {
                    None
                }
            }
        };

        if let Some(job) = next_job {
            let _ = persist_state(&worker_state, &worker_jobs_path);

            let (argv, normalized_params) = match build_template_args(&job.template_id, &job.canonical_id, &job.params) {
                Ok(v) => v,
                Err(e) => {
                    let mut failed = RunResult {
                        ok: false,
                        exit_code: 1,
                        stdout: "".to_string(),
                        stderr: e.clone(),
                        run_id: "".to_string(),
                        run_dir: "".to_string(),
                        status: "error".to_string(),
                        message: e,
                        retry_after_sec: None,
                    };
                    failed.run_id = make_run_id();
                    let _ = apply_job_result(&worker_state, &worker_jobs_path, &job.job_id, &failed);
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };

            let result = execute_pipeline_task(
                argv,
                job.template_id.clone(),
                job.canonical_id.clone(),
                normalized_params,
                Some((worker_state.clone(), job.job_id.clone())),
            );
            let _ = apply_job_result(&worker_state, &worker_jobs_path, &job.job_id, &result);
            thread::sleep(Duration::from_millis(100));
        } else {
            thread::sleep(Duration::from_millis(500));
        }
    });

    let _ = WORKER_STARTED.set(());
    Ok(())
}

fn missing_dependency(run_id: String, message: String) -> RunResult {
    let user_message = first_non_empty_line(&message)
        .unwrap_or_else(|| "Missing dependency detected. Check stderr for details.".to_string());
    RunResult {
        ok: false,
        exit_code: 1,
        stdout: "".to_string(),
        stderr: message,
        run_id,
        run_dir: "".to_string(),
        status: "missing_dependency".to_string(),
        message: user_message,
        retry_after_sec: None,
    }
}

fn validate_run_id_component(run_id: &str) -> Result<String, String> {
    let trimmed = run_id.trim();
    if trimmed.is_empty() {
        return Err("run_id is empty".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("run_id is invalid".to_string());
    }
    if trimmed.contains('\\') || trimmed.contains('/') {
        return Err("run_id must not contain path separators".to_string());
    }
    Ok(trimmed.to_string())
}

fn parse_status_from_result(path: &Path) -> String {
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return "unknown".to_string(),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return "unknown".to_string(),
    };

    if let Some(v) = value.get("status").and_then(|v| v.as_str()) {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }

    if let Some(ok) = value.get("ok").and_then(|v| v.as_bool()) {
        if ok {
            return "ok".to_string();
        }
        return "error".to_string();
    }

    "unknown".to_string()
}

fn parse_paper_id_from_input(path: &Path) -> String {
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return "unknown".to_string(),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return "unknown".to_string(),
    };

    if let Some(v) = value
        .get("desktop")
        .and_then(|v| v.get("canonical_id"))
        .and_then(|v| v.as_str())
    {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }

    if let Some(v) = value.get("paper_id").and_then(|v| v.as_str()) {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(v) = value.get("id").and_then(|v| v.as_str()) {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(v) = value
        .get("request")
        .and_then(|v| v.get("paper_id"))
        .and_then(|v| v.as_str())
    {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }

    "unknown".to_string()
}

fn known_artifact_specs() -> Vec<ArtifactSpec> {
    vec![
        ArtifactSpec {
            name: "tree.md",
            rel_path: "paper_graph/tree/tree.md",
            legacy_key: "tree_md",
        },
        ArtifactSpec {
            name: "result.json",
            rel_path: "result.json",
            legacy_key: "result_json",
        },
        ArtifactSpec {
            name: "input.json",
            rel_path: "input.json",
            legacy_key: "input_json",
        },
        ArtifactSpec {
            name: "stdout.log",
            rel_path: "stdout.log",
            legacy_key: "stdout_log",
        },
        ArtifactSpec {
            name: "stderr.log",
            rel_path: "stderr.log",
            legacy_key: "stderr_log",
        },
    ]
}

fn rel_path_to_pathbuf(rel_path: &str) -> PathBuf {
    let mut buf = PathBuf::new();
    for seg in rel_path.split('/') {
        if !seg.trim().is_empty() {
            buf.push(seg);
        }
    }
    buf
}

fn normalized_rel_path(root: &Path, target: &Path) -> Option<String> {
    let rel = target.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn detect_artifact_kind_by_name(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with(".md") {
        "markdown".to_string()
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        "html".to_string()
    } else if lower.ends_with(".json") {
        "json".to_string()
    } else if lower.ends_with(".log") || lower.ends_with(".txt") {
        "text".to_string()
    } else {
        "unknown".to_string()
    }
}

fn is_probable_graph_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("graph") || lower.contains("map") || lower.contains("viz")
}

fn is_probable_graph_json(path: &Path, name: &str, size_bytes: Option<u64>) -> bool {
    if !name.to_lowercase().ends_with(".json") {
        return false;
    }
    if is_probable_graph_name(name) {
        return true;
    }

    let size = size_bytes.unwrap_or(0);
    if size == 0 || size > 256 * 1024 {
        return false;
    }
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let v = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(_) => return false,
    };

    match v {
        serde_json::Value::Object(map) => {
            let has_nodes = map.contains_key("nodes");
            let has_edges = map.contains_key("edges");
            let has_map = map.contains_key("map") || map.contains_key("graph");
            (has_nodes && has_edges) || has_map
        }
        _ => false,
    }
}

fn classify_artifact_kind(path: &Path, name: &str, size_bytes: Option<u64>) -> String {
    let base = detect_artifact_kind_by_name(name);
    if base == "json" && is_probable_graph_json(path, name, size_bytes) {
        return "graph_json".to_string();
    }
    base
}

fn select_primary_viz_artifact(items: &[ArtifactItem]) -> Option<PrimaryVizRef> {
    let mut cands: Vec<&ArtifactItem> = items
        .iter()
        .filter(|a| a.kind == "html" || a.kind == "graph_json")
        .collect();

    cands.sort_by(|a, b| {
        let pa = if a.kind == "html" { 0 } else { 1 };
        let pb = if b.kind == "html" { 0 } else { 1 };
        pa.cmp(&pb)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });

    let item = cands.first()?;
    Some(PrimaryVizRef {
        name: item.name.clone(),
        kind: item.kind.clone(),
    })
}

fn find_ascii_nocase(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    for i in 0..=h.len() - n.len() {
        let mut ok = true;
        for j in 0..n.len() {
            if !h[i + j].eq_ignore_ascii_case(&n[j]) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }
    None
}

fn strip_script_tags(html: &str) -> (String, bool) {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut removed = false;

    loop {
        let Some(start) = find_ascii_nocase(rest, "<script") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        if let Some(end_rel) = find_ascii_nocase(after_start, "</script>") {
            let cut = end_rel + "</script>".len();
            rest = &after_start[cut..];
            removed = true;
        } else {
            removed = true;
            break;
        }
    }

    (out, removed)
}

fn contains_external_refs(html: &str) -> bool {
    let lower = html.to_lowercase();
    [
        "src=\"http://",
        "src=\"https://",
        "src=\"//",
        "src='http://",
        "src='https://",
        "src='//",
        "href=\"http://",
        "href=\"https://",
        "href=\"//",
        "href='http://",
        "href='https://",
        "href='//",
        "href=\"javascript:",
        "href='javascript:",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

fn build_sandboxed_html(raw: &str) -> (String, Vec<String>) {
    let (without_scripts, removed_scripts) = strip_script_tags(raw);
    let has_external_refs = contains_external_refs(&without_scripts);

    let mut warnings = Vec::new();
    if removed_scripts {
        warnings.push("scripts were removed for safe preview".to_string());
    }
    if has_external_refs {
        warnings.push("external refs detected; CSP blocks network/navigation".to_string());
    }

    let csp = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; connect-src 'none'; frame-ancestors 'none'; form-action 'none'; navigate-to 'none'";
    let banner = if warnings.is_empty() {
        String::new()
    } else {
        format!(
            "<div style=\"padding:8px;border:1px solid #d6b36a;background:#fff8e6;color:#6f4a00;font:12px sans-serif;\">{}</div>",
            warnings.join(" | ")
        )
    };

    let content = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"></head><body>{}{}</body></html>",
        csp,
        banner,
        without_scripts
    );
    (content, warnings)
}

fn as_stringish(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Object(m) => {
            for key in ["id", "node_id", "key", "canonical_id"] {
                if let Some(v) = m.get(key).and_then(as_stringish) {
                    return Some(v);
                }
            }
            None
        }
        _ => None,
    }
}

fn get_first_string_field<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key).and_then(as_stringish) {
            return Some(v);
        }
    }
    None
}

fn get_optional_i32_field(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<i32> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            match v {
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        if (1900..=2200).contains(&(i as i32)) {
                            return Some(i as i32);
                        }
                    }
                }
                serde_json::Value::String(s) => {
                    if let Ok(i) = s.trim().parse::<i32>() {
                        if (1900..=2200).contains(&i) {
                            return Some(i);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn get_optional_f64_field(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            match v {
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        return Some(f);
                    }
                }
                serde_json::Value::String(s) => {
                    if let Ok(f) = s.trim().parse::<f64>() {
                        return Some(f);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn extract_graph_arrays<'a>(root: &'a serde_json::Value) -> (Option<&'a Vec<serde_json::Value>>, Option<&'a Vec<serde_json::Value>>, Vec<String>) {
    let mut warnings = Vec::new();

    if let Some(obj) = root.as_object() {
        let out_nodes = obj.get("nodes").and_then(|v| v.as_array());
        let out_edges = obj.get("edges").and_then(|v| v.as_array());
        if out_nodes.is_some() || out_edges.is_some() {
            return (out_nodes, out_edges, warnings);
        }

        for container_key in ["data", "graph"] {
            if let Some(container) = obj.get(container_key).and_then(|v| v.as_object()) {
                let out_nodes = container.get("nodes").and_then(|v| v.as_array());
                let out_edges = container.get("edges").and_then(|v| v.as_array());
                if out_nodes.is_some() || out_edges.is_some() {
                    warnings.push(format!("graph arrays detected in nested key `{container_key}`"));
                    return (out_nodes, out_edges, warnings);
                }
            }
        }
    }

    warnings.push("graph schema not recognized; fallback summary mode".to_string());
    (None, None, warnings)
}

fn parse_graph_json_internal(content: &str) -> Result<GraphParseResult, String> {
    let root: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("invalid graph json: {e}"))?;

    let mut top_level_keys = root
        .as_object()
        .map(|m| {
            let mut keys: Vec<String> = m.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();
    if top_level_keys.is_empty() {
        top_level_keys = vec!["<non-object-root>".to_string()];
    }

    let (nodes_raw, edges_raw, mut warnings) = extract_graph_arrays(&root);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    if let Some(arr) = nodes_raw {
        for (idx, n) in arr.iter().enumerate() {
            let (id, label, node_type, year, score) = if let Some(obj) = n.as_object() {
                let id = get_first_string_field(
                    obj,
                    &["id", "node_id", "paper_id", "key", "canonical_id"],
                )
                .unwrap_or_else(|| format!("node:{idx}"));
                let label = get_first_string_field(obj, &["label", "title", "name"]);
                let node_type = get_first_string_field(obj, &["type", "kind", "node_type"]);
                let year = get_optional_i32_field(obj, &["year", "publication_year", "published_year"]);
                let score = get_optional_f64_field(obj, &["score", "weight", "rank"]);
                (id, label, node_type, year, score)
            } else {
                (format!("node:{idx}"), None, None, None, None)
            };

            nodes.push(GraphNodeNormalized {
                id,
                label,
                node_type,
                year,
                score,
                raw: n.clone(),
            });
        }
    }

    if let Some(arr) = edges_raw {
        for e in arr {
            let Some(obj) = e.as_object() else {
                warnings.push("edge item skipped: expected object".to_string());
                continue;
            };

            let source = get_first_string_field(obj, &["source", "from", "src", "u", "tail"]);
            let target = get_first_string_field(obj, &["target", "to", "dst", "v", "head"]);
            let (Some(source), Some(target)) = (source, target) else {
                warnings.push("edge item skipped: missing source/target".to_string());
                continue;
            };

            let edge_type = get_first_string_field(obj, &["type", "kind", "edge_type"]);
            let weight = get_optional_f64_field(obj, &["weight", "score", "value"]);
            edges.push(GraphEdgeNormalized {
                source,
                target,
                edge_type,
                weight,
                raw: e.clone(),
            });
        }
    }

    nodes.sort_by(|a, b| {
        a.id.cmp(&b.id).then_with(|| {
            a.label
                .clone()
                .unwrap_or_default()
                .cmp(&b.label.clone().unwrap_or_default())
        })
    });
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
            .then_with(|| {
                a.edge_type
                    .clone()
                    .unwrap_or_default()
                    .cmp(&b.edge_type.clone().unwrap_or_default())
            })
    });

    Ok(GraphParseResult {
        nodes: nodes.clone(),
        edges: edges.clone(),
        stats: GraphParseStats {
            nodes_count: nodes.len(),
            edges_count: edges.len(),
            top_level_keys,
        },
        warnings,
    })
}

#[tauri::command]
fn parse_graph_json(content: String) -> Result<GraphParseResult, String> {
    parse_graph_json_internal(&content)
}

fn kind_priority(kind: &str) -> i32 {
    match kind {
        "markdown" => 0,
        "html" => 1,
        "graph_json" => 2,
        "json" => 3,
        "text" => 4,
        _ => 5,
    }
}

fn list_run_artifacts_internal(run_dir: &Path) -> Result<Vec<ArtifactItem>, String> {
    let run_dir_canonical = run_dir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize run directory {}: {e}", run_dir.display()))?;

    let mut out: Vec<ArtifactItem> = Vec::new();
    let specs = known_artifact_specs();
    let mut known_rel_paths = HashSet::new();

    for spec in &specs {
        let path = run_dir_canonical.join(rel_path_to_pathbuf(spec.rel_path));
        if !path.exists() || !path.is_file() {
            continue;
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize artifact {}: {e}", path.display()))?;
        if !canonical.starts_with(&run_dir_canonical) {
            continue;
        }
        let meta = fs::metadata(&canonical).ok();
        let size_bytes = meta.as_ref().map(|m| m.len());
        let mtime_iso = meta
            .and_then(|m| m.modified().ok())
            .map(to_iso_from_system_time);

        out.push(ArtifactItem {
            name: spec.name.to_string(),
            rel_path: spec.rel_path.to_string(),
            kind: classify_artifact_kind(&canonical, spec.name, size_bytes),
            size_bytes,
            mtime_iso,
        });
        known_rel_paths.insert(spec.rel_path.to_string());
    }

    let mut stack = vec![run_dir_canonical.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if !p.is_file() {
                continue;
            }
            let canonical = match p.canonicalize() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !canonical.starts_with(&run_dir_canonical) {
                continue;
            }
            let Some(rel) = normalized_rel_path(&run_dir_canonical, &canonical) else {
                continue;
            };
            if known_rel_paths.contains(&rel) {
                continue;
            }
            let name = canonical
                .file_name()
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_else(|| rel.clone());
            let meta = fs::metadata(&canonical).ok();
            let size_bytes = meta.as_ref().map(|m| m.len());
            let mtime_iso = meta
                .and_then(|m| m.modified().ok())
                .map(to_iso_from_system_time);

            out.push(ArtifactItem {
                name: name.clone(),
                rel_path: rel,
                kind: classify_artifact_kind(&canonical, &name, size_bytes),
                size_bytes,
                mtime_iso,
            });
        }
    }

    out.sort_by(|a, b| {
        kind_priority(&a.kind)
            .cmp(&kind_priority(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    Ok(out)
}

fn resolve_named_artifact_from_catalog(run_dir: &Path, name: &str) -> Result<ArtifactItem, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("artifact name is empty".to_string());
    }
    if n.contains('/') || n.contains('\\') || n.contains("..") {
        return Err("illegal artifact name".to_string());
    }

    let catalog = list_run_artifacts_internal(run_dir)?;
    let mut hits: Vec<ArtifactItem> = catalog.into_iter().filter(|a| a.name == n).collect();
    if hits.is_empty() {
        return Err(format!("artifact not found: {n}"));
    }
    if hits.len() > 1 {
        return Err(format!("artifact name is ambiguous: {n}"));
    }
    Ok(hits.remove(0))
}

fn read_artifact_content_internal(run_dir: &Path, item: &ArtifactItem) -> Result<NamedArtifactView, String> {
    let run_dir_canonical = run_dir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize run directory {}: {e}", run_dir.display()))?;
    let target = run_dir_canonical.join(rel_path_to_pathbuf(&item.rel_path));
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize artifact {}: {e}", target.display()))?;
    if !canonical.starts_with(&run_dir_canonical) {
        return Err("artifact path is outside run directory".to_string());
    }

    let meta = fs::metadata(&canonical)
        .map_err(|e| format!("failed to stat artifact {}: {e}", canonical.display()))?;
    if meta.len() > MAX_ARTIFACT_READ_BYTES {
        return Ok(NamedArtifactView {
            kind: item.kind.clone(),
            content: format!(
                "artifact is too large to preview ({} bytes, limit={} bytes). Use Open run folder.",
                meta.len(),
                MAX_ARTIFACT_READ_BYTES
            ),
            truncated: true,
            warnings: vec!["artifact exceeds preview size limit".to_string()],
        });
    }

    let raw = fs::read_to_string(&canonical)
        .map_err(|e| format!("failed to read artifact {}: {e}", canonical.display()))?;

    if item.kind == "html" {
        let (safe_html, warnings) = build_sandboxed_html(&raw);
        return Ok(NamedArtifactView {
            kind: item.kind.clone(),
            content: safe_html,
            truncated: false,
            warnings,
        });
    }

    if item.kind == "json" || item.kind == "graph_json" {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let pretty = serde_json::to_string_pretty(&v)
                .map_err(|e| format!("failed to pretty print json {}: {e}", canonical.display()))?;
            return Ok(NamedArtifactView {
                kind: item.kind.clone(),
                content: pretty,
                truncated: false,
                warnings: Vec::new(),
            });
        }
    }

    Ok(NamedArtifactView {
        kind: item.kind.clone(),
        content: raw,
        truncated: false,
        warnings: Vec::new(),
    })
}

fn artifact_spec_by_legacy_key(legacy_key: &str) -> Option<ArtifactSpec> {
    known_artifact_specs()
        .into_iter()
        .find(|s| s.legacy_key == legacy_key)
}

fn modified_epoch_ms(path: &Path) -> u64 {
    match fs::metadata(path)
        .and_then(|m| m.modified())
        .and_then(|t| t.duration_since(UNIX_EPOCH).map_err(std::io::Error::other))
    {
        Ok(d) => d.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn resolve_run_dir_from_id(runtime: &RuntimeConfig, run_id: &str) -> Result<PathBuf, String> {
    let run_component = validate_run_id_component(run_id)?;
    let candidate = runtime.out_base_dir.join(&run_component);
    if !candidate.exists() {
        return Err(format!("run directory does not exist: {}", candidate.display()));
    }
    if !candidate.is_dir() {
        return Err(format!("run path is not a directory: {}", candidate.display()));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize run directory {}: {e}", candidate.display()))?;
    if !canonical.starts_with(&runtime.out_base_dir) {
        return Err(format!(
            "run directory is outside out_dir: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

#[tauri::command]
fn list_runs(limit: Option<usize>, filters: Option<RunListFilter>) -> Result<Vec<RunListItem>, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let f = filters.unwrap_or_default();
    let query = f.query.unwrap_or_default().to_lowercase();
    let status_filter = f.status.unwrap_or_default().to_lowercase();
    let max_rows = limit.unwrap_or(500).clamp(1, 5000);

    let mut entries: Vec<(PathBuf, u64)> = Vec::new();
    for entry in fs::read_dir(&runtime.out_base_dir)
        .map_err(|e| format!("failed to read out_dir {}: {e}", runtime.out_base_dir.display()))?
    {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let ts = modified_epoch_ms(&path);
        entries.push((path, ts));
    }

    entries.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let an = a.0.file_name().map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
            let bn = b.0.file_name().map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
            an.cmp(&bn)
        })
    });

    let mut rows = Vec::with_capacity(entries.len());
    for (run_dir, ts) in entries {
        let run_id = run_dir
            .file_name()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let status = parse_status_from_result(&run_dir.join("result.json"));
        let paper_id = parse_paper_id_from_input(&run_dir.join("input.json"));
        let primary_viz = if let Ok(raw) = fs::read_to_string(run_dir.join("input.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                parse_primary_viz_from_input(&v)
            } else {
                None
            }
        } else {
            None
        };

        if !status_filter.is_empty() && status.to_lowercase() != status_filter {
            continue;
        }
        if !query.is_empty() {
            let hay = format!("{} {} {}", run_id.to_lowercase(), paper_id.to_lowercase(), status.to_lowercase());
            if !hay.contains(&query) {
                continue;
            }
        }

        rows.push(RunListItem {
            run_id,
            status,
            created_at_epoch_ms: ts,
            mtime_epoch_ms: ts,
            paper_id,
            primary_viz,
            run_dir: run_dir.to_string_lossy().to_string(),
        });
    }

    sort_runs_for_display(&mut rows);
    if rows.len() > max_rows {
        rows.truncate(max_rows);
    }

    Ok(rows)
}

#[tauri::command]
fn get_run_status(run_id: String) -> Result<String, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let run_id = validate_run_id_component(&run_id)?;
    let run_dir = resolve_run_dir_from_id(&runtime, &run_id)?;
    Ok(parse_status_from_result(&run_dir.join("result.json")))
}

fn diagnostics_root(out_dir: &Path) -> PathBuf {
    out_dir.join(".jarvis-desktop").join("diag")
}

fn validate_diag_id_component(diag_id: &str) -> Result<String, String> {
    let trimmed = diag_id.trim();
    if trimmed.is_empty() {
        return Err("diag_id is empty".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("diag_id is invalid".to_string());
    }
    if trimmed.contains('\\') || trimmed.contains('/') {
        return Err("diag_id must not contain path separators".to_string());
    }
    Ok(trimmed.to_string())
}

fn make_diag_id() -> String {
    let ts = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let short = make_run_id()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    format!("{}_{}", ts, short)
}

fn read_app_version(repo_root: &Path) -> Option<String> {
    let path = repo_root.join("package.json");
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn redact_sensitive_text(line: &str) -> String {
    let lowered = line.to_lowercase();
    if lowered.contains("api_key")
        || lowered.contains("token")
        || lowered.contains("authorization")
        || lowered.contains("password")
    {
        if let Some(idx) = line.find(':') {
            return format!("{}: ********", &line[..idx]);
        }
        return "********".to_string();
    }
    line.to_string()
}

fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut lines: Vec<String> = raw.lines().map(redact_sensitive_text).collect();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    lines
}

fn extract_gate_commands_from_checklist(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join("scripts").join("clean_machine_checklist.md");
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        if lower.contains("npm run build")
            || lower.contains("cargo test")
            || lower.contains("smoke_tauri_e2e")
            || lower.contains("collect_diag.ps1")
        {
            out.push(t.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_recent_run_summaries(out_dir: &Path, limit: usize) -> Vec<DiagnosticRunSummary> {
    let mut entries: Vec<(PathBuf, u64)> = Vec::new();
    let read = match fs::read_dir(out_dir) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        entries.push((path.clone(), modified_epoch_ms(&path)));
    }
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            a.0.file_name()
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_default()
                .cmp(
                    &b.0.file_name()
                        .map(|v| v.to_string_lossy().to_string())
                        .unwrap_or_default(),
                )
        })
    });

    let mut out = Vec::new();
    for (run_dir, ts) in entries.into_iter().take(limit) {
        let run_id = run_dir
            .file_name()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        out.push(DiagnosticRunSummary {
            run_id,
            status: parse_status_from_result(&run_dir.join("result.json")),
            mtime_epoch_ms: ts,
            canonical_id: parse_paper_id_from_input(&run_dir.join("input.json")),
        });
    }
    out
}

fn collect_candidate_diag_files(
    runtime: &RuntimeConfig,
    include_audit: bool,
    include_recent_runs: bool,
) -> Vec<(PathBuf, String)> {
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    let jobs = jobs_file_path(&runtime.out_base_dir);
    candidates.push((jobs, "state/jobs.json".to_string()));
    let pipelines = pipelines_file_path(&runtime.out_base_dir);
    candidates.push((pipelines, "state/pipelines.json".to_string()));
    let settings = settings_file_path(&runtime.out_base_dir);
    candidates.push((settings, "state/settings.json".to_string()));
    if include_audit {
        let audit = audit_jsonl_path(&runtime.out_base_dir);
        candidates.push((audit, "state/audit.jsonl".to_string()));
    }

    if include_recent_runs {
        let runs = collect_recent_run_summaries(&runtime.out_base_dir, 5);
        for run in runs {
            let run_path = runtime.out_base_dir.join(run.run_id.clone());
            let run_id = run.run_id;
            for (src_rel, dst_rel) in [
                ("input.json", "input.json"),
                ("result.json", "result.json"),
                ("paper_graph/tree/tree.md", "tree.md"),
                ("stdout.log", "stdout.log"),
                ("stderr.log", "stderr.log"),
            ] {
                let src = run_path.join(rel_path_to_pathbuf(src_rel));
                let rel = format!("runs/{run_id}/{dst_rel}");
                candidates.push((src, rel));
            }
        }
    }

    candidates.sort_by(|a, b| {
        a.0.to_string_lossy()
            .cmp(&b.0.to_string_lossy())
            .then_with(|| a.1.cmp(&b.1))
    });
    candidates
}

fn copy_diagnostic_files_with_caps(
    diag_dir: &Path,
    candidates: &[(PathBuf, String)],
) -> Result<(Vec<DiagnosticFileEntry>, u64), String> {
    let mut entries = Vec::new();
    let mut total: u64 = 0;

    for (src, rel) in candidates {
        let source_path = src.to_string_lossy().to_string();
        if !src.exists() {
            entries.push(DiagnosticFileEntry {
                rel_path: rel.clone(),
                source_path,
                included: false,
                size_bytes: 0,
                reason: Some("missing".to_string()),
            });
            continue;
        }
        let meta = fs::metadata(src)
            .map_err(|e| format!("failed to stat diagnostic source {}: {e}", src.display()))?;
        if !meta.is_file() {
            entries.push(DiagnosticFileEntry {
                rel_path: rel.clone(),
                source_path,
                included: false,
                size_bytes: 0,
                reason: Some("not_a_file".to_string()),
            });
            continue;
        }
        let size = meta.len();
        if size > DIAG_MAX_FILE_BYTES {
            entries.push(DiagnosticFileEntry {
                rel_path: rel.clone(),
                source_path,
                included: false,
                size_bytes: size,
                reason: Some("file_too_large".to_string()),
            });
            continue;
        }
        if total.saturating_add(size) > DIAG_MAX_TOTAL_BYTES {
            entries.push(DiagnosticFileEntry {
                rel_path: rel.clone(),
                source_path,
                included: false,
                size_bytes: size,
                reason: Some("total_limit_exceeded".to_string()),
            });
            continue;
        }

        let dst = diag_dir.join(rel_path_to_pathbuf(rel));
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create diagnostic directory {}: {e}", parent.display()))?;
        }
        fs::copy(src, &dst)
            .map_err(|e| format!("failed to copy diagnostic file {} -> {}: {e}", src.display(), dst.display()))?;

        total = total.saturating_add(size);
        entries.push(DiagnosticFileEntry {
            rel_path: rel.clone(),
            source_path,
            included: true,
            size_bytes: size,
            reason: None,
        });
    }

    Ok((entries, total))
}

fn render_diag_report(summary: &DiagnosticSummary) -> String {
    let mut out = String::new();
    out.push_str("# Diagnostics Report\n\n");
    out.push_str(&format!("- diag_id: {}\n", summary.diag_id));
    out.push_str(&format!("- created_at: {}\n", summary.created_at));
    out.push_str(&format!("- app_version: {}\n", summary.app_version.clone().unwrap_or_else(|| "unknown".to_string())));
    out.push_str(&format!("\n- os: {}\n- arch: {}\n", summary.os, summary.arch));
    out.push_str("\n## Resolved Config\n");
    out.push_str(&format!("- out_dir: {}\n", summary.out_dir));
    out.push_str(&format!("- pipeline_root: {}\n", summary.pipeline_root));
    out.push_str(&format!("- python_path: {}\n", summary.python_path));
    out.push_str("\n## Gates from Checklist\n");
    if summary.gate_commands.is_empty() {
        out.push_str("- (none)\n");
    } else {
        for cmd in &summary.gate_commands {
            out.push_str(&format!("- {}\n", cmd));
        }
    }

    out.push_str("\n## State Summary\n");
    out.push_str(&format!("- pipelines: {}\n", summary.pipelines.len()));
    out.push_str(&format!("- jobs: {}\n", summary.jobs.len()));
    out.push_str(&format!("- runs: {}\n", summary.runs.len()));
    out.push_str(&format!(
        "- copied_bytes: {} / {}\n",
        summary.total_included_bytes,
        summary.max_total_bytes
    ));

    out.push_str("\n## Skipped Files\n");
    let mut skipped = 0usize;
    for f in &summary.files {
        if !f.included {
            skipped += 1;
            out.push_str(&format!(
                "- {} (reason={}, source={})\n",
                f.rel_path,
                f.reason.clone().unwrap_or_else(|| "unknown".to_string()),
                f.source_path
            ));
        }
    }
    if skipped == 0 {
        out.push_str("- (none)\n");
    }
    out
}

fn directory_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total = total.saturating_add(directory_size_bytes(&p));
        } else if let Ok(m) = fs::metadata(&p) {
            total = total.saturating_add(m.len());
        }
    }
    total
}

fn collect_diagnostics_internal(
    root: &Path,
    runtime: &RuntimeConfig,
    opts: DiagnosticsCollectOptions,
) -> Result<DiagnosticsCollectResult, String> {
    let options = opts;
    let include_audit = options.include_audit.unwrap_or(true);
    let include_recent_runs = options.include_recent_runs.unwrap_or(true);
    let include_zip = options.include_zip.unwrap_or(true);

    let diag_root = diagnostics_root(&runtime.out_base_dir);
    fs::create_dir_all(&diag_root)
        .map_err(|e| format!("failed to create diagnostics root {}: {e}", diag_root.display()))?;

    let diag_id = make_diag_id();
    let diag_dir = diag_root.join(&diag_id);
    fs::create_dir_all(&diag_dir)
        .map_err(|e| format!("failed to create diagnostic dir {}: {e}", diag_dir.display()))?;

    let mut jobs = load_jobs_from_file(&jobs_file_path(&runtime.out_base_dir))?;
    sort_jobs_for_display(&mut jobs);
    if jobs.len() > DIAG_MAX_RECENT_ITEMS {
        jobs.truncate(DIAG_MAX_RECENT_ITEMS);
    }
    let job_rows = jobs
        .into_iter()
        .map(|j| DiagnosticJobSummary {
            job_id: j.job_id,
            status: format!("{:?}", j.status).to_lowercase(),
            attempt: j.attempt,
            updated_at: j.updated_at,
            retry_at: j.retry_at,
            auto_retry_attempt_count: j.auto_retry_attempt_count,
        })
        .collect::<Vec<_>>();

    let mut pipelines = load_pipelines_from_file(&pipelines_file_path(&runtime.out_base_dir))?;
    pipelines.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.pipeline_id.cmp(&b.pipeline_id)));
    if pipelines.len() > DIAG_MAX_RECENT_ITEMS {
        pipelines.truncate(DIAG_MAX_RECENT_ITEMS);
    }
    let pipeline_rows = pipelines
        .into_iter()
        .map(|p| DiagnosticPipelineSummary {
            pipeline_id: p.pipeline_id,
            status: format!("{:?}", p.status).to_lowercase(),
            current_step_index: p.current_step_index,
            total_steps: p.steps.len(),
            updated_at: p.updated_at,
            canonical_id: p.canonical_id,
        })
        .collect::<Vec<_>>();

    let mut run_rows = if include_recent_runs {
        collect_recent_run_summaries(&runtime.out_base_dir, DIAG_MAX_RECENT_ITEMS)
    } else {
        Vec::new()
    };
    run_rows.sort_by(|a, b| b.mtime_epoch_ms.cmp(&a.mtime_epoch_ms).then_with(|| a.run_id.cmp(&b.run_id)));

    let audit_tail = if include_audit {
        read_tail_lines(&audit_jsonl_path(&runtime.out_base_dir), DIAG_AUDIT_TAIL_LINES)
    } else {
        Vec::new()
    };

    let candidates = collect_candidate_diag_files(runtime, include_audit, include_recent_runs);
    let (files, total_included_bytes) = copy_diagnostic_files_with_caps(&diag_dir, &candidates)?;

    let smoke_script_path = root
        .join("smoke_tauri_e2e.ps1")
        .to_string_lossy()
        .to_string();
    let gate_commands = extract_gate_commands_from_checklist(root);

    let python_path = choose_python(root, &runtime.pipeline_root).0;
    let summary = DiagnosticSummary {
        diag_id: diag_id.clone(),
        created_at: Utc::now().to_rfc3339(),
        app_version: read_app_version(root),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        out_dir: runtime.out_base_dir.to_string_lossy().to_string(),
        pipeline_root: runtime.pipeline_root.to_string_lossy().to_string(),
        python_path,
        include_audit,
        include_recent_runs,
        include_zip,
        smoke_script_path,
        gate_commands,
        jobs: job_rows,
        pipelines: pipeline_rows,
        runs: run_rows,
        audit_tail,
        files,
        total_included_bytes,
        max_file_bytes: DIAG_MAX_FILE_BYTES,
        max_total_bytes: DIAG_MAX_TOTAL_BYTES,
    };

    let summary_path = diag_dir.join("diag_summary.json");
    let summary_text = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("failed to serialize diag summary: {e}"))?;
    atomic_write_text(&summary_path, &summary_text)?;

    let report_path = diag_dir.join("diag_report.md");
    let report_text = render_diag_report(&summary);
    atomic_write_text(&report_path, &report_text)?;

    Ok(DiagnosticsCollectResult {
        diag_id,
        diag_dir: diag_dir.to_string_lossy().to_string(),
        report_path: report_path.to_string_lossy().to_string(),
        zip_path: None,
    })
}

#[tauri::command]
fn collect_diagnostics(opts: Option<DiagnosticsCollectOptions>) -> Result<DiagnosticsCollectResult, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    collect_diagnostics_internal(&root, &runtime, opts.unwrap_or_default())
}

#[tauri::command]
fn list_diagnostics() -> Result<Vec<DiagnosticListItem>, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let diag_root = diagnostics_root(&runtime.out_base_dir);
    if !diag_root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(&diag_root)
        .map_err(|e| format!("failed to read diagnostics root {}: {e}", diag_root.display()))?
    {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let diag_id = match path.file_name().map(|v| v.to_string_lossy().to_string()) {
            Some(v) => v,
            None => continue,
        };
        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(to_iso_from_system_time)
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let zip = path.join("bundle.zip");
        out.push(DiagnosticListItem {
            diag_id,
            created_at: modified,
            size_bytes: directory_size_bytes(&path),
            zip_path: if zip.exists() {
                Some(zip.to_string_lossy().to_string())
            } else {
                None
            },
        });
    }

    out.sort_by(|a, b| b.diag_id.cmp(&a.diag_id).then_with(|| a.created_at.cmp(&b.created_at)));
    Ok(out)
}

#[tauri::command]
fn read_diagnostic_report(diag_id: String) -> Result<String, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let diag_id = validate_diag_id_component(&diag_id)?;
    let diag_root = diagnostics_root(&runtime.out_base_dir);
    let root_canonical = canonicalize_existing_dir(&diag_root, "RULE_DIAG_ROOT_INVALID")?;
    let target = diag_root.join(&diag_id).join("diag_report.md");
    if !target.exists() {
        return Err(format!("diagnostic report not found: {}", target.display()));
    }
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize diagnostic report {}: {e}", target.display()))?;
    if !canonical.starts_with(&root_canonical) {
        return Err("diagnostic report path is outside diagnostics root".to_string());
    }
    fs::read_to_string(&canonical)
        .map_err(|e| format!("failed to read diagnostic report {}: {e}", canonical.display()))
}

#[tauri::command]
fn open_diagnostic_folder(diag_id: String) -> Result<String, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let diag_id = validate_diag_id_component(&diag_id)?;
    let diag_root = diagnostics_root(&runtime.out_base_dir);
    let root_canonical = canonicalize_existing_dir(&diag_root, "RULE_DIAG_ROOT_INVALID")?;
    let target = diag_root.join(&diag_id);
    let canonical = canonicalize_existing_dir(&target, "RULE_DIAG_DIR_INVALID")?;
    if !canonical.starts_with(&root_canonical) {
        return Err("diagnostic folder is outside diagnostics root".to_string());
    }
    Command::new("explorer")
        .arg(&canonical)
        .spawn()
        .map_err(|e| format!("Failed to open diagnostic folder in explorer: {e}"))?;
    Ok(canonical.to_string_lossy().to_string())
}

#[tauri::command]
fn read_run_artifact(run_id: String, artifact: String) -> Result<RunArtifactView, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let run_id = validate_run_id_component(&run_id)?;
    let run_dir = resolve_run_dir_from_id(&runtime, &run_id)?;

    let spec = artifact_spec_by_legacy_key(&artifact)
        .ok_or_else(|| format!("unsupported artifact: {artifact}"))?;
    let item = resolve_named_artifact_from_catalog(&run_dir, spec.name);
    let item = match item {
        Ok(v) => v,
        Err(_) => {
            let target = run_dir.join(rel_path_to_pathbuf(spec.rel_path));
            return Ok(RunArtifactView {
                run_id,
                artifact: artifact.to_string(),
                path: target.to_string_lossy().to_string(),
                exists: false,
                content: "missing".to_string(),
                parse_status: "missing".to_string(),
            });
        }
    };

    let target = run_dir.join(rel_path_to_pathbuf(&item.rel_path));
    if !target.exists() || !target.is_file() {
        return Ok(RunArtifactView {
            run_id,
            artifact: artifact.to_string(),
            path: target.to_string_lossy().to_string(),
            exists: false,
            content: "missing".to_string(),
            parse_status: "missing".to_string(),
        });
    }

    let named = read_artifact_content_internal(&run_dir, &item)?;
    Ok(RunArtifactView {
        run_id,
        artifact: artifact.to_string(),
        path: target.to_string_lossy().to_string(),
        exists: true,
        content: named.content,
        parse_status: if named.truncated {
            "truncated".to_string()
        } else {
            "ok".to_string()
        },
    })
}

#[tauri::command]
fn list_run_artifacts(run_id: String) -> Result<Vec<ArtifactItem>, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let run_id = validate_run_id_component(&run_id)?;
    let run_dir = resolve_run_dir_from_id(&runtime, &run_id)?;
    list_run_artifacts_internal(&run_dir)
}

#[tauri::command]
fn read_run_artifact_named(run_id: String, name: String) -> Result<NamedArtifactView, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let run_id = validate_run_id_component(&run_id)?;
    let run_dir = resolve_run_dir_from_id(&runtime, &run_id)?;
    let item = resolve_named_artifact_from_catalog(&run_dir, &name)?;
    read_artifact_content_internal(&run_dir, &item)
}

fn merge_desktop_input_metadata(
    run_dir: &Path,
    template_id: &str,
    canonical_id: &str,
    params: &serde_json::Value,
    primary_viz: Option<&PrimaryVizRef>,
) -> Result<(), String> {
    let input_path = run_dir.join("input.json");

    let merged = if input_path.exists() {
        let raw = fs::read_to_string(&input_path)
            .map_err(|e| format!("failed to read input.json {}: {e}", input_path.display()))?;
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(mut v) => {
                if let Some(obj) = v.as_object_mut() {
                    let desktop_obj = if let Some(existing) = obj.get_mut("desktop") {
                        if let Some(d) = existing.as_object_mut() {
                            d
                        } else {
                            *existing = serde_json::json!({});
                            existing.as_object_mut().expect("desktop converted to object")
                        }
                    } else {
                        obj.insert("desktop".to_string(), serde_json::json!({}));
                        obj.get_mut("desktop")
                            .and_then(|x| x.as_object_mut())
                            .expect("desktop inserted")
                    };

                    desktop_obj.insert("template_id".to_string(), serde_json::json!(template_id));
                    desktop_obj.insert("canonical_id".to_string(), serde_json::json!(canonical_id));
                    desktop_obj.insert("params".to_string(), params.clone());
                    desktop_obj.insert("created_by".to_string(), serde_json::json!("jarvis-desktop"));
                    desktop_obj.insert("version".to_string(), serde_json::json!(env!("CARGO_PKG_VERSION")));
                    if let Some(pv) = primary_viz {
                        desktop_obj.insert(
                            "primary_viz".to_string(),
                            serde_json::json!({ "name": pv.name, "kind": pv.kind }),
                        );
                    } else {
                        desktop_obj.remove("primary_viz");
                    }
                    v
                } else {
                    serde_json::json!({
                        "original": v,
                        "desktop": {
                            "template_id": template_id,
                            "canonical_id": canonical_id,
                            "params": params,
                            "created_by": "jarvis-desktop",
                            "version": env!("CARGO_PKG_VERSION"),
                            "primary_viz": primary_viz.map(|pv| serde_json::json!({"name": pv.name, "kind": pv.kind})),
                        },
                    })
                }
            }
            Err(_) => serde_json::json!({
                "desktop": {
                    "template_id": template_id,
                    "canonical_id": canonical_id,
                    "params": params,
                    "created_by": "jarvis-desktop",
                    "version": env!("CARGO_PKG_VERSION"),
                    "primary_viz": primary_viz.map(|pv| serde_json::json!({"name": pv.name, "kind": pv.kind})),
                },
            }),
        }
    } else {
        serde_json::json!({
            "desktop": {
                "template_id": template_id,
                "canonical_id": canonical_id,
                "params": params,
                "created_by": "jarvis-desktop",
                "version": env!("CARGO_PKG_VERSION"),
                "primary_viz": primary_viz.map(|pv| serde_json::json!({"name": pv.name, "kind": pv.kind})),
            },
        })
    };

    let pretty = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("failed to serialize merged input.json: {e}"))?;
    fs::write(&input_path, pretty)
        .map_err(|e| format!("failed to write input.json {}: {e}", input_path.display()))
}

fn execute_pipeline_task(
    task_args: Vec<String>,
    template_id: String,
    canonical_id: String,
    normalized_params: serde_json::Value,
    worker_ctx: Option<(Arc<Mutex<JobRuntimeState>>, String)>,
) -> RunResult {
    let run_id = make_run_id();
    let root = repo_root();
    let runtime = match resolve_runtime_config(&root) {
        Ok(cfg) => cfg,
        Err(e) => return missing_dependency(run_id, e),
    };
    let pipeline_root = runtime.pipeline_root.clone();

    let cli_script = pipeline_root.join("jarvis_cli.py");
    if !cli_script.is_file() {
        return missing_dependency(
            run_id,
            format!(
                "Pipeline entrypoint not found: {}. Check JARVIS_PIPELINE_ROOT.",
                cli_script.display()
            ),
        );
    }

    let (python_cmd, preflight_warnings) = choose_python(&root, &pipeline_root);
    if let Err(e) = check_python_runnable(&python_cmd, &pipeline_root) {
        return missing_dependency(
            run_id,
            format!("{e}\nHint: set JARVIS_PIPELINE_ROOT and prepare a venv under src-tauri/.venv or pipeline/.venv."),
        );
    }

    let out_base_dir = runtime.out_base_dir.clone();
    let run_dir_abs = out_base_dir.join(&run_id);
    if let Err(e) = std::fs::create_dir_all(&run_dir_abs) {
        return RunResult {
            ok: false,
            exit_code: 1,
            stdout: "".to_string(),
            stderr: format!(
                "failed to create run directory {}: {e}",
                run_dir_abs.display()
            ),
            run_id,
            run_dir: run_dir_abs.to_string_lossy().to_string(),
            status: "error".to_string(),
            message: format!(
                "failed to create run directory {}: {e}",
                run_dir_abs.display()
            ),
            retry_after_sec: None,
        };
    }

    let mut cmd = Command::new(&python_cmd);
    cmd.env("JARVIS_PIPELINE_ROOT", &pipeline_root);
    cmd.env("JARVIS_PIPELINE_OUT_DIR", &out_base_dir);
    if let Some(v) = runtime.s2_api_key.as_ref() {
        cmd.env("S2_API_KEY", v);
    }
    if let Some(v) = runtime.s2_min_interval_ms {
        cmd.env("S2_MIN_INTERVAL_MS", v.to_string());
    }
    if let Some(v) = runtime.s2_max_retries {
        cmd.env("S2_MAX_RETRIES", v.to_string());
    }
    if let Some(v) = runtime.s2_backoff_base_sec {
        cmd.env("S2_BACKOFF_BASE_SEC", v.to_string());
    }

    let mut final_args = task_args;
    final_args.extend_from_slice(&[
        "--out".to_string(),
        out_base_dir.to_string_lossy().to_string(),
        "--out-run".to_string(),
        run_id.clone(),
    ]);

    cmd.current_dir(&pipeline_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg(cli_script.as_os_str())
        .args(&final_args);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunResult {
                ok: false,
                exit_code: 1,
                stdout: "".to_string(),
                stderr: format!("failed to spawn pipeline: {e}"),
                run_id,
                run_dir: run_dir_abs.to_string_lossy().to_string(),
                status: "error".to_string(),
                message: format!("failed to spawn pipeline: {e}"),
                retry_after_sec: None,
            }
        }
    };

    if let Some((state, job_id)) = worker_ctx.as_ref() {
        if let Ok(mut guard) = state.lock() {
            if guard.running_job_id.as_deref() == Some(job_id.as_str()) {
                guard.running_pid = Some(child.id());
            }
        }
    }

    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return RunResult {
                ok: false,
                exit_code: 1,
                stdout: "".to_string(),
                stderr: format!("failed to wait pipeline process: {e}"),
                run_id,
                run_dir: run_dir_abs.to_string_lossy().to_string(),
                status: "error".to_string(),
                message: format!("failed to wait pipeline process: {e}"),
                retry_after_sec: None,
            }
        }
    };

    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let mut stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !preflight_warnings.is_empty() {
        let warning = format!("[preflight warning]\n{}\n", preflight_warnings.join("\n"));
        stderr = if stderr.is_empty() {
            warning
        } else {
            format!("{warning}{stderr}")
        };
    }

    if out.status.success() {
        let primary_viz = list_run_artifacts_internal(&run_dir_abs)
            .ok()
            .and_then(|items| select_primary_viz_artifact(&items));
        let _ = merge_desktop_input_metadata(
            &run_dir_abs,
            &template_id,
            &canonical_id,
            &normalized_params,
            primary_viz.as_ref(),
        );
    }

    let status = read_status(&stdout, &stderr, code);
    let retry_after_sec = extract_retry_after_seconds(&format!("{stdout}\n{stderr}"));
    let message = build_status_message(&status, &stdout, &stderr, retry_after_sec);

    RunResult {
        ok: out.status.success(),
        exit_code: code,
        stdout,
        stderr,
        run_id,
        run_dir: run_dir_abs.to_string_lossy().to_string(),
        status,
        message,
        retry_after_sec,
    }
}

#[tauri::command]
fn list_task_templates() -> Vec<TaskTemplateDef> {
    template_registry()
}

fn enqueue_job_internal(
    state: &Arc<Mutex<JobRuntimeState>>,
    jobs_path: &Path,
    template_id: String,
    canonical_id: String,
    params: serde_json::Value,
) -> Result<String, String> {
    let tpl = find_template(&template_id).ok_or_else(|| format!("unknown template id: {template_id}"))?;
    if !tpl.wired {
        return Err(format!("template not wired: {}", tpl.id));
    }

    let normalized = normalize_identifier_internal(&canonical_id);
    if !normalized.errors.is_empty() {
        return Err(format!("invalid canonical_id: {}", normalized.errors.join("; ")));
    }

    let job_id = format!("job_{}_{}", now_epoch_ms(), make_run_id());
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        let now = now_epoch_ms_string();
        guard.jobs.push(JobRecord {
            job_id: job_id.clone(),
            template_id,
            canonical_id,
            params,
            status: JobStatus::Queued,
            attempt: 0,
            created_at: now.clone(),
            updated_at: now,
            run_id: None,
            last_error: None,
            retry_after_seconds: None,
            retry_at: None,
            auto_retry_attempt_count: 0,
        });
    }
    persist_state(state, jobs_path)?;
    Ok(job_id)
}

#[tauri::command]
fn enqueue_job(
    template_id: String,
    canonical_id: String,
    params: serde_json::Value,
) -> Result<String, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let job_id = enqueue_job_internal(&state, &jobs_path, template_id, canonical_id, params)?;
    start_job_worker_if_needed()?;
    Ok(job_id)
}

#[tauri::command]
fn list_jobs() -> Result<Vec<JobRecord>, String> {
    let (state, jobs_path) = init_job_runtime()?;
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        guard.jobs = load_jobs_from_file(&jobs_path)?;
        let mut rows = guard.jobs.clone();
        sort_jobs_for_display(&mut rows);
        Ok(rows)
    }
}

#[tauri::command]
fn cancel_job(job_id: String) -> Result<JobRecord, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let updated: JobRecord;
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        let idx = guard
            .jobs
            .iter()
            .position(|j| j.job_id == job_id)
            .ok_or_else(|| format!("job not found: {job_id}"))?;

        match guard.jobs[idx].status {
            JobStatus::Queued => {
                guard.jobs[idx].status = JobStatus::Canceled;
            }
            JobStatus::Running => {
                guard.cancel_requested.insert(job_id.clone());
                if let Some(pid) = guard.running_pid {
                    let _ = Command::new("cmd")
                        .args(["/c", &format!("taskkill /PID {pid} /T /F")])
                        .output();
                }
                guard.jobs[idx].status = JobStatus::Canceled;
            }
            _ => {}
        }
        guard.jobs[idx].updated_at = now_epoch_ms_string();
        updated = guard.jobs[idx].clone();
    }
    persist_state(&state, &jobs_path)?;
    if let Ok((runtime, _)) = runtime_and_jobs_path() {
        let _ = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, Some(&job_id));
    }
    Ok(updated)
}

#[tauri::command]
fn retry_job(job_id: String, force: Option<bool>) -> Result<JobRecord, String> {
    let force_retry = force.unwrap_or(false);
    let (state, jobs_path) = init_job_runtime()?;
    let updated: JobRecord;
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        let idx = guard
            .jobs
            .iter()
            .position(|j| j.job_id == job_id)
            .ok_or_else(|| format!("job not found: {job_id}"))?;

        let status = guard.jobs[idx].status.clone();
        if !(status == JobStatus::Failed || status == JobStatus::NeedsRetry || force_retry) {
            return Err("job is not retryable".to_string());
        }

        if !force_retry {
            if let Some(retry_at) = guard.jobs[idx].retry_at.as_ref() {
                if let Ok(ts) = retry_at.parse::<u128>() {
                    if now_epoch_ms() < ts {
                        return Err("retry window has not started yet; pass force=true to override".to_string());
                    }
                }
            }
        }

        guard.jobs[idx].status = JobStatus::Queued;
        guard.jobs[idx].updated_at = now_epoch_ms_string();
        guard.jobs[idx].last_error = None;
        guard.jobs[idx].retry_after_seconds = None;
        guard.jobs[idx].retry_at = None;
        updated = guard.jobs[idx].clone();
    }
    persist_state(&state, &jobs_path)?;
    if let Ok((runtime, _)) = runtime_and_jobs_path() {
        let _ = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, Some(&job_id));
    }
    start_job_worker_if_needed()?;
    Ok(updated)
}

#[tauri::command]
fn clear_finished_jobs() -> Result<usize, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let removed;
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        let before = guard.jobs.len();
        guard.jobs.retain(|j| {
            !(j.status == JobStatus::Succeeded || j.status == JobStatus::Failed || j.status == JobStatus::Canceled)
        });
        removed = before.saturating_sub(guard.jobs.len());
    }
    persist_state(&state, &jobs_path)?;
    Ok(removed)
}

fn reconcile_pipelines_with_jobs(
    out_dir: &Path,
    state: &Arc<Mutex<JobRuntimeState>>,
    jobs_path: &Path,
    only_job_id: Option<&str>,
) -> Result<Vec<PipelineRecord>, String> {
    let pipelines_path = pipelines_file_path(out_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;
    if pipelines.is_empty() {
        return Ok(pipelines);
    }

    let jobs_snapshot = {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime for pipelines".to_string())?;
        guard.jobs = load_jobs_from_file(jobs_path)?;
        guard.jobs.clone()
    };

    let mut changed = false;
    for pipeline in &mut pipelines {
        if pipeline.steps.is_empty() {
            if pipeline.status != PipelineStatus::Succeeded {
                pipeline.status = PipelineStatus::Succeeded;
                pipeline.updated_at = now_epoch_ms_string();
                changed = true;
            }
            continue;
        }
        if pipeline.status != PipelineStatus::Running {
            continue;
        }

        if pipeline.current_step_index >= pipeline.steps.len() {
            pipeline.current_step_index = pipeline.steps.len().saturating_sub(1);
            changed = true;
        }

        loop {
            if pipeline.current_step_index >= pipeline.steps.len() {
                pipeline.status = PipelineStatus::Succeeded;
                pipeline.updated_at = now_epoch_ms_string();
                changed = true;
                break;
            }

            let idx = pipeline.current_step_index;
            let terminal_status = {
                let step = &pipeline.steps[idx];
                if is_pipeline_step_terminal(&step.status) {
                    Some(step.status.clone())
                } else {
                    None
                }
            };

            if let Some(step_status) = terminal_status {
                if step_status == PipelineStepStatus::Succeeded {
                    if idx + 1 >= pipeline.steps.len() {
                        pipeline.status = PipelineStatus::Succeeded;
                        pipeline.updated_at = now_epoch_ms_string();
                        changed = true;
                        break;
                    }
                    pipeline.current_step_index = idx + 1;
                    changed = true;
                    continue;
                }
                pipeline.status = match step_status {
                    PipelineStepStatus::NeedsRetry => PipelineStatus::NeedsRetry,
                    PipelineStepStatus::Canceled => PipelineStatus::Canceled,
                    _ => PipelineStatus::Failed,
                };
                pipeline.updated_at = now_epoch_ms_string();
                changed = true;
                break;
            }

            if pipeline.steps[idx].status == PipelineStepStatus::Pending {
                let job_id = enqueue_job_internal(
                    state,
                    jobs_path,
                    pipeline.steps[idx].template_id.clone(),
                    pipeline.canonical_id.clone(),
                    pipeline.steps[idx].params.clone(),
                )?;
                pipeline.steps[idx].job_id = Some(job_id);
                pipeline.steps[idx].status = PipelineStepStatus::Running;
                if pipeline.steps[idx].started_at.is_none() {
                    pipeline.steps[idx].started_at = Some(now_epoch_ms_string());
                }
                pipeline.steps[idx].finished_at = None;
                pipeline.status = PipelineStatus::Running;
                pipeline.updated_at = now_epoch_ms_string();
                changed = true;
                break;
            }

            if pipeline.steps[idx].status == PipelineStepStatus::Running {
                let job_id = pipeline.steps[idx].job_id.clone();
                let Some(step_job_id) = job_id else {
                    pipeline.steps[idx].status = PipelineStepStatus::Pending;
                    pipeline.updated_at = now_epoch_ms_string();
                    changed = true;
                    continue;
                };

                if let Some(target) = only_job_id {
                    if target != step_job_id {
                        break;
                    }
                }

                let Some(job) = jobs_snapshot.iter().find(|j| j.job_id == step_job_id) else {
                    break;
                };

                let mapped = pipeline_step_status_from_job(job);
                if mapped == PipelineStepStatus::Running {
                    break;
                }

                pipeline.steps[idx].status = mapped.clone();
                if pipeline.steps[idx].started_at.is_none() {
                    pipeline.steps[idx].started_at = Some(now_epoch_ms_string());
                }
                pipeline.steps[idx].finished_at = Some(now_epoch_ms_string());
                if pipeline.steps[idx].run_id.is_none() {
                    pipeline.steps[idx].run_id = job.run_id.clone();
                }
                if let Some(run_id) = pipeline.steps[idx].run_id.as_ref() {
                    let run_dir = out_dir.join(run_id);
                    if let Some(pv) = parse_run_primary_viz(&run_dir) {
                        pipeline.last_primary_viz = Some(pv);
                    }
                }
                pipeline.updated_at = now_epoch_ms_string();
                changed = true;
                continue;
            }

            break;
        }
    }

    if changed {
        save_pipelines_to_file(&pipelines_path, &pipelines)?;
    }
    Ok(pipelines)
}

#[tauri::command]
fn create_pipeline(
    name: String,
    canonical_id: String,
    steps: Vec<PipelineCreateStepInput>,
) -> Result<String, String> {
    if steps.is_empty() {
        return Err("pipeline must have at least one step".to_string());
    }

    let normalized = normalize_identifier_internal(&canonical_id);
    if !normalized.errors.is_empty() {
        return Err(format!("invalid canonical_id: {}", normalized.errors.join("; ")));
    }
    let canonical = normalized.canonical;

    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines_path = pipelines_file_path(&runtime.out_base_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;

    let mut out_steps = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        let tpl = find_template(&step.template_id)
            .ok_or_else(|| format!("unknown template id: {}", step.template_id))?;
        if !tpl.wired {
            return Err(format!("template not wired: {}", tpl.id));
        }
        let _ = build_template_args(&step.template_id, &canonical, &step.params)?;

        out_steps.push(PipelineStep {
            step_id: sanitize_step_id(&step.template_id, idx),
            template_id: step.template_id.clone(),
            params: step.params.clone(),
            job_id: None,
            status: PipelineStepStatus::Pending,
            run_id: None,
            started_at: None,
            finished_at: None,
        });
    }

    let pipeline_id = make_pipeline_id();
    let now = now_epoch_ms_string();
    pipelines.push(PipelineRecord {
        pipeline_id: pipeline_id.clone(),
        canonical_id: canonical,
        name: if name.trim().is_empty() {
            "Analyze Paper".to_string()
        } else {
            name.trim().to_string()
        },
        created_at: now.clone(),
        updated_at: now,
        steps: out_steps,
        current_step_index: 0,
        status: PipelineStatus::Running,
        last_primary_viz: None,
        auto_retry_attempt_count: 0,
    });
    save_pipelines_to_file(&pipelines_path, &pipelines)?;

    let _ = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;
    start_job_worker_if_needed()?;
    Ok(pipeline_id)
}

#[tauri::command]
fn list_pipelines(filters: Option<PipelineListFilter>) -> Result<Vec<PipelineSummary>, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;

    let f = filters.unwrap_or_default();
    let q = f.query.unwrap_or_default().to_lowercase();
    let status = f.status.unwrap_or_default().to_lowercase();

    let mut out = Vec::new();
    for p in pipelines {
        if !q.is_empty() {
            let hay = format!("{} {} {}", p.pipeline_id, p.name, p.canonical_id).to_lowercase();
            if !hay.contains(&q) {
                continue;
            }
        }
        if !status.is_empty() && pipeline_status_text(&p.status) != status {
            continue;
        }
        out.push(PipelineSummary {
            pipeline_id: p.pipeline_id,
            canonical_id: p.canonical_id,
            name: p.name,
            status: p.status,
            current_step_index: p.current_step_index,
            total_steps: p.steps.len(),
            updated_at: p.updated_at,
            last_primary_viz: p.last_primary_viz,
        });
    }

    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.pipeline_id.cmp(&b.pipeline_id)));
    Ok(out)
}

#[tauri::command]
fn get_pipeline(pipeline_id: String) -> Result<PipelineRecord, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;
    pipelines
        .into_iter()
        .find(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found: {pipeline_id}"))
}

#[tauri::command]
fn start_pipeline(pipeline_id: String) -> Result<PipelineRecord, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines_path = pipelines_file_path(&runtime.out_base_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;
    let idx = pipelines
        .iter()
        .position(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found: {pipeline_id}"))?;
    pipelines[idx].status = PipelineStatus::Running;
    pipelines[idx].updated_at = now_epoch_ms_string();
    save_pipelines_to_file(&pipelines_path, &pipelines)?;

    let pipelines = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;
    start_job_worker_if_needed()?;
    pipelines
        .into_iter()
        .find(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found after start: {pipeline_id}"))
}

#[tauri::command]
fn cancel_pipeline(pipeline_id: String) -> Result<PipelineRecord, String> {
    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines_path = pipelines_file_path(&runtime.out_base_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;
    let idx = pipelines
        .iter()
        .position(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found: {pipeline_id}"))?;

    let current_idx = pipelines[idx].current_step_index;
    if current_idx < pipelines[idx].steps.len() {
        let step = &mut pipelines[idx].steps[current_idx];
        if let Some(job_id) = step.job_id.clone() {
            let _ = cancel_job(job_id);
        }
        if !is_pipeline_step_terminal(&step.status) {
            step.status = PipelineStepStatus::Canceled;
            step.finished_at = Some(now_epoch_ms_string());
        }
    }
    pipelines[idx].status = PipelineStatus::Canceled;
    pipelines[idx].updated_at = now_epoch_ms_string();
    save_pipelines_to_file(&pipelines_path, &pipelines)?;

    let pipelines = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;
    pipelines
        .into_iter()
        .find(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found after cancel: {pipeline_id}"))
}

#[tauri::command]
fn retry_pipeline_step(
    pipeline_id: String,
    step_id: String,
    force: Option<bool>,
) -> Result<PipelineRecord, String> {
    let _force = force.unwrap_or(false);
    let (state, jobs_path) = init_job_runtime()?;
    let (runtime, _) = runtime_and_jobs_path()?;
    let pipelines_path = pipelines_file_path(&runtime.out_base_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;
    let pidx = pipelines
        .iter()
        .position(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found: {pipeline_id}"))?;
    let sidx = pipelines[pidx]
        .steps
        .iter()
        .position(|s| s.step_id == step_id)
        .ok_or_else(|| format!("step not found: {step_id}"))?;

    let step_status = pipelines[pidx].steps[sidx].status.clone();
    if !(step_status == PipelineStepStatus::Failed
        || step_status == PipelineStepStatus::NeedsRetry
        || step_status == PipelineStepStatus::Canceled
        || _force)
    {
        return Err("step is not retryable".to_string());
    }

    for later in (sidx + 1)..pipelines[pidx].steps.len() {
        pipelines[pidx].steps[later].job_id = None;
        pipelines[pidx].steps[later].status = PipelineStepStatus::Pending;
        pipelines[pidx].steps[later].run_id = None;
        pipelines[pidx].steps[later].started_at = None;
        pipelines[pidx].steps[later].finished_at = None;
    }

    pipelines[pidx].steps[sidx].job_id = None;
    pipelines[pidx].steps[sidx].status = PipelineStepStatus::Pending;
    pipelines[pidx].steps[sidx].run_id = None;
    pipelines[pidx].steps[sidx].started_at = None;
    pipelines[pidx].steps[sidx].finished_at = None;
    pipelines[pidx].current_step_index = sidx;
    pipelines[pidx].status = PipelineStatus::Running;
    pipelines[pidx].updated_at = now_epoch_ms_string();
    save_pipelines_to_file(&pipelines_path, &pipelines)?;

    let pipelines = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None)?;
    start_job_worker_if_needed()?;
    pipelines
        .into_iter()
        .find(|p| p.pipeline_id == pipeline_id)
        .ok_or_else(|| format!("pipeline not found after retry: {pipeline_id}"))
}

#[tauri::command]
fn get_settings() -> Result<DesktopSettings, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    load_settings(&runtime.out_base_dir)
}

#[tauri::command]
fn update_settings(settings: DesktopSettings) -> Result<DesktopSettings, String> {
    if settings.auto_retry_max_per_job == 0 {
        return Err("auto_retry_max_per_job must be >= 1".to_string());
    }
    if settings.auto_retry_max_per_pipeline == 0 {
        return Err("auto_retry_max_per_pipeline must be >= 1".to_string());
    }
    if settings.auto_retry_base_delay_seconds == 0 {
        return Err("auto_retry_base_delay_seconds must be >= 1".to_string());
    }
    if settings.auto_retry_max_delay_seconds == 0 {
        return Err("auto_retry_max_delay_seconds must be >= 1".to_string());
    }

    let (runtime, _) = runtime_and_jobs_path()?;
    save_settings(&runtime.out_base_dir, &settings)?;
    Ok(settings)
}

#[tauri::command]
fn open_audit_log() -> Result<String, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let path = audit_jsonl_path(&runtime.out_base_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create audit directory {}: {e}", parent.display()))?;
    }
    if !path.exists() {
        fs::write(&path, "")
            .map_err(|e| format!("failed to create audit log {}: {e}", path.display()))?;
    }
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("failed to open audit log in explorer: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn tick_auto_retry() -> Result<AutoRetryTickResult, String> {
    let (runtime, _) = runtime_and_jobs_path()?;
    let settings = load_settings(&runtime.out_base_dir)?;
    if !settings.auto_retry_enabled {
        return Ok(AutoRetryTickResult {
            acted: false,
            job_id: None,
            pipeline_id: None,
            reason: "auto_retry_disabled".to_string(),
        });
    }

    let (state, jobs_path) = init_job_runtime()?;
    let pipelines_path = pipelines_file_path(&runtime.out_base_dir);
    let mut pipelines = load_pipelines_from_file(&pipelines_path)?;
    let now_ms = now_epoch_ms();

    let selected = {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        guard.jobs = load_jobs_from_file(&jobs_path)?;

        if guard.running_job_id.is_some() {
            return Ok(AutoRetryTickResult {
                acted: false,
                job_id: None,
                pipeline_id: None,
                reason: "worker_busy".to_string(),
            });
        }

        let mut changed_schedule = false;
        let mut candidates: Vec<(u128, String, Option<(String, String, usize)>)> = Vec::new();
        for job in &mut guard.jobs {
            if job.status != JobStatus::NeedsRetry {
                continue;
            }

            if job.retry_at.is_none() {
                job.retry_at = Some(compute_next_retry_at_ms(
                    now_ms,
                    job.retry_after_seconds,
                    job.auto_retry_attempt_count.saturating_add(1),
                    &settings,
                ));
                changed_schedule = true;
            }

            let next_ms = parse_retry_at_ms(job.retry_at.as_ref()).unwrap_or(now_ms);
            if now_ms < next_ms {
                continue;
            }
            if job.auto_retry_attempt_count >= settings.auto_retry_max_per_job {
                continue;
            }

            let mut pipeline_ref: Option<(String, String, usize)> = None;
            for (pidx, p) in pipelines.iter().enumerate() {
                let step = p.steps.iter().find(|s| s.job_id.as_deref() == Some(job.job_id.as_str()));
                if let Some(s) = step {
                    if p.auto_retry_attempt_count < settings.auto_retry_max_per_pipeline {
                        pipeline_ref = Some((p.pipeline_id.clone(), s.step_id.clone(), pidx));
                    }
                    break;
                }
            }

            if let Some((_, _, pidx)) = pipeline_ref.as_ref() {
                if pipelines[*pidx].auto_retry_attempt_count >= settings.auto_retry_max_per_pipeline {
                    continue;
                }
            }

            candidates.push((next_ms, job.job_id.clone(), pipeline_ref));
        }

        if changed_schedule {
            persist_state(&state, &jobs_path)?;
        }

        candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        candidates.first().cloned()
    };

    let Some((_next_ms, job_id, pipeline_ref)) = selected else {
        return Ok(AutoRetryTickResult {
            acted: false,
            job_id: None,
            pipeline_id: None,
            reason: "no_eligible_item".to_string(),
        });
    };

    let mut pipeline_id_for_audit: Option<String> = None;
    if let Some((pipeline_id, step_id, pidx)) = pipeline_ref {
        let _ = retry_pipeline_step(pipeline_id.clone(), step_id, Some(false))?;
        pipeline_id_for_audit = Some(pipeline_id.clone());
        if pidx < pipelines.len() {
            pipelines[pidx].auto_retry_attempt_count = pipelines[pidx].auto_retry_attempt_count.saturating_add(1);
            pipelines[pidx].updated_at = now_epoch_ms_string();
            save_pipelines_to_file(&pipelines_path, &pipelines)?;
        }
    } else {
        let _ = retry_job(job_id.clone(), Some(false))?;
    }

    let mut attempt = 0u32;
    let mut next_retry_at = None;
    {
        let mut guard = state
            .lock()
            .map_err(|_| "failed to lock job runtime".to_string())?;
        guard.jobs = load_jobs_from_file(&jobs_path)?;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.job_id == job_id) {
            job.auto_retry_attempt_count = job.auto_retry_attempt_count.saturating_add(1);
            attempt = job.auto_retry_attempt_count;
            next_retry_at = job.retry_at.clone();
        }
    }
    persist_state(&state, &jobs_path)?;

    append_audit_auto_retry(
        &runtime.out_base_dir,
        &AuditAutoRetryEntry {
            ts: now_epoch_ms_string(),
            kind: "auto_retry".to_string(),
            job_id: job_id.clone(),
            pipeline_id: pipeline_id_for_audit.clone(),
            reason: "eligible_tick".to_string(),
            next_retry_at,
            attempt,
        },
    )?;

    Ok(AutoRetryTickResult {
        acted: true,
        job_id: Some(job_id),
        pipeline_id: pipeline_id_for_audit,
        reason: "auto_retry_enqueued".to_string(),
    })
}

#[tauri::command]
fn run_task_template(
    template_id: String,
    canonical_id: String,
    params: serde_json::Value,
) -> RunResult {
    let tpl = match find_template(&template_id) {
        Some(t) => t,
        None => {
            return RunResult {
                ok: false,
                exit_code: 1,
                stdout: "".to_string(),
                stderr: format!("unknown template id: {template_id}"),
                run_id: make_run_id(),
                run_dir: "".to_string(),
                status: "error".to_string(),
                message: format!("unknown template id: {template_id}"),
                retry_after_sec: None,
            }
        }
    };

    if !tpl.wired {
        return RunResult {
            ok: false,
            exit_code: 1,
            stdout: "".to_string(),
            stderr: format!("template is not wired: {}", tpl.id),
            run_id: make_run_id(),
            run_dir: "".to_string(),
            status: "error".to_string(),
            message: format!("template is not wired: {}", tpl.id),
            retry_after_sec: None,
        };
    }

    let (argv, normalized_params) = match build_template_args(&template_id, &canonical_id, &params) {
        Ok(v) => v,
        Err(e) => {
            return RunResult {
                ok: false,
                exit_code: 1,
                stdout: "".to_string(),
                stderr: e.clone(),
                run_id: make_run_id(),
                run_dir: "".to_string(),
                status: "error".to_string(),
                message: e,
                retry_after_sec: None,
            }
        }
    };

    execute_pipeline_task(argv, template_id, canonical_id, normalized_params, None)
}

#[tauri::command]
fn run_papers_tree(paper_id: String, depth: u8, max_per_level: u32) -> RunResult {
    let params = serde_json::json!({
        "depth": depth,
        "max_per_level": max_per_level,
    });
    run_task_template("TEMPLATE_TREE".to_string(), paper_id, params)
}

#[tauri::command]
fn open_run_folder(run_dir: String) -> Result<(), String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root).ok();
    let pipeline_root = runtime
        .as_ref()
        .map(|cfg| cfg.pipeline_root.clone())
        .or_else(|| find_pipeline_root_autodetect(&root));

    let raw = run_dir.trim();
    if raw.is_empty() {
        return Err("RULE_RUN_DIR_EMPTY: run_dir is empty".to_string());
    }
    if has_disallowed_windows_prefix(raw) {
        return Err(
            "RULE_DISALLOWED_PREFIX: UNC/device-prefixed run_dir is not allowed".to_string(),
        );
    }

    let requested = PathBuf::from(raw);
    let requested_abs = if requested.is_absolute() {
        requested.clone()
    } else if let Some(ref pipeline_root) = pipeline_root {
        absolutize(&requested, pipeline_root)
    } else {
        absolutize(&requested, &root)
    };
    if !requested_abs.exists() {
        return Err(format!(
            "RULE_RUN_DIR_NOT_FOUND: run_dir does not exist: {}",
            requested_abs.display()
        ));
    }
    if !requested_abs.is_dir() {
        return Err(format!(
            "RULE_RUN_DIR_NOT_DIRECTORY: run_dir is not a directory: {}",
            requested_abs.display()
        ));
    }
    let requested_canonical = requested_abs.canonicalize().map_err(|e| {
        format!(
            "RULE_RUN_DIR_CANONICALIZE_FAILED: failed to canonicalize {}: {e}",
            requested_abs.display()
        )
    })?;

    let mut allowed_roots = Vec::new();
    let desktop_default = root.join("logs").join("runs");
    if desktop_default.exists() {
        allowed_roots.push(canonicalize_existing_dir(
            &desktop_default,
            "RULE_ALLOWED_ROOT_DESKTOP_INVALID",
        )?);
    }

    if let Some(ref pipeline_root) = pipeline_root {
        let pipeline_default = pipeline_root.join("logs").join("runs");
        if pipeline_default.exists() {
            allowed_roots.push(canonicalize_existing_dir(
                &pipeline_default,
                "RULE_ALLOWED_ROOT_PIPELINE_INVALID",
            )?);
        }
    }

    if let Some(ref runtime_cfg) = runtime {
        if runtime_cfg.out_base_dir.exists() {
            allowed_roots.push(canonicalize_existing_dir(
                &runtime_cfg.out_base_dir,
                "RULE_ALLOWED_ROOT_RUNTIME_INVALID",
            )?);
        }
    }

    if let Ok(raw_out) = std::env::var("JARVIS_PIPELINE_OUT_DIR") {
        let trimmed = raw_out.trim();
        if !trimmed.is_empty() {
            let configured = PathBuf::from(trimmed);
            let configured_abs = if configured.is_absolute() {
                configured
            } else if let Some(ref pipeline_root) = pipeline_root {
                absolutize(&configured, pipeline_root)
            } else {
                absolutize(&configured, &root)
            };
            allowed_roots.push(canonicalize_existing_dir(
                &configured_abs,
                "RULE_ALLOWED_ROOT_CONFIG_INVALID",
            )?);
        }
    }

    allowed_roots.sort();
    allowed_roots.dedup();
    if allowed_roots.is_empty() {
        // If no canonical roots are available, fail closed.
        return Err(
            "RULE_NO_ALLOWED_ROOTS: no canonical allowed roots are available (logs/runs missing)"
                .to_string(),
        );
    }

    let allowed = allowed_roots
        .iter()
        .any(|allowed_root| requested_canonical.starts_with(allowed_root));
    if !allowed {
        let allowed_text = allowed_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "RULE_RUN_DIR_OUTSIDE_ALLOWED_ROOTS: {} is outside allowed roots: {}",
            requested_canonical.display(),
            allowed_text
        ));
    }

    Command::new("explorer")
        .arg(&requested_canonical)
        .spawn()
        .map_err(|e| format!("Failed to open explorer: {e}"))?;

    Ok(())
}

#[tauri::command]
fn get_runtime_config() -> RuntimeConfigView {
    let root = repo_root();
    runtime_config_view_from_result(resolve_runtime_config(&root))
}

#[tauri::command]
fn normalize_identifier(input: String) -> NormalizedIdentifier {
    normalize_identifier_internal(&input)
}

#[tauri::command]
fn preflight_check() -> PreflightResult {
    run_preflight_checks()
}

#[tauri::command]
fn reload_runtime_config() -> RuntimeConfigView {
    get_runtime_config()
}

#[tauri::command]
fn open_config_file_location() -> Result<String, String> {
    let path = config_file_path();
    ensure_config_file_template(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("No parent directory for config file: {}", path.display()))?;
    Command::new("explorer")
        .arg(parent)
        .spawn()
        .map_err(|e| format!("Failed to open config directory in explorer: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn create_config_if_missing() -> Result<String, String> {
    let path = config_file_path();
    ensure_config_file_template(&path)?;
    Ok(path.to_string_lossy().to_string())
}

fn resume_pipelines_if_possible() {
    let (runtime, _) = match runtime_and_jobs_path() {
        Ok(v) => v,
        Err(_) => return,
    };
    let (state, jobs_path) = match init_job_runtime() {
        Ok(v) => v,
        Err(_) => return,
    };
    let _ = reconcile_pipelines_with_jobs(&runtime.out_base_dir, &state, &jobs_path, None);
    let _ = start_job_worker_if_needed();
}

fn main() {
    let _ = start_job_worker_if_needed();
    resume_pipelines_if_possible();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            run_papers_tree,
            run_task_template,
            enqueue_job,
            list_jobs,
            cancel_job,
            retry_job,
            create_pipeline,
            list_pipelines,
            get_pipeline,
            start_pipeline,
            cancel_pipeline,
            retry_pipeline_step,
            get_settings,
            update_settings,
            open_audit_log,
            tick_auto_retry,
            clear_finished_jobs,
            library_reindex,
            library_reload,
            library_list,
            library_search,
            library_get,
            library_set_tags,
            library_stats,
            open_run_folder,
            list_task_templates,
            list_runs,
            get_run_status,
            collect_diagnostics,
            list_diagnostics,
            read_diagnostic_report,
            open_diagnostic_folder,
            read_run_artifact,
            list_run_artifacts,
            read_run_artifact_named,
            parse_graph_json,
            normalize_identifier,
            preflight_check,
            get_runtime_config,
            reload_runtime_config,
            open_config_file_location,
            create_config_if_missing
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_precedence_is_file_then_env_then_autodetect() {
        let selected =
            first_from_precedence(Some("C:/file-root"), Some("C:/env-root"), Some("C:/auto"));
        assert_eq!(selected.as_deref(), Some("C:/file-root"));

        let selected = first_from_precedence(None, Some("C:/env-root"), Some("C:/auto"));
        assert_eq!(selected.as_deref(), Some("C:/env-root"));

        let selected = first_from_precedence(None, None, Some("C:/auto"));
        assert_eq!(selected.as_deref(), Some("C:/auto"));
    }

    #[test]
    fn status_maps_429_to_needs_retry_even_when_exit_nonzero() {
        let status = read_status(
            "",
            "S2 retry exhausted: status=429 url=https://api.semanticscholar.org/graph/v1/paper/...",
            1,
        );
        assert_eq!(status, "needs_retry");
    }

    #[test]
    fn retry_message_formats_retry_after_seconds() {
        let raw = "S2 retry exhausted: status=429 retry_count=6 wait_seconds=12.35";
        let sec = extract_retry_after_seconds(raw);
        assert_eq!(sec, Some(12.35));
        let msg = build_status_message("needs_retry", "", raw, sec);
        assert!(msg.to_lowercase().contains("retry after"));
        assert!(msg.contains("12."));
    }

    #[test]
    fn normalize_identifier_doi_variants() {
        let from_url = normalize_identifier_internal("https://doi.org/10.1234/AbCd");
        assert_eq!(from_url.kind, "doi");
        assert_eq!(from_url.canonical, "10.1234/abcd");

        let from_prefix = normalize_identifier_internal("doi:10.5555/XYZ");
        assert_eq!(from_prefix.kind, "doi");
        assert_eq!(from_prefix.canonical, "10.5555/xyz");

        let from_raw = normalize_identifier_internal("10.1000/182");
        assert_eq!(from_raw.kind, "doi");
        assert_eq!(from_raw.canonical, "10.1000/182");
    }

    #[test]
    fn normalize_identifier_pmid_variants() {
        let from_url = normalize_identifier_internal("https://pubmed.ncbi.nlm.nih.gov/12345678/");
        assert_eq!(from_url.kind, "pmid");
        assert_eq!(from_url.canonical, "pmid:12345678");

        let from_prefix = normalize_identifier_internal("pmid:87654321");
        assert_eq!(from_prefix.kind, "pmid");
        assert_eq!(from_prefix.canonical, "pmid:87654321");

        let from_raw = normalize_identifier_internal("24681357");
        assert_eq!(from_raw.kind, "pmid");
        assert_eq!(from_raw.canonical, "pmid:24681357");
    }

    #[test]
    fn normalize_identifier_arxiv_variants() {
        let from_url = normalize_identifier_internal("https://arxiv.org/abs/2301.01234");
        assert_eq!(from_url.kind, "arxiv");
        assert_eq!(from_url.canonical, "arxiv:2301.01234");

        let from_prefix = normalize_identifier_internal("arxiv:1706.03762");
        assert_eq!(from_prefix.kind, "arxiv");
        assert_eq!(from_prefix.canonical, "arxiv:1706.03762");

        let from_raw = normalize_identifier_internal("2301.01234");
        assert_eq!(from_raw.kind, "arxiv");
        assert_eq!(from_raw.canonical, "arxiv:2301.01234");
    }

    #[test]
    fn normalize_identifier_s2_variants() {
        let from_url = normalize_identifier_internal(
            "https://www.semanticscholar.org/paper/Attention-Is-All-You-Need/204e3073870fae3d05bcbc2f6a8e263d9b72e776",
        );
        assert_eq!(from_url.kind, "s2");
        assert!(from_url.canonical.starts_with("S2PaperId:"));

        let from_corpus = normalize_identifier_internal("CorpusId:12345");
        assert_eq!(from_corpus.kind, "s2");
        assert_eq!(from_corpus.canonical, "CorpusId:12345");
    }

    #[test]
    fn normalize_identifier_invalid_string() {
        let invalid = normalize_identifier_internal("not-an-id???");
        assert_eq!(invalid.kind, "unknown");
        assert!(!invalid.errors.is_empty());
    }

    #[test]
    fn template_registry_defaults_are_stable() {
        let templates = template_registry();
        let tree = templates
            .iter()
            .find(|t| t.id == "TEMPLATE_TREE")
            .expect("TEMPLATE_TREE missing");
        assert!(tree.wired);
        assert_eq!(tree.params.len(), 2);

        let depth = tree
            .params
            .iter()
            .find(|p| p.key == "depth")
            .expect("depth param missing");
        assert_eq!(depth.default_value, serde_json::json!(2));

        let max_per_level = tree
            .params
            .iter()
            .find(|p| p.key == "max_per_level")
            .expect("max_per_level param missing");
        assert_eq!(max_per_level.default_value, serde_json::json!(50));
    }

    #[test]
    fn template_build_args_are_deterministic() {
        let params = serde_json::json!({ "depth": 1, "max_per_level": 5 });
        let (argv, normalized_params) = build_template_args("TEMPLATE_TREE", "arxiv:1706.03762", &params)
            .expect("build args failed");

        let expected = vec![
            "papers".to_string(),
            "tree".to_string(),
            "--id".to_string(),
            "arxiv:1706.03762".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "--max-per-level".to_string(),
            "5".to_string(),
        ];
        assert_eq!(argv, expected);
        assert_eq!(normalized_params["depth"], serde_json::json!(1));
        assert_eq!(normalized_params["max_per_level"], serde_json::json!(5));
    }

    #[test]
    fn template_build_args_for_map_related_graph_are_deterministic() {
        let related_params = serde_json::json!({ "depth": 2, "max_per_level": 12 });
        let (related_argv, related_normalized) =
            build_template_args("TEMPLATE_RELATED", "doi:10.1000/abc", &related_params)
                .expect("build related args failed");
        assert_eq!(
            related_argv,
            vec![
                "papers".to_string(),
                "tree".to_string(),
                "--id".to_string(),
                "doi:10.1000/abc".to_string(),
                "--depth".to_string(),
                "2".to_string(),
                "--max-per-level".to_string(),
                "12".to_string(),
            ]
        );
        assert_eq!(related_normalized, serde_json::json!({"depth": 2, "max_per_level": 12}));

        let map_params = serde_json::json!({ "k": 22, "seed": 7 });
        let (map_argv, map_normalized) =
            build_template_args("TEMPLATE_MAP", "arxiv:1706.03762", &map_params)
                .expect("build map args failed");
        assert_eq!(
            map_argv,
            vec![
                "papers".to_string(),
                "map3d".to_string(),
                "--id".to_string(),
                "arxiv:1706.03762".to_string(),
                "--k".to_string(),
                "22".to_string(),
                "--seed".to_string(),
                "7".to_string(),
            ]
        );
        assert_eq!(map_normalized, serde_json::json!({"k": 22, "seed": 7}));

        let graph_defaults = serde_json::json!({});
        let (graph_argv, graph_normalized) =
            build_template_args("TEMPLATE_GRAPH", "pmid:12345678", &graph_defaults)
                .expect("build graph args failed");
        assert_eq!(
            graph_argv,
            vec![
                "papers".to_string(),
                "map3d".to_string(),
                "--id".to_string(),
                "pmid:12345678".to_string(),
                "--k".to_string(),
                "40".to_string(),
                "--seed".to_string(),
                "42".to_string(),
            ]
        );
        assert_eq!(graph_normalized, serde_json::json!({"k": 40, "seed": 42}));
    }

    #[test]
    fn primary_viz_selection_prefers_html_then_graph_json() {
        let items = vec![
            ArtifactItem {
                name: "z_graph.json".to_string(),
                rel_path: "z_graph.json".to_string(),
                kind: "graph_json".to_string(),
                size_bytes: Some(10),
                mtime_iso: None,
            },
            ArtifactItem {
                name: "b_map.html".to_string(),
                rel_path: "nested/b_map.html".to_string(),
                kind: "html".to_string(),
                size_bytes: Some(10),
                mtime_iso: None,
            },
            ArtifactItem {
                name: "a_map.html".to_string(),
                rel_path: "a_map.html".to_string(),
                kind: "html".to_string(),
                size_bytes: Some(10),
                mtime_iso: None,
            },
        ];

        let picked = select_primary_viz_artifact(&items).expect("primary viz should exist");
        assert_eq!(picked.kind, "html");
        assert_eq!(picked.name, "a_map.html");
    }

    #[test]
    fn merge_input_metadata_is_non_destructive() {
        let base = std::env::temp_dir().join(format!("jarvis_input_merge_{}", now_epoch_ms()));
        let run_dir = base.join("run_1");
        let _ = fs::create_dir_all(&run_dir);
        fs::write(
            run_dir.join("input.json"),
            r#"{"title":"A","request":{"id":"x"},"desktop":{"custom":"keep"}}"#,
        )
        .expect("write input");

        let pv = PrimaryVizRef {
            name: "map.html".to_string(),
            kind: "html".to_string(),
        };
        merge_desktop_input_metadata(
            &run_dir,
            "TEMPLATE_MAP",
            "arxiv:1706.03762",
            &serde_json::json!({"k": 24, "seed": 42}),
            Some(&pv),
        )
        .expect("merge input metadata");

        let updated_raw = fs::read_to_string(run_dir.join("input.json")).expect("read merged input");
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).expect("parse merged input");
        assert_eq!(updated.get("title"), Some(&serde_json::json!("A")));
        assert_eq!(updated.get("request").and_then(|v| v.get("id")), Some(&serde_json::json!("x")));
        assert_eq!(updated.get("desktop").and_then(|v| v.get("custom")), Some(&serde_json::json!("keep")));
        assert_eq!(updated.get("desktop").and_then(|v| v.get("template_id")), Some(&serde_json::json!("TEMPLATE_MAP")));
        assert_eq!(updated.get("desktop").and_then(|v| v.get("primary_viz")).and_then(|v| v.get("kind")), Some(&serde_json::json!("html")));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn job_persistence_roundtrip() {
        let base = std::env::temp_dir().join(format!("jarvis_job_rt_{}", now_epoch_ms()));
        let jobs_path = base.join("jobs.json");
        let jobs = vec![JobRecord {
            job_id: "job_1".to_string(),
            template_id: "TEMPLATE_TREE".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            params: serde_json::json!({"depth": 1, "max_per_level": 5}),
            status: JobStatus::Queued,
            attempt: 0,
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            run_id: None,
            last_error: None,
            retry_after_seconds: None,
            retry_at: None,
            auto_retry_attempt_count: 0,
        }];

        save_jobs_to_file(&jobs_path, &jobs).expect("save jobs failed");
        let loaded = load_jobs_from_file(&jobs_path).expect("load jobs failed");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].job_id, "job_1");

        let _ = fs::remove_file(&jobs_path);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn job_state_transition_queued_running_succeeded() {
        let mut job = JobRecord {
            job_id: "job_a".to_string(),
            template_id: "TEMPLATE_TREE".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            params: serde_json::json!({}),
            status: JobStatus::Queued,
            attempt: 0,
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            run_id: None,
            last_error: None,
            retry_after_seconds: None,
            retry_at: None,
            auto_retry_attempt_count: 0,
        };

        job.status = JobStatus::Running;
        job.attempt += 1;
        apply_mock_transition(
            &mut job,
            JobStatus::Succeeded,
            Some("run_1".to_string()),
            None,
            None,
        );

        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.attempt, 1);
        assert_eq!(job.run_id.as_deref(), Some("run_1"));
    }

    #[test]
    fn job_state_transition_needs_retry_and_retry_queue() {
        let mut job = JobRecord {
            job_id: "job_b".to_string(),
            template_id: "TEMPLATE_TREE".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            params: serde_json::json!({}),
            status: JobStatus::Running,
            attempt: 1,
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            run_id: Some("run_2".to_string()),
            last_error: None,
            retry_after_seconds: None,
            retry_at: None,
            auto_retry_attempt_count: 0,
        };

        apply_mock_transition(
            &mut job,
            JobStatus::NeedsRetry,
            Some("run_2".to_string()),
            Some("429".to_string()),
            Some(3.0),
        );
        assert_eq!(job.status, JobStatus::NeedsRetry);
        assert_eq!(job.retry_after_seconds, Some(3.0));
        assert!(job.retry_at.is_some());

        job.status = JobStatus::Queued;
        job.retry_after_seconds = None;
        job.retry_at = None;
        assert_eq!(job.status, JobStatus::Queued);
    }

    #[test]
    fn library_extract_with_and_without_artifacts() {
        let base = std::env::temp_dir().join(format!("jarvis_lib_extract_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&base);

        let run1 = base.join("run_a");
        let _ = fs::create_dir_all(&run1);
        fs::write(
            run1.join("input.json"),
            r#"{"desktop":{"canonical_id":"arxiv:1706.03762","template_id":"TEMPLATE_TREE"},"title":"A"}"#,
        )
        .expect("write input run1");
        fs::write(run1.join("result.json"), r#"{"status":"succeeded","year":2017}"#)
            .expect("write result run1");

        let run2 = base.join("run_b");
        let _ = fs::create_dir_all(&run2);

        let e1 = extract_run_for_library(&run1).expect("extract run1");
        assert_eq!(e1.0, "arxiv:1706.03762");
        assert_eq!(e1.1.status, "succeeded");

        let e2 = extract_run_for_library(&run2).expect("extract run2");
        assert_eq!(e2.0, "run:run_b");
        assert_eq!(e2.1.status, "unknown");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn library_rebuild_is_deterministic() {
        let base = std::env::temp_dir().join(format!("jarvis_lib_det_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&base);

        let run1 = base.join("run_1");
        let run2 = base.join("run_2");
        let _ = fs::create_dir_all(&run1);
        let _ = fs::create_dir_all(&run2);
        fs::write(
            run1.join("input.json"),
            r#"{"desktop":{"canonical_id":"doi:10.1/abc","template_id":"TEMPLATE_TREE"}}"#,
        )
        .expect("write run1 input");
        fs::write(run1.join("result.json"), r#"{"status":"failed"}"#).expect("write run1 result");
        fs::write(
            run2.join("input.json"),
            r#"{"desktop":{"canonical_id":"arxiv:1706.03762","template_id":"TEMPLATE_TREE"}}"#,
        )
        .expect("write run2 input");
        fs::write(run2.join("result.json"), r#"{"status":"succeeded"}"#).expect("write run2 result");

        let r1 = build_library_records(&base, &[]).expect("build first");
        let r2 = build_library_records(&base, &[]).expect("build second");
        let s1 = serde_json::to_string(&r1).expect("ser1");
        let s2 = serde_json::to_string(&r2).expect("ser2");
        assert_eq!(s1, s2);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn library_set_tags_persistence_roundtrip() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_lib_tags_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&out_dir);

        let rec = LibraryRecord {
            paper_key: "arxiv:1706.03762".to_string(),
            canonical_id: Some("arxiv:1706.03762".to_string()),
            title: None,
            year: None,
            source_kind: Some("arxiv".to_string()),
            tags: vec!["old".to_string()],
            runs: vec![],
            primary_viz: None,
            last_run_id: None,
            last_status: "unknown".to_string(),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        write_library_records(&out_dir, &[rec]).expect("write initial library");

        let mut loaded = read_library_records(&out_dir).expect("load initial library");
        assert_eq!(loaded.len(), 1);
        loaded[0].tags = vec!["tag1".to_string(), "tag2".to_string()];
        write_library_records(&out_dir, &loaded).expect("write updated library");

        let reloaded = read_library_records(&out_dir).expect("reload updated library");
        assert_eq!(reloaded[0].tags, vec!["tag1".to_string(), "tag2".to_string()]);

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn library_search_ranking_is_deterministic() {
        let now = Utc::now().to_rfc3339();
        let rec = LibraryRecord {
            paper_key: "arxiv:1706.03762".to_string(),
            canonical_id: Some("arxiv:1706.03762".to_string()),
            title: Some("Attention Is All You Need".to_string()),
            year: Some(2017),
            source_kind: Some("arxiv".to_string()),
            tags: vec!["transformer".to_string()],
            runs: vec![LibraryRunEntry {
                run_id: "20260218_abc".to_string(),
                template_id: Some("TEMPLATE_TREE".to_string()),
                status: "succeeded".to_string(),
                primary_viz: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            }],
            primary_viz: None,
            last_run_id: Some("20260218_abc".to_string()),
            last_status: "succeeded".to_string(),
            created_at: now.clone(),
            updated_at: now,
        };

        let tokens = tokenize_query("arxiv:1706.03762 transformer template_tree");
        let (score, _, matched) = score_library_record(&rec, &tokens);
        assert!(matched);
        assert!(score >= 140);
    }

    #[test]
    fn library_search_tokenization_trims_and_lowers() {
        let tokens = tokenize_query("  DOI:10.1000/XYZ   failed ");
        assert_eq!(tokens, vec!["doi:10.1000/xyz".to_string(), "failed".to_string()]);
    }

    #[test]
    fn list_run_artifacts_returns_safe_relative_paths() {
        let run_dir = std::env::temp_dir().join(format!("jarvis_artifacts_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(run_dir.join("paper_graph").join("tree"));
        fs::write(run_dir.join("paper_graph").join("tree").join("tree.md"), "# tree")
            .expect("write tree");
        fs::write(run_dir.join("result.json"), "{}")
            .expect("write result");

        let items = list_run_artifacts_internal(&run_dir).expect("list artifacts");
        assert!(items.iter().any(|a| a.name == "tree.md"));
        assert!(items.iter().all(|a| !a.rel_path.starts_with("..")));
        assert!(items.iter().all(|a| !PathBuf::from(&a.rel_path).is_absolute()));

        let _ = fs::remove_dir_all(&run_dir);
    }

    #[test]
    fn artifact_name_rejects_traversal_patterns() {
        let run_dir = std::env::temp_dir().join(format!("jarvis_artifacts_bad_name_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&run_dir);
        fs::write(run_dir.join("result.json"), "{}")
            .expect("write result");

        let bad = resolve_named_artifact_from_catalog(&run_dir, "../result.json");
        assert!(bad.is_err());
        let slash = resolve_named_artifact_from_catalog(&run_dir, "paper_graph/tree/tree.md");
        assert!(slash.is_err());

        let _ = fs::remove_dir_all(&run_dir);
    }

    #[test]
    fn artifact_catalog_order_is_deterministic() {
        let run_dir = std::env::temp_dir().join(format!("jarvis_artifacts_order_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(run_dir.join("paper_graph").join("tree"));
        fs::write(run_dir.join("paper_graph").join("tree").join("tree.md"), "# tree")
            .expect("write tree");
        fs::write(run_dir.join("a.json"), "{}")
            .expect("write a json");
        fs::write(run_dir.join("z.log"), "ok")
            .expect("write z log");

        let first = list_run_artifacts_internal(&run_dir).expect("list first");
        let second = list_run_artifacts_internal(&run_dir).expect("list second");
        let s1 = serde_json::to_string(&first).expect("ser first");
        let s2 = serde_json::to_string(&second).expect("ser second");
        assert_eq!(s1, s2);

        let _ = fs::remove_dir_all(&run_dir);
    }

    #[test]
    fn artifact_size_limit_returns_truncated_message() {
        let run_dir = std::env::temp_dir().join(format!("jarvis_artifacts_size_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&run_dir);
        let big = "A".repeat((MAX_ARTIFACT_READ_BYTES + 1024) as usize);
        fs::write(run_dir.join("stdout.log"), big).expect("write big log");

        let item = ArtifactItem {
            name: "stdout.log".to_string(),
            rel_path: "stdout.log".to_string(),
            kind: "text".to_string(),
            size_bytes: None,
            mtime_iso: None,
        };
        let view = read_artifact_content_internal(&run_dir, &item).expect("read item");
        assert!(view.truncated);
        assert!(view.content.to_lowercase().contains("too large"));

        let _ = fs::remove_dir_all(&run_dir);
    }

    #[test]
    fn classify_graph_json_by_name_and_structure() {
        let run_dir = std::env::temp_dir().join(format!("jarvis_artifacts_graph_kind_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(&run_dir);

        let named = run_dir.join("my_graph_payload.json");
        fs::write(&named, r#"{"x":1}"#).expect("write named graph");
        let kind_named = classify_artifact_kind(&named, "my_graph_payload.json", Some(7));
        assert_eq!(kind_named, "graph_json");

        let structured = run_dir.join("payload.json");
        fs::write(&structured, r#"{"nodes":[],"edges":[]}"#).expect("write structured graph");
        let size = fs::metadata(&structured).expect("meta structured").len();
        let kind_structured = classify_artifact_kind(&structured, "payload.json", Some(size));
        assert_eq!(kind_structured, "graph_json");

        let _ = fs::remove_dir_all(&run_dir);
    }

    #[test]
    fn sandboxed_html_inserts_csp_and_removes_scripts() {
        let raw = r#"<html><head><script>alert(1)</script></head><body><a href="https://example.com">x</a></body></html>"#;
        let (safe, warnings) = build_sandboxed_html(raw);
        assert!(safe.to_lowercase().contains("content-security-policy"));
        assert!(!safe.to_lowercase().contains("<script"));
        assert!(warnings.iter().any(|w| w.contains("scripts were removed")));
        assert!(warnings.iter().any(|w| w.contains("external refs detected")));
    }

    fn degree_map_for_test(edges: &[GraphEdgeNormalized]) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for e in edges {
            *out.entry(e.source.clone()).or_insert(0) += 1;
            *out.entry(e.target.clone()).or_insert(0) += 1;
        }
        out
    }

    #[test]
    fn parse_graph_json_top_level_nodes_edges() {
        let raw = r#"{"nodes":[{"id":"n1","label":"A"},{"id":"n2"}],"edges":[{"source":"n1","target":"n2"}]}"#;
        let parsed = parse_graph_json_internal(raw).expect("parse graph top level");
        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.nodes[0].id, "n1");
        assert!(parsed.stats.top_level_keys.contains(&"edges".to_string()));
        assert!(parsed.stats.top_level_keys.contains(&"nodes".to_string()));
    }

    #[test]
    fn parse_graph_json_nested_graph_variant() {
        let raw = r#"{"graph":{"nodes":[{"id":"x"}],"edges":[{"from":"x","to":"x"}]}}"#;
        let parsed = parse_graph_json_internal(raw).expect("parse nested graph");
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.edges.len(), 1);
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("nested key `graph`")));
    }

    #[test]
    fn degree_computation_is_stable() {
        let raw = r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"}],"edges":[{"source":"a","target":"b"},{"source":"a","target":"c"}]}"#;
        let parsed = parse_graph_json_internal(raw).expect("parse for degree");
        let degree = degree_map_for_test(&parsed.edges);
        assert_eq!(degree.get("a"), Some(&2));
        assert_eq!(degree.get("b"), Some(&1));
        assert_eq!(degree.get("c"), Some(&1));
    }

    #[test]
    fn parse_graph_json_unknown_schema_fallback() {
        let raw = r#"{"items":[1,2,3],"meta":{"x":1}}"#;
        let parsed = parse_graph_json_internal(raw).expect("parse unknown schema");
        assert_eq!(parsed.nodes.len(), 0);
        assert_eq!(parsed.edges.len(), 0);
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("fallback summary mode")));
    }

    #[test]
    fn pipeline_persistence_roundtrip() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_pipe_rt_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let path = pipelines_file_path(&out_dir);

        let data = vec![PipelineRecord {
            pipeline_id: "pipe_1".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            name: "Analyze Paper".to_string(),
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            steps: vec![PipelineStep {
                step_id: "step_01_template_tree".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                job_id: None,
                status: PipelineStepStatus::Pending,
                run_id: None,
                started_at: None,
                finished_at: None,
            }],
            current_step_index: 0,
            status: PipelineStatus::Running,
            last_primary_viz: None,
            auto_retry_attempt_count: 0,
        }];

        save_pipelines_to_file(&path, &data).expect("save pipelines");
        let loaded = load_pipelines_from_file(&path).expect("load pipelines");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].pipeline_id, "pipe_1");
        assert_eq!(loaded[0].steps[0].template_id, "TEMPLATE_TREE");

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn pipeline_transition_success_enqueues_next_step() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_pipe_success_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let state = Arc::new(Mutex::new(JobRuntimeState::default()));
        let jobs_path = jobs_file_path(&out_dir);
        save_jobs_to_file(&jobs_path, &[]).expect("save empty jobs");

        let pipeline = PipelineRecord {
            pipeline_id: "pipe_a".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            name: "Analyze".to_string(),
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            steps: vec![
                PipelineStep {
                    step_id: "step_01_template_tree".to_string(),
                    template_id: "TEMPLATE_TREE".to_string(),
                    params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                    job_id: None,
                    status: PipelineStepStatus::Pending,
                    run_id: None,
                    started_at: None,
                    finished_at: None,
                },
                PipelineStep {
                    step_id: "step_02_template_related".to_string(),
                    template_id: "TEMPLATE_RELATED".to_string(),
                    params: serde_json::json!({"depth": 1, "max_per_level": 20}),
                    job_id: None,
                    status: PipelineStepStatus::Pending,
                    run_id: None,
                    started_at: None,
                    finished_at: None,
                },
            ],
            current_step_index: 0,
            status: PipelineStatus::Running,
            last_primary_viz: None,
            auto_retry_attempt_count: 0,
        };
        save_pipelines_to_file(&pipelines_file_path(&out_dir), &[pipeline]).expect("save pipeline");

        let first = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, None).expect("reconcile first");
        let first_job_id = first[0].steps[0].job_id.clone().expect("step1 job id");
        let mut jobs = load_jobs_from_file(&jobs_path).expect("load jobs after first reconcile");
        assert_eq!(jobs.len(), 1);
        jobs[0].status = JobStatus::Succeeded;
        jobs[0].run_id = Some("run_success_step1".to_string());
        save_jobs_to_file(&jobs_path, &jobs).expect("save succeeded job");

        let second = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, Some(&first_job_id))
            .expect("reconcile second");
        assert_eq!(second[0].steps[0].status, PipelineStepStatus::Succeeded);
        assert_eq!(second[0].current_step_index, 1);
        assert_eq!(second[0].steps[1].status, PipelineStepStatus::Running);
        assert!(second[0].steps[1].job_id.is_some());

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn pipeline_needs_retry_stops_without_continuation() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_pipe_retry_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let state = Arc::new(Mutex::new(JobRuntimeState::default()));
        let jobs_path = jobs_file_path(&out_dir);

        let job_id = "job_retry_1".to_string();
        save_jobs_to_file(
            &jobs_path,
            &[JobRecord {
                job_id: job_id.clone(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1706.03762".to_string(),
                params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                status: JobStatus::NeedsRetry,
                attempt: 1,
                created_at: now_epoch_ms_string(),
                updated_at: now_epoch_ms_string(),
                run_id: Some("run_retry_step1".to_string()),
                last_error: Some("429".to_string()),
                retry_after_seconds: Some(3.0),
                retry_at: Some((now_epoch_ms() + 3000).to_string()),
                auto_retry_attempt_count: 0,
            }],
        )
        .expect("save jobs");

        let pipeline = PipelineRecord {
            pipeline_id: "pipe_b".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            name: "Analyze".to_string(),
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            steps: vec![
                PipelineStep {
                    step_id: "step_01_template_tree".to_string(),
                    template_id: "TEMPLATE_TREE".to_string(),
                    params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                    job_id: Some(job_id.clone()),
                    status: PipelineStepStatus::Running,
                    run_id: None,
                    started_at: Some(now_epoch_ms_string()),
                    finished_at: None,
                },
                PipelineStep {
                    step_id: "step_02_template_graph".to_string(),
                    template_id: "TEMPLATE_GRAPH".to_string(),
                    params: serde_json::json!({"k": 40, "seed": 42}),
                    job_id: None,
                    status: PipelineStepStatus::Pending,
                    run_id: None,
                    started_at: None,
                    finished_at: None,
                },
            ],
            current_step_index: 0,
            status: PipelineStatus::Running,
            last_primary_viz: None,
            auto_retry_attempt_count: 0,
        };
        save_pipelines_to_file(&pipelines_file_path(&out_dir), &[pipeline]).expect("save pipeline");

        let rows = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, Some(&job_id))
            .expect("reconcile needs_retry");
        assert_eq!(rows[0].status, PipelineStatus::NeedsRetry);
        assert_eq!(rows[0].steps[0].status, PipelineStepStatus::NeedsRetry);
        assert_eq!(rows[0].steps[1].status, PipelineStepStatus::Pending);
        assert!(rows[0].steps[1].job_id.is_none());

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn pipeline_restart_resume_does_not_duplicate_enqueue() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_pipe_resume_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let state = Arc::new(Mutex::new(JobRuntimeState::default()));
        let jobs_path = jobs_file_path(&out_dir);
        save_jobs_to_file(&jobs_path, &[]).expect("save empty jobs");

        let pipeline = PipelineRecord {
            pipeline_id: "pipe_c".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            name: "Analyze".to_string(),
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            steps: vec![PipelineStep {
                step_id: "step_01_template_tree".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                job_id: None,
                status: PipelineStepStatus::Pending,
                run_id: None,
                started_at: None,
                finished_at: None,
            }],
            current_step_index: 0,
            status: PipelineStatus::Running,
            last_primary_viz: None,
            auto_retry_attempt_count: 0,
        };
        save_pipelines_to_file(&pipelines_file_path(&out_dir), &[pipeline]).expect("save pipeline");

        let _ = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, None).expect("first resume");
        let jobs_first = load_jobs_from_file(&jobs_path).expect("load jobs after first");
        assert_eq!(jobs_first.len(), 1);

        let _ = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, None).expect("second resume");
        let jobs_second = load_jobs_from_file(&jobs_path).expect("load jobs after second");
        assert_eq!(jobs_second.len(), 1);

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn pipeline_cancellation_propagates_correctly() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_pipe_cancel_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let state = Arc::new(Mutex::new(JobRuntimeState::default()));
        let jobs_path = jobs_file_path(&out_dir);

        let job_id = "job_cancel_1".to_string();
        save_jobs_to_file(
            &jobs_path,
            &[JobRecord {
                job_id: job_id.clone(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1706.03762".to_string(),
                params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                status: JobStatus::Canceled,
                attempt: 1,
                created_at: now_epoch_ms_string(),
                updated_at: now_epoch_ms_string(),
                run_id: None,
                last_error: Some("canceled".to_string()),
                retry_after_seconds: None,
                retry_at: None,
                auto_retry_attempt_count: 0,
            }],
        )
        .expect("save canceled job");

        let pipeline = PipelineRecord {
            pipeline_id: "pipe_d".to_string(),
            canonical_id: "arxiv:1706.03762".to_string(),
            name: "Analyze".to_string(),
            created_at: now_epoch_ms_string(),
            updated_at: now_epoch_ms_string(),
            steps: vec![PipelineStep {
                step_id: "step_01_template_tree".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                params: serde_json::json!({"depth": 1, "max_per_level": 5}),
                job_id: Some(job_id.clone()),
                status: PipelineStepStatus::Running,
                run_id: None,
                started_at: Some(now_epoch_ms_string()),
                finished_at: None,
            }],
            current_step_index: 0,
            status: PipelineStatus::Running,
            last_primary_viz: None,
            auto_retry_attempt_count: 0,
        };
        save_pipelines_to_file(&pipelines_file_path(&out_dir), &[pipeline]).expect("save pipeline");

        let rows = reconcile_pipelines_with_jobs(&out_dir, &state, &jobs_path, Some(&job_id))
            .expect("reconcile cancel");
        assert_eq!(rows[0].status, PipelineStatus::Canceled);
        assert_eq!(rows[0].steps[0].status, PipelineStepStatus::Canceled);

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn needs_attention_filter_logic_matches_failed_and_retry_only() {
        assert!(is_needs_attention_job_status(&JobStatus::Failed));
        assert!(is_needs_attention_job_status(&JobStatus::NeedsRetry));
        assert!(!is_needs_attention_job_status(&JobStatus::Queued));
        assert!(!is_needs_attention_job_status(&JobStatus::Running));
        assert!(!is_needs_attention_job_status(&JobStatus::Succeeded));
        assert!(!is_needs_attention_job_status(&JobStatus::Canceled));

        assert!(is_needs_attention_pipeline_status(&PipelineStatus::Failed));
        assert!(is_needs_attention_pipeline_status(&PipelineStatus::NeedsRetry));
        assert!(!is_needs_attention_pipeline_status(&PipelineStatus::Running));
        assert!(!is_needs_attention_pipeline_status(&PipelineStatus::Succeeded));
        assert!(!is_needs_attention_pipeline_status(&PipelineStatus::Canceled));
    }

    #[test]
    fn deterministic_sorting_for_jobs_and_runs() {
        let mut jobs = vec![
            JobRecord {
                job_id: "job_b".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1".to_string(),
                params: serde_json::json!({}),
                status: JobStatus::Queued,
                attempt: 0,
                created_at: "1".to_string(),
                updated_at: "100".to_string(),
                run_id: None,
                last_error: None,
                retry_after_seconds: None,
                retry_at: None,
                auto_retry_attempt_count: 0,
            },
            JobRecord {
                job_id: "job_a".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1".to_string(),
                params: serde_json::json!({}),
                status: JobStatus::Queued,
                attempt: 0,
                created_at: "1".to_string(),
                updated_at: "100".to_string(),
                run_id: None,
                last_error: None,
                retry_after_seconds: None,
                retry_at: None,
                auto_retry_attempt_count: 0,
            },
            JobRecord {
                job_id: "job_c".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1".to_string(),
                params: serde_json::json!({}),
                status: JobStatus::Queued,
                attempt: 0,
                created_at: "1".to_string(),
                updated_at: "101".to_string(),
                run_id: None,
                last_error: None,
                retry_after_seconds: None,
                retry_at: None,
                auto_retry_attempt_count: 0,
            },
        ];
        sort_jobs_for_display(&mut jobs);
        assert_eq!(jobs[0].job_id, "job_c");
        assert_eq!(jobs[1].job_id, "job_a");
        assert_eq!(jobs[2].job_id, "job_b");

        let mut runs = vec![
            RunListItem {
                run_id: "run_b".to_string(),
                status: "ok".to_string(),
                created_at_epoch_ms: 10,
                mtime_epoch_ms: 10,
                paper_id: "arxiv:1".to_string(),
                primary_viz: None,
                run_dir: "x".to_string(),
            },
            RunListItem {
                run_id: "run_a".to_string(),
                status: "ok".to_string(),
                created_at_epoch_ms: 10,
                mtime_epoch_ms: 10,
                paper_id: "arxiv:1".to_string(),
                primary_viz: None,
                run_dir: "x".to_string(),
            },
            RunListItem {
                run_id: "run_c".to_string(),
                status: "ok".to_string(),
                created_at_epoch_ms: 11,
                mtime_epoch_ms: 11,
                paper_id: "arxiv:1".to_string(),
                primary_viz: None,
                run_dir: "x".to_string(),
            },
        ];
        sort_runs_for_display(&mut runs);
        assert_eq!(runs[0].run_id, "run_c");
        assert_eq!(runs[1].run_id, "run_a");
        assert_eq!(runs[2].run_id, "run_b");
    }

    #[test]
    fn auto_retry_schedule_prefers_retry_after_header() {
        let settings = DesktopSettings::default();
        let now_ms = 1_000u128;
        let next = compute_next_retry_at_ms(now_ms, Some(12.5), 3, &settings);
        assert_eq!(next.parse::<u128>().ok(), Some(now_ms + 12_500));
    }

    #[test]
    fn auto_retry_schedule_uses_exponential_backoff_with_cap() {
        let settings = DesktopSettings {
            auto_retry_enabled: true,
            auto_retry_max_per_job: 3,
            auto_retry_max_per_pipeline: 3,
            auto_retry_base_delay_seconds: 10,
            auto_retry_max_delay_seconds: 25,
        };
        let now_ms = 2_000u128;

        let first = compute_next_retry_at_ms(now_ms, None, 1, &settings);
        assert_eq!(first.parse::<u128>().ok(), Some(now_ms + 10_000));

        let third = compute_next_retry_at_ms(now_ms, None, 3, &settings);
        assert_eq!(third.parse::<u128>().ok(), Some(now_ms + 25_000));
    }

    #[test]
    fn parse_retry_at_ms_handles_valid_and_invalid_values() {
        let valid = Some("12345".to_string());
        assert_eq!(parse_retry_at_ms(valid.as_ref()), Some(12_345));

        let invalid = Some("not-a-number".to_string());
        assert_eq!(parse_retry_at_ms(invalid.as_ref()), None);
        assert_eq!(parse_retry_at_ms(None), None);
    }

    #[test]
    fn diagnostics_bundle_generation_creates_report_and_summary_with_skips() {
        let base = std::env::temp_dir().join(format!("jarvis_diag_bundle_{}", now_epoch_ms()));
        let repo_root = base.join("repo");
        let pipeline_root = base.join("pipeline");
        let out_dir = base.join("out");
        let _ = fs::create_dir_all(repo_root.join("scripts"));
        let _ = fs::create_dir_all(&pipeline_root);
        let _ = fs::create_dir_all(pipeline_root.join("jarvis_core"));
        let _ = fs::create_dir_all(&out_dir);

        fs::write(repo_root.join("package.json"), r#"{"version":"0.0.1"}"#).expect("write package");
        fs::write(repo_root.join("smoke_tauri_e2e.ps1"), "# smoke").expect("write smoke");
        fs::write(
            repo_root.join("scripts").join("clean_machine_checklist.md"),
            "- npm run build\n- cargo test\n- smoke_tauri_e2e.ps1\n- scripts\\collect_diag.ps1\n",
        )
        .expect("write checklist");

        fs::write(pipeline_root.join("pyproject.toml"), "[tool.poetry]").expect("write pyproject");
        fs::write(pipeline_root.join("jarvis_cli.py"), "print('ok')").expect("write cli");

        let jobs_path = jobs_file_path(&out_dir);
        let pipelines_path = pipelines_file_path(&out_dir);
        save_jobs_to_file(
            &jobs_path,
            &[JobRecord {
                job_id: "job_1".to_string(),
                template_id: "TEMPLATE_TREE".to_string(),
                canonical_id: "arxiv:1706.03762".to_string(),
                params: serde_json::json!({}),
                status: JobStatus::NeedsRetry,
                attempt: 1,
                created_at: now_epoch_ms_string(),
                updated_at: now_epoch_ms_string(),
                run_id: Some("run_1".to_string()),
                last_error: Some("429".to_string()),
                retry_after_seconds: Some(3.0),
                retry_at: Some(now_epoch_ms_string()),
                auto_retry_attempt_count: 0,
            }],
        )
        .expect("save jobs");
        save_pipelines_to_file(
            &pipelines_path,
            &[PipelineRecord {
                pipeline_id: "pipe_1".to_string(),
                canonical_id: "arxiv:1706.03762".to_string(),
                name: "Analyze".to_string(),
                created_at: now_epoch_ms_string(),
                updated_at: now_epoch_ms_string(),
                steps: vec![],
                current_step_index: 0,
                status: PipelineStatus::NeedsRetry,
                last_primary_viz: None,
                auto_retry_attempt_count: 0,
            }],
        )
        .expect("save pipelines");

        save_settings(&out_dir, &DesktopSettings::default()).expect("save settings");
        let _ = fs::write(audit_jsonl_path(&out_dir), "{\"kind\":\"auto_retry\"}\n");

        let run_dir = out_dir.join("run_1");
        let _ = fs::create_dir_all(run_dir.join("paper_graph").join("tree"));
        fs::write(run_dir.join("input.json"), r#"{"desktop":{"canonical_id":"arxiv:1706.03762"}}"#)
            .expect("write input");
        fs::write(run_dir.join("result.json"), r#"{"status":"needs_retry"}"#)
            .expect("write result");
        fs::write(run_dir.join("paper_graph").join("tree").join("tree.md"), "# tree")
            .expect("write tree");
        fs::write(
            run_dir.join("stdout.log"),
            "X".repeat((DIAG_MAX_FILE_BYTES + 1024) as usize),
        )
        .expect("write huge stdout");

        let runtime = RuntimeConfig {
            config_file_path: repo_root.join("config.json"),
            config_file_loaded: false,
            pipeline_root,
            out_base_dir: out_dir.clone(),
            s2_api_key: None,
            s2_min_interval_ms: None,
            s2_max_retries: None,
            s2_backoff_base_sec: None,
        };

        let result = collect_diagnostics_internal(&repo_root, &runtime, DiagnosticsCollectOptions::default())
            .expect("collect diagnostics");
        let diag_dir = PathBuf::from(&result.diag_dir);
        assert!(diag_dir.exists());
        assert!(diag_dir.join("diag_report.md").exists());
        assert!(diag_dir.join("diag_summary.json").exists());

        let summary_raw = fs::read_to_string(diag_dir.join("diag_summary.json")).expect("read summary");
        let summary: DiagnosticSummary = serde_json::from_str(&summary_raw).expect("parse summary");
        assert!(!summary.jobs.is_empty());
        assert!(!summary.pipelines.is_empty());
        assert!(summary.files.iter().any(|f| !f.included && f.reason.as_deref() == Some("file_too_large")));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn schema_version_missing_defaults_to_v1_for_jobs() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_schema_missing_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let path = jobs_file_path(&out_dir);
        fs::write(
            &path,
            r#"{"jobs":[{"job_id":"job_1","template_id":"TEMPLATE_TREE","canonical_id":"arxiv:1","params":{},"status":"queued","attempt":0,"created_at":"1","updated_at":"1","run_id":null,"last_error":null,"retry_after_seconds":null,"retry_at":null}]}"#,
        )
        .expect("write legacy jobs");

        let rows = load_jobs_from_file(&path).expect("load legacy jobs");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].job_id, "job_1");

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn schema_version_higher_refuses_read_and_write() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_schema_high_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        let path = pipelines_file_path(&out_dir);
        fs::write(&path, r#"{"schema_version":99,"pipelines":[]}"#).expect("write high schema");

        let load_err = match load_pipelines_from_file(&path) {
            Ok(_) => panic!("must fail on high schema load"),
            Err(e) => e,
        };
        assert!(load_err.contains("unsupported schema_version"));

        let write_err = save_pipelines_to_file(&path, &[]).expect_err("must fail on high schema write");
        assert!(write_err.contains("refusing to modify"));

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn atomic_write_keeps_no_tmp_file_for_settings() {
        let out_dir = std::env::temp_dir().join(format!("jarvis_atomic_settings_{}", now_epoch_ms()));
        let _ = fs::create_dir_all(out_dir.join(".jarvis-desktop"));
        save_settings(&out_dir, &DesktopSettings::default()).expect("save settings");
        let path = settings_file_path(&out_dir);
        let tmp = path.with_extension("json.tmp");
        assert!(path.exists());
        assert!(!tmp.exists());

        let raw = fs::read_to_string(&path).expect("read settings");
        assert!(raw.contains("schema_version"));

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn run_summary_extraction_handles_missing_files() {
        let base = std::env::temp_dir().join(format!("jarvis_run_summary_{}", now_epoch_ms()));
        let run = base.join("run_1");
        let _ = fs::create_dir_all(&run);

        assert_eq!(parse_paper_id_from_input(&run.join("input.json")), "unknown");
        assert_eq!(parse_status_from_result(&run.join("result.json")), "unknown");

        fs::write(run.join("input.json"), r#"{"desktop":{"canonical_id":"doi:10.1/abc"}}"#)
            .expect("write input");
        fs::write(run.join("result.json"), r#"{"status":"succeeded"}"#)
            .expect("write result");

        assert_eq!(parse_paper_id_from_input(&run.join("input.json")), "doi:10.1/abc");
        assert_eq!(parse_status_from_result(&run.join("result.json")), "succeeded");

        let _ = fs::remove_dir_all(&base);
    }
}
