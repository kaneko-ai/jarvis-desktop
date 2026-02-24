# auto-dev.ps1 (Hardened, Worktree-based, Long-run safe)
# - 1 cycle = 1 worktree (no dirty start contamination)
# - Writes plan/meta/evidence/result into .codex-logs (UTF-8 no BOM)
# - Script performs push + optional PR creation (gh) to avoid "local-only completion"
# - Stops on dirty repo unless explicitly overridden

[CmdletBinding()]
param(
  [int]$MaxPRs = 3,
  [int]$SleepMinutes = 2,
  [string]$PlanFile = "master-plan.md",

  # If omitted, defaults to the directory where this script lives.
  [string]$ProjectDir = "",

  # Long-run safety knobs
  [switch]$AllowDirtyStart = $false,
  [switch]$SkipBaselineChecks = $false,

  # PR automation
[bool]$AutoPush = $true,
[bool]$AutoCreatePR = $true,
[bool]$DraftPR = $true,

  # Worktree options
  [switch]$KeepWorktrees = $false
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# -------------------------
# Helpers
# -------------------------
function New-Utf8NoBom() {
  return New-Object System.Text.UTF8Encoding($false)
}

function Write-Utf8File([string]$Path, [string]$Content) {
  $dir = Split-Path -Parent $Path
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
  [System.IO.File]::WriteAllText($Path, $Content, (New-Utf8NoBom))
}

function Append-Utf8File([string]$Path, [string]$Content) {
  $dir = Split-Path -Parent $Path
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
  [System.IO.File]::AppendAllText($Path, $Content, (New-Utf8NoBom))
}

function Ensure-InfoExclude([string]$RepoRoot, [string[]]$Patterns) {
  $infoDir = Join-Path $RepoRoot ".git\info"
  if (-not (Test-Path $infoDir)) { return }
  $excludePath = Join-Path $infoDir "exclude"
  if (-not (Test-Path $excludePath)) { New-Item -ItemType File -Force -Path $excludePath | Out-Null }

  $current = @()
  try { $current = Get-Content $excludePath -ErrorAction SilentlyContinue } catch { $current = @() }

  foreach ($p in $Patterns) {
    if ($current -notcontains $p) {
      Add-Content -Path $excludePath -Value $p
    }
  }
}

function Get-LatestFile([string]$Dir, [string]$Pattern) {
  if (-not (Test-Path $Dir)) { return $null }
  $files = Get-ChildItem -Path $Dir -Filter $Pattern -File -ErrorAction SilentlyContinue |
           Sort-Object LastWriteTime -Descending
  if ($files -and $files.Count -gt 0) { return $files[0].FullName }
  return $null
}

function Truncate([string]$Text, [int]$MaxChars) {
  if (-not $Text) { return "" }
  if ($Text.Length -le $MaxChars) { return $Text }
  return $Text.Substring($Text.Length - $MaxChars)
}

function Run-Checked([string]$Label, [scriptblock]$Action) {
  try {
    return & $Action
  } catch {
    throw "[$Label] failed: $($_.Exception.Message)"
  }
}

# -------------------------
# Encoding (reduce mojibake)
# -------------------------
try {
  $utf8NoBom = New-Utf8NoBom
  [Console]::OutputEncoding = $utf8NoBom
  $OutputEncoding = $utf8NoBom
} catch {}

# -------------------------
# Resolve project dir safely
# -------------------------
if (-not $ProjectDir -or $ProjectDir.Trim() -eq "") {
  $ProjectDir = $PSScriptRoot
  if (-not $ProjectDir) { $ProjectDir = (Get-Location).Path }
}
$ProjectDir = (Resolve-Path -LiteralPath $ProjectDir).Path

Set-Location -LiteralPath $ProjectDir
$repoTop = (git rev-parse --show-toplevel 2>$null)
if (-not $repoTop) {
  throw "This directory is not inside a git repository: $ProjectDir"
}
$RepoRoot = (Resolve-Path -LiteralPath $repoTop).Path
Set-Location -LiteralPath $RepoRoot

# Required files
$AgentsPath = Join-Path $RepoRoot "AGENTS.md"
$PlanPath   = Join-Path $RepoRoot $PlanFile
if (-not (Test-Path $AgentsPath)) { throw "Missing AGENTS.md at repo root: $AgentsPath" }
if (-not (Test-Path $PlanPath))   { throw "Missing plan file: $PlanPath" }

# Logs + worktree root (kept under repo root but ignored via .git/info/exclude)
$LogDir = Join-Path $RepoRoot ".codex-logs"
$WtRoot = Join-Path $RepoRoot ".wt"
if (-not (Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
if (-not (Test-Path $WtRoot)) { New-Item -ItemType Directory -Force -Path $WtRoot | Out-Null }

Ensure-InfoExclude $RepoRoot @(
  ".codex-logs/",
  ".wt/"
)

# Tools sanity
$codexVer = (codex --version 2>$null)
if (-not $codexVer) { throw "codex CLI not found. Install it first." }

# GitHub CLI optional checks
$ghOk = $true
try { gh --version | Out-Null } catch { $ghOk = $false }
if ($AutoCreatePR -and -not $ghOk) {
  Write-Host "[warn] gh CLI missing; disabling AutoCreatePR." -ForegroundColor Yellow
  $AutoCreatePR = $false
}
if ($AutoCreatePR -and $ghOk) {
  try { gh auth status | Out-Null } catch {
    Write-Host "[warn] gh auth status failed; disabling AutoCreatePR." -ForegroundColor Yellow
    $AutoCreatePR = $false
  }
}

# Repo cleanliness guard
$dirty = (git status --porcelain)
if ($dirty -and -not $AllowDirtyStart) {
  throw "Dirty working tree detected at start. Fix or run with -AllowDirtyStart.`n$dirty"
}

$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " auto-dev (worktree) loop started" -ForegroundColor Cyan
Write-Host " RepoRoot : $RepoRoot" -ForegroundColor Cyan
Write-Host " Max PRs  : $MaxPRs" -ForegroundColor Cyan
Write-Host " Sleep    : $SleepMinutes minutes" -ForegroundColor Cyan
Write-Host " AutoPush : $AutoPush  AutoPR : $AutoCreatePR  Draft : $DraftPR" -ForegroundColor Cyan
Write-Host " Started  : $(Get-Date)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# Baseline checks (once)
$BaselineFile = Join-Path $LogDir "baseline-$Timestamp.md"
if (-not $SkipBaselineChecks) {
  $baseline = @()
  $baseline += "# Baseline Checks"
  $baseline += "Time: $(Get-Date)"
  $baseline += "Repo: $RepoRoot"
  $baseline += ""
  $baseline += "## git status --porcelain"
  $baseline += (git status --porcelain | Out-String)
  $baseline += "## git log --oneline -5"
  $baseline += (git log --oneline -5 | Out-String)

  $baseline += "## npm run lint"
  $baseline += (npm run lint 2>&1 | Out-String)

  $tauriDir = Join-Path $RepoRoot "src-tauri"
  if (Test-Path $tauriDir) {
    Push-Location $tauriDir
    $baseline += "## cargo fmt --check"
    $baseline += (cargo fmt --check 2>&1 | Out-String)
    $baseline += "## cargo test"
    $baseline += (cargo test 2>&1 | Out-String)
    Pop-Location
  } else {
    $baseline += "## src-tauri not found; skipping cargo checks"
  }

  Write-Utf8File $BaselineFile ($baseline -join "`n")
  Write-Host "[baseline] saved: $BaselineFile" -ForegroundColor Gray
}

# Track created PR URLs
$CreatedPRs = New-Object System.Collections.Generic.List[string]

for ($i = 1; $i -le $MaxPRs; $i++) {

  Write-Host ""
  Write-Host "--- [$i / $MaxPRs] Cycle start: $(Get-Date) ---" -ForegroundColor Yellow

  # Always sync main first (ff-only)
  Run-Checked "git-switch-main" { git switch main | Out-Null }
  Run-Checked "git-pull-ff"     { git pull --ff-only | Out-Null }

  # Ensure clean before creating worktree
  $dirtyNow = (git status --porcelain)
  if ($dirtyNow -and -not $AllowDirtyStart) {
    throw "Dirty working tree before cycle $i. Aborting.`n$dirtyNow"
  }

  $CycleId = "{0}-{1:D3}" -f $Timestamp, $i
  $PlanNextFile = Join-Path $LogDir "plan-next-$CycleId.md"
  $MetaFile     = Join-Path $LogDir "meta-$CycleId.json"
  $EvidenceFile = Join-Path $LogDir "evidence-$CycleId.md"
  $ResultFile   = Join-Path $LogDir "result-$CycleId.md"

  # Previous result (latest available)
  $PrevResultPath = Get-LatestFile $LogDir "result-*.md"
  $PrevResult = "No previous result."
  if ($PrevResultPath) {
    $PrevResult = Get-Content $PrevResultPath -Raw -Encoding utf8
    $PrevResult = Truncate $PrevResult 4000
  }

  # Create isolated worktree
  $WtPath = Join-Path $WtRoot "wt-$CycleId"
  if (Test-Path $WtPath) { Remove-Item -Recurse -Force $WtPath }

  # IMPORTANT:
  # main is already checked out in the primary worktree, so we cannot check out 'main' again.
  # Use detached HEAD worktree, then Codex will create a feature branch inside it.
  git worktree prune | Out-Null
  $wtOut = (git worktree add $WtPath --detach main 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0 -or -not (Test-Path $WtPath)) {
    throw "Failed to create worktree at $WtPath. git output:`n$wtOut"
  }

  try {
    Push-Location -LiteralPath $WtPath

    # STEP A: Plan
    Write-Host "[A] Planning next PR..." -ForegroundColor Green

    $PlanContent = Get-Content $PlanPath -Raw -Encoding utf8
    $PlanContent = Truncate $PlanContent 12000

    $PlanPrompt = @"
You are a senior developer working on the repository at: $WtPath

Hard rules:
- Read and follow: $AgentsPath
- Read and follow: $PlanPath
- Do NOT guess. Use rg and existing code/tests as evidence.
- Keep changes small and reviewable (aim <= 400 lines).
- One PR = one theme.

Context:
- Previous cycle result (truncated):
$PrevResult

Task:
Decide the single most important next PR to implement.
Output TWO files:

(1) A concrete instruction Markdown saved to:
$PlanNextFile

(2) A machine-readable meta JSON saved to:
$MetaFile

The JSON must include:
{
  "branch_name": "prNN-short-slug",
  "pr_title": "feat(desktop): ... / fix: ... / test: ... / ci: ... / docs: ...",
  "commit_message": "...",
  "tests_to_run": ["npm run lint", "cd src-tauri && cargo test", "cd src-tauri && cargo fmt --check"],
  "notes": "constraints / risks / why this PR now"
}

The instruction Markdown must include:
- Branch name
- PR title
- Detailed implementation steps
- Exact files to edit (candidate list)
- How to test
- Stop conditions (when to stop and report)

IMPORTANT:
- Save the files EXACTLY at the given absolute paths.
- Do not start implementation yet.
"@

    $StepALog = Join-Path $LogDir "step-a-$CycleId.log"
    codex exec --full-auto $PlanPrompt 2>&1 | Tee-Object -FilePath $StepALog | Out-Null

    if (-not (Test-Path $PlanNextFile)) { throw "Plan instruction file not generated: $PlanNextFile (see $StepALog)" }
    if (-not (Test-Path $MetaFile))     { throw "Meta JSON not generated: $MetaFile (see $StepALog)" }

    # Read meta
    $metaRaw = Get-Content $MetaFile -Raw -Encoding utf8
    $meta = $null
    try { $meta = $metaRaw | ConvertFrom-Json } catch { throw "Invalid JSON meta: $MetaFile" }
    if (-not $meta.branch_name -or -not $meta.pr_title -or -not $meta.commit_message) {
      throw "Meta JSON missing required keys. File: $MetaFile"
    }

    # STEP B: Implement (Codex edits code, creates branch, commits; NO push/PR here)
    Write-Host "[B] Implementing on branch $($meta.branch_name) ..." -ForegroundColor Green

    $PlanNext = Get-Content $PlanNextFile -Raw -Encoding utf8
    $PlanNext = Truncate $PlanNext 14000

    $ImplPrompt = @"
Repository workdir: $WtPath

Follow AGENTS: $AgentsPath
Follow master plan: $PlanPath

Implement the PR described below.

Rules:
- Create and switch to branch: $($meta.branch_name)
- Implement exactly one PR theme (no unrelated refactors).
- Run tests listed in meta (if a test fails due to pre-existing drift, record it explicitly).
- Commit with message: $($meta.commit_message)
- DO NOT push.
- DO NOT create GitHub PR.
- Keep changes small (<= 400 lines if possible). If larger, stop and explain.

Instruction:
$PlanNext

At the end, print:
- current branch name
- latest commit hash
- git diff --stat origin/main...HEAD (or main...HEAD)
"@

    $StepBLog = Join-Path $LogDir "step-b-$CycleId.log"
    codex exec --full-auto $ImplPrompt 2>&1 | Tee-Object -FilePath $StepBLog | Out-Null

    # Evidence collection (script-side; deterministic)
    Write-Host "[B] Collecting evidence..." -ForegroundColor DarkGreen

    $e = @()
    $e += "# Evidence $CycleId"
    $e += "Time: $(Get-Date)"
    $e += "Worktree: $WtPath"
    $e += "Branch(meta): $($meta.branch_name)"
    $e += ""

    $e += "## git status --porcelain"
    $e += (git status --porcelain | Out-String)

    $e += "## git branch --show-current"
    $e += (git branch --show-current | Out-String)

    $e += "## git log -1 --oneline --decorate"
    $e += (git log -1 --oneline --decorate | Out-String)

    $e += "## git diff --stat main...HEAD"
    $e += (git diff --stat main...HEAD | Out-String)

    $e += "## npm run lint"
    $e += (npm run lint 2>&1 | Out-String)

    $tauriDirWt = Join-Path $WtPath "src-tauri"
    if (Test-Path $tauriDirWt) {
      Push-Location $tauriDirWt
      $e += "## cargo fmt --check"
      $e += (cargo fmt --check 2>&1 | Out-String)
      $e += "## cargo test"
      $e += (cargo test 2>&1 | Out-String)
      Pop-Location
    } else {
      $e += "## src-tauri not found; skipping cargo checks"
    }

    Write-Utf8File $EvidenceFile ($e -join "`n")

    # STEP C: Summarize
    Write-Host "[C] Summarizing..." -ForegroundColor Green

    $SummaryPrompt = @"
You are a senior engineer writing a cycle summary.

Read:
- Instruction: $PlanNextFile
- Meta: $MetaFile
- Evidence: $EvidenceFile
- AGENTS: $AgentsPath
- Plan: $PlanPath

Write a concise but complete result report and save to:
$ResultFile

Must include:
- Branch name
- Intended PR title
- What changed (files + bullets)
- Test results (lint/fmt/test) and whether failures are pre-existing
- Remaining TODOs / blockers
- Whether it's safe to open a PR now

Output only the file; no extra chatter.
"@

    $StepCLog = Join-Path $LogDir "step-c-$CycleId.log"
    codex exec --full-auto $SummaryPrompt 2>&1 | Tee-Object -FilePath $StepCLog | Out-Null

    if (-not (Test-Path $ResultFile)) {
      Write-Utf8File $ResultFile "Result file not generated. See logs: $StepCLog"
    }

    # STEP D: Auto push + PR (script-controlled)
    $prUrl = ""
    if ($AutoPush -or $AutoCreatePR) {
      Write-Host "[D] Pushing branch / creating PR (script-side)..." -ForegroundColor Green

      $cur = (git branch --show-current).Trim()
      if ($cur -ne $meta.branch_name) {
        throw "Current branch '$cur' != expected '$($meta.branch_name)'. Aborting push/PR."
      }

      if ($AutoPush) {
        git push -u origin $meta.branch_name | Out-Null
      }

      if ($AutoCreatePR -and $ghOk) {
        $draftFlag = ""
        if ($DraftPR) { $draftFlag = "--draft" }

        $existing = ""
        try {
          $existing = (gh pr view $meta.branch_name --json url 2>$null | ConvertFrom-Json).url
        } catch { $existing = "" }

        if (-not $existing) {
          $body = @"
Summary:
- $($meta.pr_title)

How to test:
- npm run lint
- cd src-tauri && cargo fmt --check
- cd src-tauri && cargo test

Notes:
- See .codex-logs/evidence-$CycleId.md and result-$CycleId.md
"@
          $tmpBody = Join-Path $LogDir "pr-body-$CycleId.txt"
          Write-Utf8File $tmpBody $body

          $created = (gh pr create --base main --head $meta.branch_name --title $meta.pr_title --body-file $tmpBody $draftFlag 2>&1 | Out-String)
          $prUrl = ($created | Select-String -Pattern "https://github\.com/.+/pull/\d+" -AllMatches).Matches.Value | Select-Object -First 1
        } else {
          $prUrl = $existing
        }

        if ($prUrl) {
          $CreatedPRs.Add($prUrl) | Out-Null
          Append-Utf8File $ResultFile "`n`nPR: $prUrl`n"
        }
      }
    }

    # Show result
    Write-Host "Result ($CycleId):" -ForegroundColor Cyan
    Get-Content $ResultFile -Encoding utf8

  }
  finally {
    Pop-Location

    if (-not $KeepWorktrees) {
      try { git worktree remove $WtPath --force | Out-Null } catch {}
      try { if (Test-Path $WtPath) { Remove-Item -Recurse -Force $WtPath } } catch {}
    }
  }

  if ($i -lt $MaxPRs) {
    Write-Host "Sleeping $SleepMinutes minutes..." -ForegroundColor Gray
    Start-Sleep -Seconds ($SleepMinutes * 60)
  }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " auto-dev loop COMPLETE" -ForegroundColor Cyan
Write-Host " Finished: $(Get-Date)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# -------------------------
# POST-LOOP: Night report + next plan
# -------------------------
Write-Host ""
Write-Host "========================================" -ForegroundColor Magenta
Write-Host " Generating night report and next plan..." -ForegroundColor Magenta
Write-Host "========================================" -ForegroundColor Magenta

$ReportFile    = Join-Path $LogDir "night-report-$Timestamp.md"
$NextPlanFile  = Join-Path $LogDir "next-session-plan-$Timestamp.md"
$PRListFile    = Join-Path $LogDir "pr-urls-$Timestamp.txt"

if ($CreatedPRs.Count -gt 0) {
  Write-Utf8File $PRListFile ($CreatedPRs -join "`n")
} else {
  Write-Utf8File $PRListFile "(no PR urls recorded)"
}

$MgmtEvidence = @()
$MgmtEvidence += "# Nightly Evidence"
$MgmtEvidence += "Time: $(Get-Date)"
$MgmtEvidence += "Repo: $RepoRoot"
$MgmtEvidence += ""
$MgmtEvidence += "## git log --oneline -20"
$MgmtEvidence += (git log --oneline -20 | Out-String)
$MgmtEvidence += "## gh pr list (last 100)"
if ($ghOk) {
  $MgmtEvidence += (gh pr list --state all --limit 100 2>&1 | Out-String)
} else {
  $MgmtEvidence += "(gh not available)"
}
$MgmtEvidencePath = Join-Path $LogDir "mgmt-evidence-$Timestamp.md"
Write-Utf8File $MgmtEvidencePath ($MgmtEvidence -join "`n")

$ReportPrompt = @"
You are a senior engineering manager reviewing an automated overnight run.

Read:
- All cycle results: $LogDir\result-*.md
- All cycle evidence: $LogDir\evidence-*.md
- Tasks: $RepoRoot\tasks\todo.md (if exists), $RepoRoot\tasks\lessons.md (if exists)
- Repo evidence: $MgmtEvidencePath
- PR URLs file: $PRListFile
- AGENTS: $AgentsPath
- Plan: $PlanPath

Produce TWO files:

(1) Night report saved to:
$ReportFile

Must include:
- PR-by-PR status (merged/open/local-only)
- Failures / incomplete work
- Patterns / lessons
- Top risks / technical debt
- Recommended next 5-10 PRs

(2) Next session instruction plan saved to:
$NextPlanFile

Must include:
- Mandatory hygiene steps
- Concrete reconciliation steps (PR lifecycle gap closure)
- Verification debt closure steps
- Acceptance criteria and logging requirements
"@

$StepReportLog = Join-Path $LogDir "step-report-$Timestamp.log"
codex exec --full-auto $ReportPrompt 2>&1 | Tee-Object -FilePath $StepReportLog | Out-Null

if (Test-Path $ReportFile) {
  Write-Host ""
  Write-Host "=== Night Report ===" -ForegroundColor Cyan
  Get-Content $ReportFile -Encoding utf8
}
if (Test-Path $NextPlanFile) {
  Write-Host ""
  Write-Host "=== Next Session Plan ===" -ForegroundColor Cyan
  Get-Content $NextPlanFile -Encoding utf8
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Magenta
Write-Host " Report and next plan COMPLETE" -ForegroundColor Magenta
Write-Host " Logs: $LogDir" -ForegroundColor Magenta
Write-Host "========================================" -ForegroundColor Magenta