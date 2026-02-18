# CLEAN MACHINE E2E Report Template

## 1. 実行情報
- 実行日時:
- OS:
- ユーザー:
- ブランチ / コミット:

## 2. 実行コマンド
- リセット: `powershell -NoProfile -ExecutionPolicy Bypass -File .\\scripts\\reset_app_state_win.ps1 -Confirm`
- Build: `npm run build`
- Rust test: `cargo test -q --manifest-path src-tauri/Cargo.toml`
- Release: `npm run release:win`
- Smoke: `powershell -ExecutionPolicy Bypass -File .\\smoke_tauri_e2e.ps1 -PipelineRoot "<pipeline_root>"`

## 3. 生成物（相対パス）
- MSI:
- Setup EXE:
- release_manifest.json:
- SHA256SUMS.txt:

## 4. 初回起動（クリック起動）
- Setup表示: OK / NG
- 証跡（スクショ or diag bundle）:

## 5. Pipeline Repo（PR17導線）
- Clone/Bootstrap: OK / NG（理由）
- Update: OK / NG（理由）
- Validate: OK / NG（理由）
- Open: OK / NG（理由）

## 6. One-click pipeline
- Run作成: OK / NG
- Jobs/Artifacts表示: OK / NG
- viewer（HTML/graph）: OK / NG

## 7. Diagnostics / Workspace
- Diagnostics zip生成: OK / NG
- zip_path:
- Open zip: OK / NG
- Workspace export zip: OK / NG
- Workspace import: dry-run / merge / replace（結果）

## 8. 起動導線
- デスクトップショートカット起動: OK / NG
- スタートメニュー起動: OK / NG

## 9. 失敗時の記録
- 失敗理由:
- 再現手順:
- diag zip path:
- workspace zip path:
