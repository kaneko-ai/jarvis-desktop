# JARVIS Desktop RUNBOOK

## One True Path (first-time setup)

```powershell
# 1) Python pipeline setup
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install --upgrade pip
.\.venv\Scripts\python.exe -m pip install -e .

# 2) Desktop setup
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
npm ci

# 3) Required env
$env:JARVIS_PIPELINE_ROOT="C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline"
$env:JARVIS_PIPELINE_OUT_DIR="C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline\logs\runs"
```

## Prerequisite check

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
.\preflight_desktop.ps1
```

Checks:
- `node`, `npm`, `python`, `cargo`, `rustc`
- desktop root shape (`package.json`, `src-tauri`)
- pipeline root validity (`pyproject.toml`, `jarvis_cli.py`, `jarvis_core`)
- `JARVIS_PIPELINE_ROOT`, `JARVIS_PIPELINE_OUT_DIR`
- `S2_API_KEY` presence (optional warning)

## Deterministic desktop build

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
.\build_desktop.ps1
```

This runs:
1. `preflight_desktop.ps1`
2. `npm ci`
3. `npm run build`
4. `cargo build --manifest-path src-tauri/Cargo.toml`

## Windows release packaging

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
npm run release:windows
```

This runs:
1. `npm ci`
2. `npm run build`
3. `cargo tauri build`

Release artifacts are generated under:
- `src-tauri/target/release/bundle/msi/*.msi`
- `src-tauri/target/release/bundle/nsis/*.exe` (if NSIS target is enabled)

GitHub Actions (manual):
- workflow: `.github/workflows/release-windows.yml`
- trigger: `workflow_dispatch`
- uploaded artifact: `tauri-windows-bundle`

### Clean-machine verification (release)
1. Copy installer from `src-tauri/target/release/bundle/...` to a machine without source code.
2. Install and launch app.
3. In app, open config location and set `JARVIS_PIPELINE_ROOT` in `%APPDATA%\jarvis-desktop\config.json`.
4. Run `Run papers tree` with `arxiv:1706.03762`, depth=1, max=5.
5. Confirm `logs/runs/<run_id>/input.json`, `result.json`, `paper_graph/tree/tree.md`.

## Config UX (file + env + auto-detect)

Desktop resolves config in this order:
1. `%APPDATA%\jarvis-desktop\config.json`
2. environment variables
3. auto-detect (pipeline root only)

Supported keys in config file:
- `JARVIS_PIPELINE_ROOT`
- `JARVIS_PIPELINE_OUT_DIR`
- `S2_API_KEY`
- `S2_MIN_INTERVAL_MS`
- `S2_MAX_RETRIES`
- `S2_BACKOFF_BASE_SEC`

UI panel provides:
- resolved `pipeline_root` and `out_dir`
- `Open config file location` button
- `Reload config` button

## Run desktop dev

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
npx tauri dev
```

## Run pipeline standalone

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline
.\.venv\Scripts\python.exe jarvis_cli.py papers tree --id arxiv:1706.03762 --depth 2 --max-per-level 50 --out logs/runs --out-run auto
```

Expected artifacts under `logs/runs/<run_id>/`:
- `input.json`
- `result.json`
- `report.md`
- `paper_graph/tree/tree.md`

## Smoke (desktop + pipeline)

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop\jarvis-desktop
.\smoke_tauri_e2e.ps1
```

What it verifies:
- Tauri process starts
- pipeline CLI can generate a new run directory
- required files exist (`input.json`, `result.json`, `paper_graph/tree/tree.md`)

## Clean-machine verification checklist

- Checklist: `scripts/clean_machine_checklist.md`
- Goal: installer artifact ベースで、別PC/別ユーザーでも同じ One True Path を再現する。

## Diagnostics

Generate machine + config + latest run artifact diagnostics:

```powershell
cd C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop-repo
powershell -ExecutionPolicy Bypass -File .\scripts\collect_diag.ps1 -PipelineRoot "C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline"
```

Output:
- `diag_report.md` (desktop root)
- Includes OS/tool versions, sanitized config, latest run dir, and artifact existence checks.

## Security: open_run_folder restrictions

`open_run_folder` accepts only canonical directories under:
- `<desktop_root>/logs/runs`
- `<pipeline_root>/logs/runs`
- `JARVIS_PIPELINE_OUT_DIR` (if set)

Rejected:
- empty path
- non-existing path
- file path (must be directory)
- UNC / device prefixes (`\\`, `\\?\`, `\\.\`)
- paths outside allowed roots after canonicalization

## Troubleshooting

### 429 / transient API failures
- Set `S2_API_KEY` if available.
- Pipeline retry knobs:
  - `S2_MAX_RETRIES`
  - `S2_BACKOFF_BASE_SEC`
  - `S2_MIN_INTERVAL_MS`
- UI shows `rate-limited; retry after X sec` when available.
- `Retry (new run_id)` reruns the same request with a newly generated run id.

### Missing dependency
- If `missing_dependency` appears in UI:
  - verify `JARVIS_PIPELINE_ROOT`
  - verify `<pipeline_root>\jarvis_cli.py` exists
  - verify python venv under `src-tauri/.venv` or `<pipeline_root>/.venv`
  - run `.\preflight_desktop.ps1`

### Rust/Cargo not found

```powershell
winget install Rustlang.Rustup
cargo -V
rustc -V
```
