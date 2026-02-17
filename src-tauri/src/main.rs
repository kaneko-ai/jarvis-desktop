#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io::Write};

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
    paper_id: String,
    run_dir: String,
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

fn normalize_paper_id(raw: &str) -> Result<String, String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return Err("paper_id is empty".to_string());
    }

    s = s.trim_matches('"').trim_matches('\'').trim().to_string();
    s = s.replace('\u{3000}', " ");
    s = s.trim().to_string();
    if s.is_empty() {
        return Err("paper_id is empty after trim".to_string());
    }

    let lower = s.to_lowercase();

    if lower.contains("arxiv.org/abs/") {
        if let Some(idx) = lower.find("arxiv.org/abs/") {
            let tail = &s[(idx + "arxiv.org/abs/".len())..];
            let id = tail.split(&['?', '#'][..]).next().unwrap_or("").trim();
            let id = id.trim_end_matches('/');
            if id.is_empty() {
                return Err("failed to parse arxiv id from url".to_string());
            }
            return Ok(format!("arxiv:{id}"));
        }
    }
    if lower.contains("arxiv.org/pdf/") {
        if let Some(idx) = lower.find("arxiv.org/pdf/") {
            let tail = &s[(idx + "arxiv.org/pdf/".len())..];
            let id = tail
                .split(&['?', '#'][..])
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches(".pdf")
                .trim_end_matches('/');
            if id.is_empty() {
                return Err("failed to parse arxiv id from pdf url".to_string());
            }
            return Ok(format!("arxiv:{id}"));
        }
    }

    if lower.contains("doi.org/") {
        if let Some(idx) = lower.find("doi.org/") {
            let tail = &s[(idx + "doi.org/".len())..];
            let doi = tail.split(&['?', '#'][..]).next().unwrap_or("").trim();
            let doi = doi.trim_end_matches('/');
            if doi.is_empty() {
                return Err("failed to parse doi from doi.org url".to_string());
            }
            return Ok(format!("doi:{doi}"));
        }
    }
    if lower.contains("dx.doi.org/") {
        if let Some(idx) = lower.find("dx.doi.org/") {
            let tail = &s[(idx + "dx.doi.org/".len())..];
            let doi = tail.split(&['?', '#'][..]).next().unwrap_or("").trim();
            let doi = doi.trim_end_matches('/');
            if doi.is_empty() {
                return Err("failed to parse doi from dx.doi.org url".to_string());
            }
            return Ok(format!("doi:{doi}"));
        }
    }

    if lower.contains("semanticscholar.org/paper/") {
        let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
        if let Some(last) = parts.last() {
            let cand = last.split(&['?', '#'][..]).next().unwrap_or("").trim();
            if !cand.is_empty() {
                return Ok(format!("s2:{cand}"));
            }
        }
        return Err("failed to parse semantic scholar paper id from url".to_string());
    }

    for p in ["arxiv:", "doi:", "pmid:", "s2:"] {
        if lower.starts_with(p) {
            let body = s[p.len()..].trim().replace(' ', "");
            if body.is_empty() {
                return Err(format!("paper_id has prefix {p} but empty body"));
            }
            return Ok(format!("{p}{body}"));
        }
    }

    if s.starts_with("10.") && s.contains('/') {
        let doi = s.replace(' ', "");
        if doi.is_empty() {
            return Err("doi inference failed".to_string());
        }
        return Ok(format!("doi:{doi}"));
    }

    if !s.contains('.') && s.chars().all(|c| c.is_ascii_digit()) {
        return Ok(format!("pmid:{s}"));
    }

    if s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.contains('.') {
        return Ok(format!("arxiv:{s}"));
    }

    Ok(s)
}

fn make_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{}", now.as_secs(), now.subsec_nanos())
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
    if trimmed.contains(['\\', '/']) {
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
fn list_runs() -> Result<Vec<RunListItem>, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;

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

    entries.sort_by(|a, b| b.1.cmp(&a.1));

    let mut rows = Vec::with_capacity(entries.len());
    for (run_dir, ts) in entries {
        let run_id = run_dir
            .file_name()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let status = parse_status_from_result(&run_dir.join("result.json"));
        let paper_id = parse_paper_id_from_input(&run_dir.join("input.json"));
        rows.push(RunListItem {
            run_id,
            status,
            created_at_epoch_ms: ts,
            paper_id,
            run_dir: run_dir.to_string_lossy().to_string(),
        });
    }

    Ok(rows)
}

