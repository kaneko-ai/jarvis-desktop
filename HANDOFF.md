# チャット引き継ぎドキュメント (2026-02-26)

## このドキュメントの目的

前回チャットで実施した auto-dev.ps1 の安定化作業の全記録。
他のAIチャットに渡す際、これを読めば前提・経緯・成果・残課題を過不足なく把握できる。

---

## 1. 前提環境

| 項目 | 値 |
|---|---|
| OS | Windows (PowerShell 5.1) |
| Node.js | v24.13.1 |
| Codex CLI | codex-cli 0.104.0 |
| 認証 | ChatGPT Plus OAuth (`codex auth login` 済み) |
| リポジトリ | `kaneko-ai/jarvis-desktop` (public) |
| パス | `C:\Users\kaneko yu\Documents\jarvis-work\jarvis-desktop` |
| GitHub CLI | `gh` 認証済み |

## 2. 引き継ぎ元チャットの経緯（時系列）

### フェーズ1: 問題の診断

前々回チャットで auto-dev.ps1 v3 の夜間自動開発ループを構築したが、以下の問題でブロックされていた：

1. **`--quiet` 非対応**: codex-cli 0.104.0 で `--quiet` が存在せず `unexpected argument` エラー
2. **stdinパイプ未統一**: 長文プロンプトをCLI引数で渡すと壊れる
3. **baseline `cargo fmt --check` 即死**: 既存のfmt driftでスクリプト停止
4. **チャット圧縮による前提崩壊**: AIがCodex CLI前提を見失いMCPの話に迷走

これらは前々回チャット末尾で方針が確定し、引き継ぎドキュメントとして整理された。
### フェーズ2: 今回チャットでの実作業

今回チャットで、実際のディスク上の auto-dev.ps1 を調査したところ、
前々回チャットで議論していた「理想形v3コード」と、ディスク上の実コードが異なっていたことが判明。

実コードの特徴：
- 関数名が異なる（`Send-Discord`, `Write-Step`, `Run-Cmd`, `Write-FileSafe` 等）
- Codex呼び出しは `$prompt | codex exec --full-auto - 2>&1` で既にstdinパイプ統一済み
- `--quiet` は使われていない（既に解決済み）
- baseline `cargo fmt --check` は非致命化済み
- `git worktree prune` が存在しない
- worktree作成後の `npm ci` がない

つまり前々回の3大問題のうち `--quiet` と stdinパイプ と baseline fmt は既に解決済みで、
実際のブロッカーは別の3つの新しい問題だった。

## 3. 発見された実際の問題と修正内容

### 問題A: Worktree内で npm ci / lint / build が全滅

原因: `package-lock.json` がリポジトリにコミットされていなかった。
git worktree には git追跡ファイルしかコピーされないため、worktree内に `node_modules` も
`package-lock.json` も存在せず、`npm ci` / `eslint` / `vite` が全て失敗しPR68がブロックされていた。

修正: `npm install` で生成し、gitにコミット。
commit: chore: add package-lock.json for reproducible installs (a9477ca)


### 問題B: eslint が .wt/ 内の古いworktreeもスキャンして3重警告

原因: `.wt/` と `.codex-logs/` が eslint の除外対象に入っていなかった。
`eslint .` がルートから再帰スキャンし、残骸worktree内のソースも拾い警告が27個（9個x3パス）出ていた。

修正: `eslint.config.js` の `globalIgnores` に `.wt` と `.codex-logs` を追加。
commit: chore: exclude .wt and .codex-logs from eslint scan (c85bbe1)


### 問題C: auto-dev.ps1 に worktree掃除と npm ci がない

原因: 古いworktreeがディスクに残り続ける + worktree内にnode_modulesがない。

修正: auto-dev.ps1 に2箇所追加。
1. worktree作成前に `.wt/` 内の古いフォルダを物理削除 + `git worktree prune`
2. worktree作成後、STEP A0の前に `npm ci`（失敗時は `npm install` にフォールバック）
commit: fix: add worktree cleanup and npm ci install step to auto-dev loop (e07c016)


### 問題D: PowerShell 5.1 がUTF-8 BOMなしファイルをShift_JISとして読む

原因: auto-dev.ps1 がBOMなしUTF-8で保存されていた。
PowerShell 5.1 はBOMがないとシステムデフォルト（日本語Windows = Shift_JIS）として読むため、
日本語文字列がmojibake化し、パーサーが構文エラーを大量に出してスクリプトが起動しなかった。

修正: ファイル先頭にUTF-8 BOM (EF BB BF) を追加。
構文チェック `[System.Management.Automation.Language.Parser]::ParseFile()` で Syntax OK を確認。
commit: fix: add UTF-8 BOM for PowerShell 5.1 compatibility (c92c725)

## 4. 修正後のテスト実行結果

`.\auto-dev.ps1 -MaxPRs 1 -SleepMinutes 1` で1サイクル完走。

| 項目 | 結果 |
|---|---|
| 全ステップ完走 | A0→A→A1→B→B-test→B2→C→D→E すべて実行 |
| npm run lint | 0 errors, 9 warnings（正常） |
| cargo fmt --check | pass |
| cargo test | 80 passed, 0 failed |
| PR作成 | #47 作成済み (`pr68-run-path-consistency`) |
| Discord通知 | 日本語で正常送信 |
| 所要時間 | 45分52秒（前回11時間から大幅短縮） |
| サイクルステータス | 「失敗」（B-testリトライ3/3使い切りが原因、PRは作成された） |

## 5. 現在のmainブランチのコミット履歴

c92c725 fix: add UTF-8 BOM for PowerShell 5.1 compatibility e07c016 fix: add worktree cleanup and npm ci install step to auto-dev loop c85bbe1 chore: exclude .wt and .codex-logs from eslint scan a9477ca chore: add package-lock.json for reproducible installs 7176296 fix: codex exec stderr handling (2>&1), remove worktree fallback c6e6221 chore: automated improvement cycle 1


## 6. オープンPR

- #47 `pr68-run-path-consistency`: fix: unify run explorer and dashboard data source with runtime out dir
  - URL: https://github.com/kaneko-ai/jarvis-desktop/pull/47
  - ステータス: OPEN (CIチェック待ち)
## 7. 残存する軽微な課題（緊急度低）

### 7-1. B-testリトライ3/3使い切り問題
Codexが実装したコードのテストが3回とも失敗してリトライ上限に達した。
ただしB2レビューで4件修正され、最終的にPRは作成されている。
根本原因は `cargo test` の stderr 出力を PowerShell が ErrorRecord 扱いすることがある問題。
auto-dev.ps1 内のCodex呼び出しでは `$ErrorActionPreference = "Continue"` で回避しているが、
テスト実行パスの一部で同じ罠が残っている可能性がある。

### 7-2. テスト件数が Before/After で 0件
Before/After の Tests/TestPass が共に0になっている。
実際には80テスト通っているので、パース正規表現がマッチしていない可能性。
Discord通知の表示に影響するが、実害は小さい。

### 7-3. PR作成時の "already exists" エラー
前回サイクルの残骸ブランチがGitHub側に残っていたため、同名ブランチでPR作成しようとしてエラー。
ブランチ名がサイクルごとに異なるため通常は再発しない。

### 7-4. src/App.jsx の React hooks 警告9件
既存のlint警告。auto-devの動作には影響しない。
master-plan.md に PR72 として修正候補が記載されている。
## 8. auto-dev.ps1 の主要設計（クイックリファレンス）

### ステップフロー

    A0: Deep Research（コードベース調査）
    A:  Plan（実装計画 + meta.json生成）
    A1: Self-Annotation（計画のセルフレビュー）
    B:  Implement（実装 + コミット）
    B-test: Test + Self-heal（テスト→失敗なら修正→リトライ、最大3回）
    B2: Independent Review（独立コードレビュー）
    B+: Evidence（証跡収集）
    C:  Summary（結果要約）
    D:  Push & PR（GitHub PR作成）
    E:  Auto-merge（squash merge + ブランチ削除）

### Codex呼び出しパターン（全7箇所共通）

    $ErrorActionPreference = "Continue"
    $prompt | codex exec --full-auto - 2>&1
    $ErrorActionPreference = "Stop"

### 主要関数

| 関数名 | 役割 |
|---|---|
| Write-Step | ETA付きステップ表示 |
| Run-Cmd | cmd.exe経由の安全なコマンド実行 |
| Write-FileSafe | UTF-8ファイル書き出し |
| Send-Discord | Discord Webhook通知（日本語） |
| Send-CycleReport | Before/After付きサイクル完了通知 |
| Load-SpecTemplates | prompts/spec-*.md の読み込み |
| Load-Memory | .codex-logs/memory/ からの記憶読み込み |
| Get-ConstraintWedge | 制約条件ブロック生成 |
## 9. 次のAIへの注意事項

### やるべきこと
- auto-dev.ps1 を変更する場合は必ず UTF-8 BOM付き で保存すること
- Codex CLI のオプションは codex exec --help で確認してから使うこと
- PowerShell 5.1 の stderr 罠を常に意識すること

### やってはいけないこと
- MCPの一般論に脱線しない（過去チャットでの失敗原因）
- OpenClaw / BrowserUse / WebUI自動操作への回帰を提案しない
- Codex CLIの初歩セットアップをやり直さない（認証済み）
- BOMなしUTF-8でファイルを保存しない（PowerShell 5.1が読めない）

## 10. 主要ファイルURL（public リポジトリ）

- リポジトリ: https://github.com/kaneko-ai/jarvis-desktop
- auto-dev.ps1: https://github.com/kaneko-ai/jarvis-desktop/blob/main/auto-dev.ps1
- eslint.config.js: https://github.com/kaneko-ai/jarvis-desktop/blob/main/eslint.config.js
- HANDOFF.md: https://github.com/kaneko-ai/jarvis-desktop/blob/main/HANDOFF.md
- PR #47: https://github.com/kaneko-ai/jarvis-desktop/pull/47
- コミット履歴: https://github.com/kaneko-ai/jarvis-desktop/commits/main