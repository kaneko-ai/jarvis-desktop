# Clean Machine E2E Runbook (One True Path)

この手順は分岐禁止です。必ず上から順に実行してください。

## 0. 前提
- リポジトリ: `jarvis-desktop`
- ブランチ: `pr18-clean-machine-e2e-audit`
- Pipeline root 例: `C:\Users\<user>\Documents\jarvis-work\jarvis-ml-pipeline`

## 1. 状態リセット
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\reset_app_state_win.ps1 -Confirm
```

## 2. ローカルゲート
```powershell
npm run build
cargo test -q --manifest-path src-tauri/Cargo.toml
powershell -ExecutionPolicy Bypass -File .\smoke_tauri_e2e.ps1 -PipelineRoot "C:\Users\<user>\Documents\jarvis-work\jarvis-ml-pipeline"
```

## 3. リリース生成
```powershell
npm run release:win
```

## 4. 生成物確認
- `dist/releases/<ver>/installers/*.msi`
- `dist/releases/<ver>/installers/*-setup.exe`
- `dist/releases/<ver>/release_manifest.json`
- `dist/releases/<ver>/SHA256SUMS.txt`

## 5. インストールとクリック起動
1. `*-setup.exe` を実行
2. スタートメニューまたはデスクトップショートカットで起動（ターミナル起動は禁止）

## 6. Setup Wizard（Pipeline Engine）
1. `remote_url / local_path / ref` を確認
2. `Save settings`
3. `Clone / Setup`
4. `Update`
5. `Validate`
6. `Open folder`

## 7. One-click pipeline
1. Mainで run を作成
2. Runs / Jobs / Artifacts を確認
3. viewer（HTML/graph）を確認

## 8. Diagnostics / Workspace
1. Opsで `Collect Diagnostics`
2. Diagnostics zip を `Open zip` で確認
3. Workspace `Export`
4. Workspace `Import` を `dry-run -> merge` の順で確認

## 9. 証跡記録
- `docs/CLEAN_MACHINE_E2E_REPORT_TEMPLATE.md` を元に
  `docs/CLEAN_MACHINE_E2E_REPORT_YYYY-MM-DD.md` を作成して記録する。
