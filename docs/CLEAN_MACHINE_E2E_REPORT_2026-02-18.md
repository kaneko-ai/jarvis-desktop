# CLEAN MACHINE E2E Report (2026-02-18)

## 1. 実行情報
- 実行日時: 2026-02-18
- OS: Windows
- ユーザー: kaneko yu
- ブランチ / コミット: pr18-clean-machine-e2e-audit / (local HEAD)

## 2. 実行コマンド
- `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\reset_app_state_win.ps1 -Confirm`
- `npm run build`
- `cargo test -q --manifest-path src-tauri/Cargo.toml`
- `npm run release:win`
- `powershell -ExecutionPolicy Bypass -File .\smoke_tauri_e2e.ps1 -PipelineRoot "C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline"`
- `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\collect_diag.ps1 -PipelineRoot "C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline" -OutPath .\diag_report.md`

## 3. 生成物（相対パス）
- `dist/releases/0.1.0/installers/jarvis-desktop_0.1.0_x64_en-US.msi`
- `dist/releases/0.1.0/installers/jarvis-desktop_0.1.0_x64-setup.exe`
- `dist/releases/0.1.0/release_manifest.json`
- `dist/releases/0.1.0/SHA256SUMS.txt`

## 4. ゲート結果
- `npm run build`: OK
- `cargo test -q --manifest-path src-tauri/Cargo.toml`: OK (55 passed)
- `smoke_tauri_e2e.ps1`: OK
  - run_dir: `C:\Users\kaneko yu\Documents\jarvis-work\jarvis-ml-pipeline\logs\runs\20260218_143527_f081cc8e`
- `release:win`: OK

## 5. 初回起動（クリック起動）
- Setup表示: 未実施（GUI手動検証が必要）
- 証跡: 未取得

## 6. Pipeline Repo（PR17導線）
- Clone/Bootstrap: 未実施（GUI手動検証が必要）
- Update: 未実施（GUI手動検証が必要）
- Validate: 未実施（GUI手動検証が必要）
- Open: 未実施（GUI手動検証が必要）

## 7. One-click pipeline
- Run作成: OK（smokeでCLI run artifact生成を確認）
- Jobs/Artifacts表示: 未実施（GUI手動検証が必要）
- viewer（HTML/graph）: 未実施（GUI手動検証が必要）

## 8. Diagnostics / Workspace
- Diagnostics report生成: OK
  - `diag_report.md`
- Diagnostics zip生成: 未実施（GUI手動検証が必要）
- Workspace export/import: 未実施（GUI手動検証が必要）

## 9. 起動導線
- デスクトップショートカット起動: 未実施
- スタートメニュー起動: 未実施

## 10. 失敗/課題メモ
- 初回 `npm run release:win` は `npm ci` lock不整合で失敗（`npm install` 後に再実行で解消）。
- GUIベースの監査項目（Setup/Ops導線、Workspace import/export、クリック起動）は本レポートでは未実施。
