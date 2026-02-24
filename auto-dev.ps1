<#
.SYNOPSIS
    auto-dev v3 – Codex 自動開発ループ（worktree分離・日本語Discord通知・ETA表示）
.DESCRIPTION
    A0(リサーチ)→A(計画)→A1(セルフアノテーション)→B(実装+自己修復)→B2(レビュー)→
    B+(証跡)→C(要約)→D(Push/PR)→E(マージ) を繰り返す自動開発スクリプト。
    Discord通知は日本語 Before/After 形式。ターミナルにリアルタイムETA表示。
#>
param(
    [int]    $MaxPRs           = 5,
    [int]    $SleepMinutes     = 15,
    [string] $PlanFile         = "master-plan.md",
    [switch] $AllowDirtyStart,
    [switch] $AutoPush         = $true,
    [switch] $AutoCreatePR     = $true,
    [switch] $DraftPR          = $true,
    [string] $DiscordWebhookUrl = "https://discord.com/api/webhooks/1475856515066892288/rj96GVnnXvyULPXj20pppIn7QXtAUkAbNIAa1KhwUHoSbbJYX3kTuRMnprT63_Qky32u",
    [int]    $MaxRetries       = 3,
    [int]    $MaxDiffLines     = 400,
    [int]    $MaxFilesChanged  = 12
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── タイムスタンプ ──
$RunId     = Get-Date -Format "yyyyMMdd-HHmmss"
$StartTime = Get-Date

# ── ステップ別推定所要時間（秒） ── ETA計算用
$StepWeights = [ordered]@{
    "A0" = 420;  "A" = 180;  "A1" = 300
    "B"  = 900;  "B2" = 600; "B+" = 60
    "C"  = 120;  "D"  = 30;  "E"  = 15
}
$TotalEstSec   = ($StepWeights.Values | Measure-Object -Sum).Sum
$CycleStartTime = $null

# ── ETA付きステップ表示 ──
function Write-Step {
    param([string]$Step, [string]$Message, [int]$Cycle, [int]$Total)
    $now     = Get-Date
    $elapsed = $now - $CycleStartTime

    # 現在ステップまでの累計推定秒
    $cumSec = 0
    foreach ($k in $StepWeights.Keys) {
        $cumSec += $StepWeights[$k]
        if ($k -eq $Step) { break }
    }
    $pct       = [math]::Min(99, [math]::Round(($cumSec / $TotalEstSec) * 100))
    $remainSec = [math]::Max(0, $TotalEstSec - $elapsed.TotalSeconds)
    $eta       = $now.AddSeconds($remainSec)
    $etaStr    = $eta.ToString("HH:mm")

    $line = "[$Step] $Message (サイクル $Cycle/$Total, 経過: $("{0:mm\:ss}" -f $elapsed), ${pct}%, 完了予定: $etaStr)"
    Write-Host $line -ForegroundColor Cyan
    return $line
}

# ── 安全なコマンド実行 ──
function Run-Cmd {
    param([string]$Cmd, [int]$TimeoutSec = 900, [switch]$NoThrow)
    $pinfo = New-Object System.Diagnostics.ProcessStartInfo
    $pinfo.FileName  = "pwsh"; $pinfo.Arguments = "-NoProfile -Command `"$Cmd`""
    $pinfo.RedirectStandardOutput = $true; $pinfo.RedirectStandardError = $true
    $pinfo.UseShellExecute = $false; $pinfo.CreateNoWindow = $true
    $p = [System.Diagnostics.Process]::Start($pinfo)
    if (-not $p.WaitForExit($TimeoutSec * 1000)) {
        $p.Kill(); if (-not $NoThrow) { throw "TIMEOUT ($TimeoutSec s): $Cmd" }
    }
    $stdout = $p.StandardOutput.ReadToEnd()
    $stderr = $p.StandardError.ReadToEnd()
    if ($p.ExitCode -ne 0 -and -not $NoThrow) { throw "FAIL ($($p.ExitCode)): $Cmd`n$stderr" }
    return $stdout
}

# ── UTF-8 BOMなし書き込み ──
function Write-FileSafe {
    param([string]$Path, [string]$Content)
    $dir = Split-Path $Path -Parent
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

# ── .git/info/exclude 更新 ──
function Update-GitExclude {
    $exFile = Join-Path $RepoRoot ".git\info\exclude"
    $lines  = @(".codex-logs/", "worktrees/")
    if (Test-Path $exFile) {
        $existing = Get-Content $exFile -ErrorAction SilentlyContinue
        foreach ($l in $lines) {
            if ($existing -notcontains $l) { Add-Content $exFile $l }
        }
    }
}

# ── Discord 送信 ──
function Send-Discord {
    param([string]$Message)
    if (-not $DiscordWebhookUrl) { return }
    try {
        $body = @{ content = $Message } | ConvertTo-Json -Compress -Depth 3
        Invoke-RestMethod -Uri $DiscordWebhookUrl -Method Post -ContentType "application/json; charset=utf-8" -Body ([System.Text.Encoding]::UTF8.GetBytes($body)) | Out-Null
    } catch {
        Write-Host "  [discord] 送信失敗: $_" -ForegroundColor Yellow
    }
}

# ── Discord サイクルレポート（日本語 Before/After） ──
function Send-CycleReport {
    param(
        [int]$CycleId, [string]$PRTitle, [string]$PRUrl, [string]$BranchName,
        [hashtable]$Before, [hashtable]$After,
        [int]$RetryUsed, [int]$RetryMax, [int]$ReviewFixes,
        [string]$ElapsedTime, [string]$Status
    )
    if (-not $DiscordWebhookUrl) { return }
    $icon = if ($Status -eq "成功") { ":white_check_mark:" } else { ":x:" }
    $msg = @"
━━━━━━━━━━━━━━━━━━━━━━━━━━
$icon **サイクル #$CycleId 完了** | $Status
━━━━━━━━━━━━━━━━━━━━━━━━━━
:clipboard: **タスク:** $PRTitle
:herb: **ブランチ:** $BranchName

:bar_chart: **Before → After**
テスト合計     : $($Before.Tests) 件 → $($After.Tests) 件
テスト成功     : $($Before.TestPass) 件 → $($After.TestPass) 件
lint 警告      : $($Before.LintWarn) 件 → $($After.LintWarn) 件
lint エラー    : $($Before.LintErr) 件 → $($After.LintErr) 件
fmt ドリフト   : $($Before.FmtDrift) → $($After.FmtDrift)
変更ファイル数 : − → $($After.FilesChanged) 件
diff 行数      : − → $($After.DiffLines) 行

:wrench: **自己修復リトライ:** $RetryUsed / $RetryMax 回使用
:mag: **レビュー自動修正:** $ReviewFixes 件
:stopwatch: **所要時間:** $ElapsedTime

"@
    if ($PRUrl) { $msg += ":link: **PR:** $PRUrl`n" }
    $msg += "━━━━━━━━━━━━━━━━━━━━━━━━━━"
    Send-Discord $msg
}

# ── 仕様テンプレート読み込み ──
function Load-SpecTemplates {
    $tplDir = Join-Path $RepoRoot "prompts"
    $out = ""
    if (Test-Path $tplDir) {
        Get-ChildItem $tplDir -Filter "spec-*.md" | Sort-Object Name | ForEach-Object {
            $out += "`n## [$($_.BaseName)]`n$(Get-Content $_.FullName -Raw)`n"
        }
    }
    return $out
}

# ── 階層メモリ読み込み（Mnemis方式） ──
function Load-Memory {
    param([string]$MemDir)
    $out = ""
    if (Test-Path $MemDir) {
        foreach ($sub in @("patterns", "failures", "decisions")) {
            $d = Join-Path $MemDir $sub
            if (Test-Path $d) {
                Get-ChildItem $d -Filter "*.md" -ErrorAction SilentlyContinue |
                    Sort-Object LastWriteTime -Descending | Select-Object -First 5 |
                    ForEach-Object { $out += "`n### [$sub/$($_.Name)]`n$(Get-Content $_.FullName -Raw -ErrorAction SilentlyContinue)`n" }
            }
        }
    }
    return $out
}

# ── ハード制約ウェッジ ──
function Get-ConstraintWedge {
    return @"

## HARD CONSTRAINTS (MUST NOT VIOLATE)
- Max diff lines: $MaxDiffLines (reject if exceeded)
- Max files changed: $MaxFilesChanged (reject if exceeded)
- Forbidden patterns: console.log, TODO, FIXME, HACK (in new code)
- Required: all tests pass, lint 0 errors, cargo fmt clean
- Language: PR title and body in English, code comments in English
- Scope: one logical change per PR, no unrelated changes
"@
}

# ══════════════════════════════════════════
#  初期化
# ══════════════════════════════════════════
$RepoRoot = (git rev-parse --show-toplevel 2>$null)
if (-not $RepoRoot) { $RepoRoot = $PSScriptRoot }
$RepoRoot = (Resolve-Path $RepoRoot).Path

$LogDir = Join-Path $RepoRoot ".codex-logs"
$MemDir = Join-Path $LogDir "memory"
foreach ($d in @($LogDir, $MemDir, "$MemDir\patterns", "$MemDir\failures", "$MemDir\decisions")) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

Update-GitExclude

# 必須ファイル確認
$agentsMd = Join-Path $RepoRoot "AGENTS.md"
$planPath = Join-Path $RepoRoot $PlanFile
if (-not (Test-Path $agentsMd)) { throw "AGENTS.md not found at $agentsMd" }
if (-not (Test-Path $planPath)) { throw "$PlanFile not found at $planPath" }

$agentsContent = Get-Content $agentsMd -Raw
$planContent   = Get-Content $planPath -Raw
$specTemplates = Load-SpecTemplates
$memoryContext = Load-Memory $MemDir
$constraints   = Get-ConstraintWedge

Write-Host @"

========================================
 auto-dev v3 (worktree) loop started
 RepoRoot : $RepoRoot
 Max PRs  : $MaxPRs
 Sleep    : $SleepMinutes minutes
 AutoPush : $AutoPush  AutoPR : $AutoCreatePR  Draft : $DraftPR
 Retries  : $MaxRetries | MaxDiff: $MaxDiffLines | MaxFiles: $MaxFilesChanged
 Discord  : $(if ($DiscordWebhookUrl) {'enabled'} else {'disabled'})
 Memory   : $MemDir
 Started  : $(Get-Date -Format 'MM/dd/yyyy HH:mm:ss')
========================================
"@ -ForegroundColor Green

Send-Discord ":rocket: **auto-dev v3 起動** | 最大 $MaxPRs PR | $(Get-Date -Format 'HH:mm') 開始"

# ── ベースライン ──
Write-Host "[baseline] Running baseline checks..." -ForegroundColor DarkGray
$baselinePath = Join-Path $LogDir "baseline-$RunId.md"
$baseOut = "# Baseline $RunId`n"
try { $fmtBase = & cargo fmt --check --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String; $baseOut += "## cargo fmt`n$fmtBase`n" } catch {}
try { $lintBase = & npm run lint 2>&1 | Out-String; $baseOut += "## lint`n$lintBase`n" } catch {}
try { $testBase = & cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String; $baseOut += "## test`n$testBase`n" } catch {}
Write-FileSafe $baselinePath $baseOut
Write-Host "[baseline] saved: $baselinePath" -ForegroundColor DarkGray

$allPRUrls = @()

# ══════════════════════════════════════════
#  メインループ
# ══════════════════════════════════════════
for ($cycle = 1; $cycle -le $MaxPRs; $cycle++) {

    $script:CycleStartTime = Get-Date
    $prTitle    = ""
    $prUrl      = ""
    $branch     = ""
    $retryUsed  = 0
    $reviewFixCount = 0

    Write-Host "`n--- [$cycle / $MaxPRs] サイクル開始: $(Get-Date -Format 'MM/dd/yyyy HH:mm:ss') ---" -ForegroundColor Magenta
    Send-Discord ":arrows_counterclockwise: **サイクル $cycle / $MaxPRs 開始** | $(Get-Date -Format 'HH:mm')"

    # ── Before 計測 ──
    $beforeData = @{ Tests = 0; TestPass = 0; LintWarn = 0; LintErr = 0; FmtDrift = "不明" }
    $afterData  = @{ Tests = 0; TestPass = 0; LintWarn = 0; LintErr = 0; FmtDrift = "不明"; FilesChanged = 0; DiffLines = 0 }

    try {
        $fmtChk = & cargo fmt --check --manifest-path src-tauri/Cargo.toml 2>&1
        $beforeData.FmtDrift = if ($LASTEXITCODE -ne 0) { "あり" } else { "なし" }
    } catch { $beforeData.FmtDrift = "不明" }
    try {
        $lo = & npm run lint 2>&1 | Out-String
        if ($lo -match '(\d+)\s+error')   { $beforeData.LintErr  = [int]$Matches[1] }
        if ($lo -match '(\d+)\s+warning') { $beforeData.LintWarn = [int]$Matches[1] }
    } catch {}
    try {
        $to = & cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String
        if ($to -match 'test result: \w+\.\s+(\d+)\s+passed') {
            $beforeData.TestPass = [int]$Matches[1]; $beforeData.Tests = [int]$Matches[1]
        }
    } catch {}

    # worktree 作成
    $wtName = "wt-$RunId-$cycle"
    $wtPath = Join-Path $RepoRoot "worktrees\$wtName"
    try {
        git worktree add $wtPath main --detach 2>$null
    } catch {
        Write-Host "  [worktree] fallback: using repo root" -ForegroundColor Yellow
        $wtPath = $RepoRoot
    }

    Push-Location -LiteralPath $wtPath
    try {

    # ============================================================
    #  STEP A0 – Deep Research (Boris Cherny 方式)
    # ============================================================
    $stepMsg = Write-Step "A0" "コードベース深層リサーチ中..." $cycle $MaxPRs
    Send-Discord ":microscope: [A0] リサーチ開始 | サイクル $cycle"

    $researchPrompt = @"
You are a senior engineer performing deep research before planning.
Read the codebase, AGENTS.md, and the master plan carefully.
Focus on: architecture, patterns, recent changes, potential issues, test coverage gaps.

AGENTS.md:
$agentsContent

Master Plan:
$planContent

Memory from previous sessions:
$memoryContext

Output a structured research report as research-$cycle.md covering:
1. Current architecture overview
2. Recent change patterns
3. Risk areas and technical debt
4. Test coverage gaps
5. Recommended next focus area
"@

    $researchFile = Join-Path $LogDir "research-$cycle.md"
    try {
        codex --full-auto -m o4-mini "$researchPrompt" 2>$null
        if (Test-Path "research-$cycle.md") {
            Move-Item "research-$cycle.md" $researchFile -Force
        } else {
            Write-FileSafe $researchFile "# Research $cycle`nNo output generated."
        }
    } catch {
        Write-FileSafe $researchFile "# Research $cycle`nError: $_"
    }
    $researchContent = Get-Content $researchFile -Raw -ErrorAction SilentlyContinue

    # ============================================================
    #  STEP A – Planning（仕様テンプレート + 制約 + メモリ）
    # ============================================================
    $stepMsg = Write-Step "A" "次のPRを計画中..." $cycle $MaxPRs
    Send-Discord ":memo: [A] 計画策定中 | サイクル $cycle"

    $planPrompt = @"
You are a senior engineer. Based on the research, AGENTS.md, master plan, spec templates,
memory, and constraints below, determine the single most impactful next PR.

Research:
$researchContent

AGENTS.md:
$agentsContent

Master Plan:
$planContent

Spec Templates:
$specTemplates

Memory:
$memoryContext

$constraints

Output exactly:
- Line 1: branch name (e.g., pr70-fix-dashboard)
- Line 2: PR title (e.g., fix: correct dashboard stat calculation)
- Lines 3+: implementation plan (step by step, max 20 steps)

Save output as plan.md
"@

    $planOutFile = Join-Path $LogDir "plan-$cycle.md"
    try {
        codex --full-auto "$planPrompt" 2>$null
        if (Test-Path "plan.md") {
            Copy-Item "plan.md" $planOutFile -Force
            $planOut = Get-Content "plan.md" -Raw
        } else {
            $planOut = "pr${cycle}-auto-improvement`nchore: automated improvement cycle $cycle`nImplement improvements."
        }
    } catch {
        $planOut = "pr${cycle}-auto-improvement`nchore: automated improvement cycle $cycle`nImplement improvements."
    }

    $planLines = $planOut -split "`n" | Where-Object { $_.Trim() }
    $branch    = ($planLines[0] -replace '[^a-zA-Z0-9\-]', '-').ToLower().Substring(0, [math]::Min(60, $planLines[0].Length))
    $prTitle   = if ($planLines.Count -gt 1) { $planLines[1].Trim() } else { "chore: auto improvement $cycle" }

    Write-FileSafe $planOutFile $planOut

    # ============================================================
    #  STEP A1 – Self-Annotation（独立レビュー）
    # ============================================================
    $stepMsg = Write-Step "A1" "計画をセルフアノテーション中..." $cycle $MaxPRs
    Send-Discord ":mag_right: [A1] セルフアノテーション | サイクル $cycle"

    $annotPrompt = @"
You are an independent reviewer. Review this implementation plan critically.
Check for: scope creep, missing edge cases, constraint violations, unclear steps.
Add annotations with [OK], [WARN], or [FIX] prefix to each step.
If any step has [FIX], rewrite the corrected plan.

Plan:
$planOut

Constraints:
$constraints

Save annotated plan as plan-annotated.md
"@

    try {
        codex --full-auto -m o4-mini "$annotPrompt" 2>$null
        if (Test-Path "plan-annotated.md") {
            $planOut = Get-Content "plan-annotated.md" -Raw
            Copy-Item "plan-annotated.md" (Join-Path $LogDir "plan-annotated-$cycle.md") -Force
        }
    } catch {}

    # ============================================================
    #  STEP B – Implementation（ブランチ作成 + Codex実装）
    # ============================================================
    $stepMsg = Write-Step "B" "$branch で実装中..." $cycle $MaxPRs
    Send-Discord ":hammer_and_wrench: [B] 実装開始 | ``$branch`` | サイクル $cycle"

    $ErrorActionPreference = "Continue"
    git switch -c $branch 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { git checkout -b $branch 2>&1 | Out-Null }
    $ErrorActionPreference = "Stop"


    # node_modules クリーン
    if (Test-Path "node_modules") { Remove-Item "node_modules" -Recurse -Force -ErrorAction SilentlyContinue }
    if (Test-Path "package-lock.json") { Remove-Item "package-lock.json" -Force -ErrorAction SilentlyContinue }

    $implPrompt = @"
Implement the following plan precisely. Follow AGENTS.md rules strictly.
Run ``npm ci && npm run build`` if package.json exists.
Do NOT modify dotfiles, CI configs, or unrelated code.

AGENTS.md:
$agentsContent

Plan:
$planOut

$constraints
"@

    try {
        codex --full-auto "$implPrompt" 2>$null
    } catch {
        Write-Host "  [B] Codex implementation error: $_" -ForegroundColor Yellow
    }

    git add -A
    git commit -m "$prTitle" --allow-empty 2>$null

    # ============================================================
    #  STEP B-test – テスト + 自己修復リトライ
    # ============================================================
    $testResult = ""
    for ($retry = 1; $retry -le $MaxRetries; $retry++) {
        $stepMsg = Write-Step "B" "テスト実行中 (試行 $retry/$MaxRetries)..." $cycle $MaxPRs

        $testPassed = $true
        $testErrors = @()

        # cargo fmt
        try {
            & cargo fmt --check --manifest-path src-tauri/Cargo.toml 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                & cargo fmt --manifest-path src-tauri/Cargo.toml 2>$null
                git add -A; git commit --amend --no-edit 2>$null
                Write-Host "  [B-test] cargo fmt auto-fixed and commit amended." -ForegroundColor Yellow
            }
        } catch {}

        # lint
        try {
            $lintResult = & npm run lint 2>&1 | Out-String
            if ($lintResult -match '(\d+)\s+error' -and [int]$Matches[1] -gt 0) {
                $testPassed = $false; $testErrors += "lint errors: $($Matches[1])"
            }
        } catch {}

        # cargo test
        try {
            $testResult = & cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0) {
                $testPassed = $false; $testErrors += "cargo test failed"
            }
        } catch { $testPassed = $false; $testErrors += "cargo test exception: $_" }

        if ($testPassed) {
            Write-Host "  [B-test] 全テスト合格 (試行 $retry)" -ForegroundColor Green
            break
        }

        $retryUsed = $retry
        if ($retry -lt $MaxRetries) {
            Write-Host "  [B-test] 失敗 (試行 $retry): $($testErrors -join '; ')" -ForegroundColor Yellow
            Send-Discord ":warning: [B] テスト失敗 (試行 $retry/$MaxRetries) | 自己修復中..."

            $fixPrompt = @"
The following tests failed. Fix the issues and ensure all tests pass.
Do not change test expectations unless the tests themselves are wrong.

Errors:
$($testErrors -join "`n")

Test output:
$testResult

$constraints
"@
            try {
                codex --full-auto "$fixPrompt" 2>$null
                git add -A; git commit --amend --no-edit 2>$null
            } catch {}
        } else {
            Write-Host "  [B-test] 最大リトライ到達。続行。" -ForegroundColor Red
            Send-Discord ":x: [B] テスト失敗 | リトライ上限 $MaxRetries 到達"
        }
    }

    # ============================================================
    #  STEP B2 – Independent Code Review
    # ============================================================
    $stepMsg = Write-Step "B2" "独立コードレビュー中..." $cycle $MaxPRs
    Send-Discord ":mag: [B2] コードレビュー開始 | サイクル $cycle"

    $diffContent = git diff main --stat 2>$null | Out-String
    $diffFull    = git diff main 2>$null | Out-String

    $reviewPrompt = @"
You are an independent code reviewer. Review this diff carefully.

## Review checklist:
1. Correctness: Does the code do what the PR title says? Any logic bugs?
2. Security: Path traversal? Unsafe input handling? Secrets exposed?
3. Quality: Dead code? Duplicated logic? Missing error handling?
4. Tests: Are new behaviors tested? Are edge cases covered?
5. Diff hygiene: Any unrelated changes? Formatting-only hunks that should be separate?
6. AGENTS compliance: Does it follow the rules in AGENTS.md?
7. Constraint compliance: Is diff <= `$MaxDiffLines lines? Files <= `$MaxFilesChanged?

PR Title: $prTitle

AGENTS.md:
$agentsContent

Diff stat:
$diffContent

Full diff:
$diffFull

If you find issues, fix them directly and save a summary as review-fixes.md.
If no issues, save review-clean.md with "No issues found."
"@

    try {
        Write-FileSafe "review-prompt.txt" $reviewPrompt; codex --full-auto -m o4-mini (Get-Content "review-prompt.txt" -Raw) 2>$null
        if (Test-Path "review-fixes.md") {
            $reviewFixCount = (git diff --name-only 2>$null | Measure-Object).Count
            git add -A; git commit --amend --no-edit 2>$null
            Copy-Item "review-fixes.md" (Join-Path $LogDir "review-fixes-$cycle.md") -Force
            Write-Host "  [B2] レビュー修正: $reviewFixCount 件" -ForegroundColor Yellow
        } elseif (Test-Path "review-clean.md") {
            Write-Host "  [B2] レビュー: 問題なし" -ForegroundColor Green
        }
    } catch {
        Write-Host "  [B2] レビューエラー: $_" -ForegroundColor Yellow
    }

    Send-Discord ":mag: [B2] レビュー完了 | 修正 $reviewFixCount 件"

    # ============================================================
    #  STEP B+ – Evidence collection
    # ============================================================
    $stepMsg = Write-Step "B+" "証跡を収集中..." $cycle $MaxPRs

    $evidencePath = Join-Path $LogDir "evidence-$cycle.md"
    $ev = "# Evidence – Cycle $cycle`n`n"
    try { $ev += "## cargo test`n$(& cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String)`n" } catch {}
    try { $ev += "## npm run lint`n$(& npm run lint 2>&1 | Out-String)`n" } catch {}
    try { $ev += "## cargo fmt --check`n$(& cargo fmt --check --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String)`n" } catch {}
    try { $ev += "## git diff --stat`n$(git diff main --stat 2>&1 | Out-String)`n" } catch {}
    Write-FileSafe $evidencePath $ev

    # ============================================================
    #  STEP C – Summary generation
    # ============================================================
    $stepMsg = Write-Step "C" "PR要約を生成中..." $cycle $MaxPRs

    $summaryPrompt = @"
Summarize the changes in this PR for the GitHub PR description.
Write in English. Include: what changed, why, test results.
Keep it concise (max 200 words). Save as pr-summary.md.

PR Title: $prTitle
Diff stat:
$(git diff main --stat 2>$null | Out-String)
Evidence:
$(Get-Content $evidencePath -Raw -ErrorAction SilentlyContinue)
"@

    $prBody = ""
    try {
        codex --full-auto -m o4-mini "$summaryPrompt" 2>$null
        if (Test-Path "pr-summary.md") {
            $prBody = Get-Content "pr-summary.md" -Raw
            Copy-Item "pr-summary.md" (Join-Path $LogDir "summary-$cycle.md") -Force
        }
    } catch {}
    if (-not $prBody) { $prBody = "Automated PR by auto-dev v3. See evidence in .codex-logs." }

    # ============================================================
    #  STEP D – Push & PR creation
    # ============================================================
    $stepMsg = Write-Step "D" "プッシュ＆PR作成中..." $cycle $MaxPRs

    if ($AutoPush) {
        $ErrorActionPreference = "Continue"; git push origin $branch --force-with-lease 2>&1 | Out-Null; $ErrorActionPreference = "Stop"
        Write-Host "  [D] プッシュ完了: $branch" -ForegroundColor Green

        if ($AutoCreatePR) {
            $draftFlag = if ($DraftPR) { "--draft" } else { "" }
            try {
                $prResult = & gh pr create --base main --head $branch --title "$prTitle" --body "$prBody" $draftFlag 2>&1
                $prUrl = ($prResult | Select-String -Pattern 'https://github.com/.+/pull/\d+' | Select-Object -First 1).Matches.Value
                if ($prUrl) {
                    Write-Host "  [D] PR作成: $prUrl" -ForegroundColor Green
                    $allPRUrls += $prUrl
                } else {
                    Write-Host "  [D] PR作成結果: $prResult" -ForegroundColor Yellow
                }
            } catch {
                Write-Host "  [D] PR作成エラー: $_" -ForegroundColor Yellow
            }
        }
    }

    Send-Discord ":rocket: [D] プッシュ＆PR作成完了 | ``$branch``"

    # ── After 計測 ──
    try {
        $fc2 = & cargo fmt --check --manifest-path src-tauri/Cargo.toml 2>&1
        $afterData.FmtDrift = if ($LASTEXITCODE -ne 0) { "あり" } else { "なし" }
    } catch { $afterData.FmtDrift = "不明" }
    try {
        $lo2 = & npm run lint 2>&1 | Out-String
        if ($lo2 -match '(\d+)\s+error')   { $afterData.LintErr  = [int]$Matches[1] }
        if ($lo2 -match '(\d+)\s+warning') { $afterData.LintWarn = [int]$Matches[1] }
    } catch {}
    try {
        $to2 = & cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Out-String
        if ($to2 -match 'test result: \w+\.\s+(\d+)\s+passed') {
            $afterData.TestPass = [int]$Matches[1]; $afterData.Tests = [int]$Matches[1]
        }
    } catch {}
    try {
        $ds = git diff main --stat 2>$null | Out-String
        if ($ds -match '(\d+)\s+file')      { $afterData.FilesChanged = [int]$Matches[1] }
        if ($ds -match '(\d+)\s+insertion')  { $ins = [int]$Matches[1] } else { $ins = 0 }
        if ($ds -match '(\d+)\s+deletion')   { $del = [int]$Matches[1] } else { $del = 0 }
        $afterData.DiffLines = $ins + $del
    } catch {}

    $cycleElapsed = (Get-Date) - $script:CycleStartTime
    $elapsedStr   = "{0:mm\分ss\秒}" -f $cycleElapsed

    # ── 日本語 Before/After レポート送信 ──
    $cycleStatus = if ($prUrl) { "成功" } else { "失敗" }
    Send-CycleReport -CycleId $cycle -PRTitle $prTitle -PRUrl $prUrl -BranchName $branch `
        -Before $beforeData -After $afterData `
        -RetryUsed $retryUsed -RetryMax $MaxRetries -ReviewFixes $reviewFixCount `
        -ElapsedTime $elapsedStr -Status $cycleStatus

    # メタ情報保存
    $metaPath = Join-Path $LogDir "meta-$cycle.json"
    $meta = @{
        cycle = $cycle; branch = $branch; prTitle = $prTitle; prUrl = $prUrl
        retryUsed = $retryUsed; reviewFixes = $reviewFixCount
        elapsed = $cycleElapsed.TotalSeconds; status = $cycleStatus
        before = $beforeData; after = $afterData
    } | ConvertTo-Json -Depth 3
    Write-FileSafe $metaPath $meta

    } # end try
    finally {
        Pop-Location
    }

    # ============================================================
    #  STEP E – Auto-merge（repo root から実行）
    # ============================================================
    $stepMsg = Write-Step "E" "マージ中..." $cycle $MaxPRs

    if ($prUrl) {
        Push-Location -LiteralPath $RepoRoot
        try {
            Start-Sleep -Seconds 3
            $prNum = ($prUrl -split '/')[-1]
            $ErrorActionPreference = "Continue"; & gh pr merge $prNum --squash --delete-branch 2>&1 | Out-Null; $ErrorActionPreference = "Stop"
            if ($LASTEXITCODE -eq 0) {
                Write-Host "  [E] マージ成功: PR #$prNum" -ForegroundColor Green
                Send-Discord ":dart: [E] マージ完了 | PR #$prNum"
                $ErrorActionPreference = "Continue"; git pull origin main 2>&1 | Out-Null; $ErrorActionPreference = "Stop"
            } else {
                Write-Host "  [E] マージ不可（レビュー待ちまたはCI未完了）" -ForegroundColor Yellow
                Send-Discord ":hourglass: [E] マージ保留 | PR #$prNum（手動対応可）"
            }
        } catch {
            Write-Host "  [E] マージエラー: $_" -ForegroundColor Yellow
        }
        Pop-Location
    }

    # ── worktree cleanup ──
    try {
        git worktree remove $wtPath --force 2>$null
        Write-Host "  [cleanup] worktree removed: $wtName" -ForegroundColor DarkGray
    } catch {}

    # ── Memory 蓄積 ──
    $lessonPath = Join-Path $MemDir "patterns\cycle-$cycle-$RunId.md"
    $lesson = @"
# Cycle $cycle – $prTitle
- Status: $cycleStatus
- Retry: $retryUsed/$MaxRetries
- Review fixes: $reviewFixCount
- Elapsed: $elapsedStr
- Branch: $branch
"@
    Write-FileSafe $lessonPath $lesson

    # ── スリープ ──
    if ($cycle -lt $MaxPRs) {
        Write-Host "  [sleep] 次のサイクルまで $SleepMinutes 分待機..." -ForegroundColor DarkGray
        Send-Discord ":zzz: 次のサイクルまで $SleepMinutes 分待機"
        Start-Sleep -Seconds ($SleepMinutes * 60)
    }
}

# ══════════════════════════════════════════
#  終了処理
# ══════════════════════════════════════════
$totalElapsed = (Get-Date) - $StartTime
$totalStr     = "{0:hh\時間mm\分ss\秒}" -f $totalElapsed

$nightlyReport = @"
# Nightly Report – $RunId
- Total cycles: $MaxPRs
- Total elapsed: $totalStr
- PRs created: $($allPRUrls.Count)
$(($allPRUrls | ForEach-Object { "  - $_" }) -join "`n")
"@
Write-FileSafe (Join-Path $LogDir "nightly-$RunId.md") $nightlyReport

# 最終Discord通知
$finalMsg = @"
━━━━━━━━━━━━━━━━━━━━━━━━━━
:clipboard: **夜間レポート** | $RunId
━━━━━━━━━━━━━━━━━━━━━━━━━━
実行サイクル : $MaxPRs
合計所要時間 : $totalStr
作成PR数     : $($allPRUrls.Count)
$(($allPRUrls | ForEach-Object { ":link: $_" }) -join "`n")
━━━━━━━━━━━━━━━━━━━━━━━━━━
"@
Send-Discord $finalMsg

Write-Host @"

========================================
 auto-dev v3 completed
 Total PRs : $($allPRUrls.Count)
 Elapsed   : $totalStr
 Report    : $(Join-Path $LogDir "nightly-$RunId.md")
========================================
"@ -ForegroundColor Green