#[tauri::command]
fn read_run_artifact(run_id: String, artifact: String) -> Result<RunArtifactView, String> {
    let root = repo_root();
    let runtime = resolve_runtime_config(&root)?;
    let run_id = validate_run_id_component(&run_id)?;
    let run_dir = resolve_run_dir_from_id(&runtime, &run_id)?;

    let (artifact_key, rel_path, is_json) = match artifact.as_str() {
        "tree_md" => ("tree_md", PathBuf::from("paper_graph").join("tree").join("tree.md"), false),
        "result_json" => ("result_json", PathBuf::from("result.json"), true),
        "input_json" => ("input_json", PathBuf::from("input.json"), true),
        "stdout_log" => ("stdout_log", PathBuf::from("stdout.log"), false),
        "stderr_log" => ("stderr_log", PathBuf::from("stderr.log"), false),
        _ => return Err(format!("unsupported artifact: {artifact}")),
    };

    let target = run_dir.join(rel_path);
    if !target.exists() {
        return Ok(RunArtifactView {
            run_id,
            artifact: artifact_key.to_string(),
            path: target.to_string_lossy().to_string(),
            exists: false,
            content: "missing".to_string(),
            parse_status: "missing".to_string(),
        });
    }

    let raw = fs::read_to_string(&target)
        .map_err(|e| format!("failed to read artifact {}: {e}", target.display()))?;

    if is_json {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => {
                let pretty = serde_json::to_string_pretty(&v)
                    .map_err(|e| format!("failed to pretty print json {}: {e}", target.display()))?;
                Ok(RunArtifactView {
                    run_id,
                    artifact: artifact_key.to_string(),
                    path: target.to_string_lossy().to_string(),
                    exists: true,
                    content: pretty,
                    parse_status: "ok".to_string(),
                })
            }
            Err(_) => Ok(RunArtifactView {
                run_id,
                artifact: artifact_key.to_string(),
                path: target.to_string_lossy().to_string(),
                exists: true,
                content: raw,
                parse_status: "raw".to_string(),
            }),
        }
    } else {
        Ok(RunArtifactView {
            run_id,
            artifact: artifact_key.to_string(),
            path: target.to_string_lossy().to_string(),
            exists: true,
            content: raw,
            parse_status: "ok".to_string(),
        })
    }
}

#[tauri::command]
fn run_papers_tree(paper_id: String, depth: u8, max_per_level: u32) -> RunResult {
    let run_id = make_run_id();

    let normalized = match normalize_paper_id(&paper_id) {
        Ok(v) => v,
        Err(e) => {
            return RunResult {
                ok: false,
                exit_code: 1,
                stdout: "".to_string(),
                stderr: format!("paper_id normalize error: {e}"),
                run_id,
                run_dir: "".to_string(),
                status: "error".to_string(),
                message: format!("paper_id normalize error: {e}"),
                retry_after_sec: None,
            }
        }
    };

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
    cmd.current_dir(&pipeline_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg(cli_script.as_os_str())
        .args([
            "papers",
            "tree",
            "--id",
            &normalized,
            "--depth",
            &depth.to_string(),
            "--max-per-level",
            &max_per_level.to_string(),
            "--out",
            out_base_dir.to_string_lossy().as_ref(),
            "--out-run",
            &run_id,
        ]);

    let out = match cmd.output() {
        Ok(o) => o,
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
    let status = read_status(&stdout, &stderr, code);
    let retry_after_sec = extract_retry_after_seconds(&format!("{stdout}\n{stderr}"));
    let message = build_status_message(&status, &stdout, &stderr, retry_after_sec);

    RunResult {
        ok: out.status.success(),
        exit_code: code,
        stdout: stdout.clone(),
        stderr: stderr.clone(),
        run_id,
        run_dir: run_dir_abs.to_string_lossy().to_string(),
        status,
        message,
        retry_after_sec,
    }
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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            run_papers_tree,
            open_run_folder,
            list_runs,
            read_run_artifact,
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
}
