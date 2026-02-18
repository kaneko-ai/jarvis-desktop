# Local Release Builder (Windows)

## 目的
`jarvis-desktop` をローカルで再現可能にビルドし、別PCでダブルクリック起動できる成果物（インストーラー / 実行ファイル）を作成します。

## 前提条件
- Windows
- Node.js / npm
- Rust (cargo / rustc)
- WebView2 Runtime（Windows 11 は通常同梱。未導入環境は Microsoft の公式配布で導入）
- `jarvis-ml-pipeline` がローカルに存在し、`smoke_tauri_e2e.ps1` が通ること

## 実行コマンド
プロジェクトルートで実行:

```powershell
npm run release:win
```

必要に応じて PipelineRoot を明示:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\release_win.ps1 -PipelineRoot "C:\path\to\jarvis-ml-pipeline"
```

## スクリプトが実行する内容
1. 依存解決 (`npm ci` / lock がない場合は `npm install`)
2. フロントビルド (`npm run build`)
3. Rust テスト (`cargo test -q` in `src-tauri`)
4. スモーク (`smoke_tauri_e2e.ps1 -RunDiagStrict`)
5. Tauri リリースビルド (`npx tauri build`)
6. 成果物収集 + `SHA256SUMS.txt` + `release_manifest.json` 生成

## 出力先
すべて `dist/releases/<app_version>/` 配下に保存されます。

- `installers/` : MSI / NSIS EXE などのインストーラー
- `bin/` : アプリ本体 EXE（存在する場合）
- `release_manifest.json` : バージョン、コミット、成果物一覧、SHA256
- `SHA256SUMS.txt` : 監査用ハッシュ一覧

## 別PCでの起動
- 推奨: `installers/` 内の MSI または EXE インストーラーを実行
- 代替: `bin/` の EXE を直接起動（配布形態によっては依存不足の可能性あり）

## SmartScreen にブロックされた場合
未署名ローカルビルドでは SmartScreen 警告が出る場合があります。組織ポリシーに従って確認し、信頼できるビルドのみを実行してください。
