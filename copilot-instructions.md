# Copilot Instructions（jarvis-desktop）

## 1) 目的（非交渉）
- desktop の One True Path は `RUNBOOK.md` と preflight/build/smoke scripts とする。
- Tauri + Rust + Node + Python 連携では、機能追加より「再現性と安全性」を優先する。

## 2) 実行経路（固定）
- 開発・検証は `RUNBOOK` 記載の順序を厳守する。
- 変更後は必ず次を実行する。
  1. `preflight_desktop.ps1`
  2. `build_desktop.ps1`（必要時のみ `-SkipNpmCi`）
  3. `smoke_tauri_e2e.ps1`
- Release 変更が絡む場合は `release_windows.ps1` と workflow を破壊しない。

## 3) セキュリティ制約（絶対）
- `open_run_folder` の allowed roots + canonicalize + prefix拒否（`UNC` / `\\?\` / `\\.\`）を維持する。
- run_dir を任意パスで開ける改修を禁止する。
- 外部入力（config/env/UI）由来のパスは必ず検証する。

## 4) 設定解決の契約
- config の優先順位 `file > env > autodetect` を維持する。
- UI の runtime config 表示 / Reload / Open location の挙動を壊さない。

## 5) 429 UX の契約
- `429` または retry 枯渇時は `status=needs_retry` を維持し、`Retry(new run_id)` を壊さない。
- ユーザーへ `retry-after` 等の再試行情報を明示する。

## 6) 変更時の必須成果物
- run 生成時に `logs/runs/<run_id>` へ `input.json` / `result.json` / `tree.md` が出力される契約を維持する。
- 上記成果物を smoke で検証する運用を維持する。
