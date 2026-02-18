# Clean Machine Checklist (jarvis-desktop)

目的: release artifact を使って、別PC/別ユーザーのクリーン環境で One True Path を再現する。

## 0. 事前準備
- [ ] インストーラ（MSI/EXE）を取得
- [ ] `jarvis-ml-pipeline` の配置先を決定（例: `C:\Users\<user>\Documents\jarvis-work\jarvis-ml-pipeline`）
- [ ] Python 3.12+ をインストール
- [ ] Pipeline 側で `jarvis_cli.py` が実行可能であることを確認

## 1. インストールと初回起動
- [ ] `dist/releases/<version>/installers/` の `*.msi` または `*-setup.exe` を実行
- [ ] Start Menu から `jarvis-desktop` を起動（ターミナル不要）
- [ ] Setup 画面で `デスクトップにショートカットを作成` を押す
- [ ] Desktop に `jarvis-desktop.lnk` が作成されることを確認
- [ ] `jarvis-desktop.lnk` をダブルクリックして起動できることを確認
- [ ] いったんアプリを終了し、`jarvis-desktop.lnk` から再起動できることを確認
- [ ] Start Menu からも再起動できることを確認
- [ ] （任意）タスクバーへピン留め（手動操作）
- [ ] UI で `Open config file location` を押す
- [ ] `config.json` が無ければ `Create config template` を押す（または自動生成を確認）

## 2. config.json 作成/編集
- [ ] config ファイル場所: `%APPDATA%\jarvis-desktop\config.json`
- [ ] 最低限のキーを設定:

```json
{
  "JARVIS_PIPELINE_ROOT": "C:\\Users\\<user>\\Documents\\jarvis-work\\jarvis-ml-pipeline",
  "JARVIS_PIPELINE_OUT_DIR": "logs/runs",
  "S2_API_KEY": "",
  "S2_MIN_INTERVAL_MS": 1000,
  "S2_MAX_RETRIES": 6,
  "S2_BACKOFF_BASE_SEC": 0.5
}
```

- [ ] UI の `Reload config` を実行
- [ ] `Config validation: ok` を確認

## 3. One True Path 実行確認
- [ ] `paper_id=arxiv:1706.03762` / `depth=1` / `max=5` で `Run papers tree`
- [ ] 完了後、`status` が `ok` または `needs_retry` であることを確認
- [ ] `Open run folder` が開くことを確認
- [ ] `logs/runs/<run_id>/input.json` を確認
- [ ] `logs/runs/<run_id>/result.json` を確認
- [ ] `logs/runs/<run_id>/paper_graph/tree/tree.md` を確認

## 4. 失敗時の分岐

### A) pipeline root 不正
- 症状: `missing_dependency` / `pipeline root is invalid`
- 対応:
  - [ ] `JARVIS_PIPELINE_ROOT` を実在パスへ修正
  - [ ] `jarvis_cli.py` と `jarvis_core/` 存在を確認
  - [ ] `Reload config` 再実行

### B) python 無し / 実行不可
- 症状: `python preflight failed`
- 対応:
  - [ ] `python --version` を確認
  - [ ] pipeline 側 `.venv` 作成
  - [ ] 再実行

### C) S2 API キー未設定
- 症状: 成功するが取得件数/安定性が低い、または429が増える
- 対応:
  - [ ] 必要に応じて `S2_API_KEY` を設定
  - [ ] `Reload config`

### D) 429 / transient failure
- 症状: `status=needs_retry`、`retry-after` 表示
- 対応:
  - [ ] 表示された待機時間後に `Retry (new run_id)`
  - [ ] 連続失敗時は `S2_MAX_RETRIES`, `S2_BACKOFF_BASE_SEC`, `S2_MIN_INTERVAL_MS` を調整

### E) 権限問題（out_dir 書込不可）
- 症状: `out_dir is not writable`
- 対応:
  - [ ] `JARVIS_PIPELINE_OUT_DIR` を書込可能な場所に変更
  - [ ] フォルダ権限（ACL）確認
  - [ ] `Reload config` 再実行

### F) Open run folder セキュリティ拒否
- 症状: `RULE_RUN_DIR_OUTSIDE_ALLOWED_ROOTS` など
- 対応:
  - [ ] run_dir が allowed roots 配下か確認
  - [ ] UNC/`\\?\`/`\\.\` prefix を使わない

## 5. 証跡採取
- [ ] `scripts\collect_diag.ps1` を実行
- [ ] `diag_report.md` を保存
- [ ] 実行ログ（build/smoke/アプリ画面）と併せて添付
