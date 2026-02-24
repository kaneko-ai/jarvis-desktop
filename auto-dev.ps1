# auto-dev.ps1 (Hardened, Worktree-based, Long-run safe)
# - 1 cycle = 1 worktree (no dirty start contamination)
# - Writes plan/meta/evidence/result into .codex-logs (UTF-8 no BOM)
# - Script performs push + optional PR creation (gh) to avoid "local-only completion"
# - STEP E: auto-merge (squash) after PR creation — runs from RepoRoot to avoid 'main already checked out'
# - Stops on dirty repo unless explicitly overridden
#
# Enhancements v2 (2026-02-25):
# - #1 Independent code review (STEP B2) after implementation
# - #2 Spec-driven prompts (prompts/ templates referenced in STEP A)
# - #4 Discord webhook notifications on every step completion/error
# - #6 Self-healing retry loop in STEP B (up to MaxRetries)
# - Boris Cherny method: STEP A0 (Deep Research) + STEP A1 (Self-Annotation)
# - Mnemis/koylanai: Hierarchical memory in .codex-logs/memory/
# - tabbata/super_bonochin: Constraint-wedge prompting (hard numeric constraints)
# - STEP E fix: merge from RepoRoot, not worktree
#
# IMPORTANT (Windows PowerShell 5.1):
# - git/cargo/npm often print progress to stderr even on success.
# - PowerShell 5.1 can treat that as an ErrorRecord; with $ErrorActionPreference="Stop" it kills the script.
# - Therefore we run native tools through cmd.exe and check exit codes.
#
# IMPORTANT (codex-cli 0.104.0+):
# - Long prompts passed as CLI arguments break on Windows due to quoting/escaping.
# - We write prompts to a temp file and pipe via stdin: Get-Content file | codex exec --full-auto -

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

  # PR automation (bool, not switch)
  [bool]$AutoPush = $true,
  [bool]$AutoCreatePR = $true,
  [bool]$DraftPR = $true,

  # Worktree options
  [switch]$KeepWorktrees = $false,

  # Self-healing retry (#6)
  [int]$MaxRetries = 3,

  # Constraint wedges (tabbata/super_bonochin)
  [int]$MaxDiffLines = 400,
  [int]$MaxFilesChanged = 12,
  [int]$MaxPromptTokenEstimate = 16000,

  # Discord webhook (#4) - empty string disables notifications
  [string]$DiscordWebhookUrl = "https://discord.com/api/webhooks/1475856515066892288/rj96GVnnXvyULPXj20pppIn7QXtAUkAbNIAa1KhwUHoSbbJYX3kTuRMnprT63_Qky32u"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# -------------------------
# Native-command safe runner (PS5.1)
# -------------------------
function Quote-CmdArg([string]$s) {
  if ($null -eq $s) { return '""' }
  return '"' + ($s -replace '"','""') + '"'
}

function Invoke-Cmd {
  param(
    [Parameter(Mandatory=$true)][string]$CmdLine,
    [string]$WorkingDirectory = ""
  )

  $out = @()
  $code = 0

  if ($WorkingDirectory -and $WorkingDirectory.Trim() -ne "") {
    Push-Location -LiteralPath $WorkingDirectory
    try {
      $out = cmd.exe /d /c "$CmdLine 2>&1"
      $code = $LASTEXITCODE
    } finally {
      Pop-Location
    }
  } else {
    $out = cmd.exe /d /c "$CmdLine 2>&1"
    $code = $LASTEXITCODE
  }

  return [pscustomobject]@{
    Output   = ($out -join "`n")
    ExitCode = $code
  }
}

function Run-CmdChecked {
  param(
    [Parameter(Mandatory=$true)][string]$Label,
    [Parameter(Mandatory=$true)][string]$CmdLine,
    [string]$WorkingDirectory = ""
  )

  $r = Invoke-Cmd -CmdLine $CmdLine -WorkingDirectory $WorkingDirectory
  if ($r.ExitCode -ne 0) {
    throw "[$Label] failed (exit=$($r.ExitCode)).`n$($r.Output)"
  }
  return $r.Output
}

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

# -------------------------
# #4 Discord notification helper
# -------------------------
function Send-Notification {
  param(
    [Parameter(Mandatory=$true)][string]$Message,
    [string]$Color = "3447003",
    [string]$Title = ""
  )

  if (-not $DiscordWebhookUrl -or $DiscordWebhookUrl.Trim() -eq "") { return }

  try {
    $safeMsg = $Message
    if ($safeMsg.Length -gt 1900) { $safeMsg = $safeMsg.Substring(0, 1900) + "... (truncated)" }

    $payload = @{
      embeds = @(
        @{
          title       = if ($Title) { $Title } else { "auto-dev.ps1" }
          description = $safeMsg
          color       = [int]$Color
          timestamp   = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
          footer      = @{ text = "Jarvis Auto-Dev" }
        }
      )
    } | ConvertTo-Json -Depth 5 -Compress

    Invoke-RestMethod -Uri $DiscordWebhookUrl -Method Post -ContentType "application/json; charset=utf-8" -Body ([System.Text.Encoding]::UTF8.GetBytes($payload)) | Out-Null
  } catch {
    Write-Host "[notify][warn] Discord notification failed: $_" -ForegroundColor Yellow
  }
}

# -------------------------
# Progress helper
# -------------------------
function Write-Progress-Step {
  param(
    [string]$Step,
    [string]$Message,
    [string]$Percent,
    [int]$Cycle,
    [int]$MaxCycles,
    [datetime]$LoopStart
  )
  $elapsed = ((Get-Date) - $LoopStart).ToString("hh\:mm\:ss")
  Write-Host "[$Step] $Message (cycle $Cycle/$MaxCycles, elapsed: $elapsed, $Percent)" -ForegroundColor Green
}

# -------------------------
# #2 Spec-driven template loader
# -------------------------
function Get-SpecTemplates {
  param([string]$RepoRoot)

  $promptsDir = Join-Path $RepoRoot "prompts"
  if (-not (Test-Path $promptsDir)) { return "" }

  $templates = @()
  $templateFiles = Get-ChildItem -Path $promptsDir -Filter "spec-*.md" -File -ErrorAction SilentlyContinue |
                   Sort-Object Name
  foreach ($f in $templateFiles) {
    $content = Get-Content $f.FullName -Raw -Encoding utf8
    $content = Truncate $content 2000
    $templates += "### Template: $($f.Name)"
    $templates += $content
    $templates += ""
  }

  if ($templates.Count -eq 0) { return "" }

  $header = @"

## Spec-Driven Development Templates (from prompts/)
Use these templates to structure your planning output.
Pick the most relevant template(s) for the current PR task.

"@
  return $header + ($templates -join "`n")
}

# -------------------------
# Mnemis/koylanai: Hierarchical memory loader
# -------------------------
function Get-MemoryContext {
  param([string]$LogDir)

  $memDir = Join-Path $LogDir "memory"
  if (-not (Test-Path $memDir)) { New-Item -ItemType Directory -Force -Path $memDir | Out-Null }

  $memoryBlock = @()

  # Category: lessons (from tasks/lessons.md + memory/lessons/)
  $lessonsDir = Join-Path $memDir "lessons"
  if (-not (Test-Path $lessonsDir)) { New-Item -ItemType Directory -Force -Path $lessonsDir | Out-Null }

  # Load tasks/lessons.md if exists
  $globalLessons = Join-Path $RepoRoot "tasks\lessons.md"
  if (Test-Path $globalLessons) {
    $content = Get-Content $globalLessons -Raw -Encoding utf8
    $content = Truncate $content 3000
    $memoryBlock += "## Memory: Global Lessons (tasks/lessons.md)"
    $memoryBlock += $content
    $memoryBlock += ""
  }

  # Load latest cycle-specific lessons
  $cycleLessons = Get-ChildItem -Path $lessonsDir -Filter "*.md" -File -ErrorAction SilentlyContinue |
                  Sort-Object LastWriteTime -Descending | Select-Object -First 5
  foreach ($f in $cycleLessons) {
    $content = Get-Content $f.FullName -Raw -Encoding utf8
    $content = Truncate $content 1000
    $memoryBlock += "### Memory: $($f.Name)"
    $memoryBlock += $content
    $memoryBlock += ""
  }

  # Category: patterns (recurring issues)
  $patternsFile = Join-Path $memDir "patterns.md"
  if (Test-Path $patternsFile) {
    $content = Get-Content $patternsFile -Raw -Encoding utf8
    $content = Truncate $content 2000
    $memoryBlock += "## Memory: Recurring Patterns"
    $memoryBlock += $content
    $memoryBlock += ""
  }

  # Category: architecture decisions
  $adrsDir = Join-Path $memDir "decisions"
  if (Test-Path $adrsDir) {
    $adrs = Get-ChildItem -Path $adrsDir -Filter "*.md" -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 3
    foreach ($f in $adrs) {
      $content = Get-Content $f.FullName -Raw -Encoding utf8
      $content = Truncate $content 1000
      $memoryBlock += "### Memory: Decision - $($f.Name)"
      $memoryBlock += $content
      $memoryBlock += ""
    }
  }

  if ($memoryBlock.Count -eq 0) { return "" }

  return "`n## Hierarchical Memory Context`n" + ($memoryBlock -join "`n")
}

# -------------------------
# Memory writer: save cycle learnings
# -------------------------
function Save-CycleLearnings {
  param(
    [string]$LogDir,
    [string]$CycleId,
    [string]$Learnings
  )

  $lessonsDir = Join-Path $LogDir "memory\lessons"
  if (-not (Test-Path $lessonsDir)) { New-Item -ItemType Directory -Force -Path $lessonsDir | Out-Null }

  $file = Join-Path $lessonsDir "cycle-$CycleId.md"
  Write-Utf8File $file $Learnings
}

# -------------------------
# Constraint wedge block (tabbata/super_bonochin)
# -------------------------
function Get-ConstraintWedges {
  return @"

## HARD CONSTRAINTS (non-negotiable numeric bounds)
These are simultaneous constraint wedges. ALL must be satisfied.
Violating ANY constraint = immediate STOP and report why.

| Constraint | Value | Enforcement |
|---|---|---|
| Max diff lines (insertions+deletions) | $MaxDiffLines | If git diff --stat shows more, split the PR |
| Max files changed | $MaxFilesChanged | If more files touched, split the PR |
| Max branch name length | 50 chars | Truncate slug if needed |
| Tests must pass | 100% of existing tests | New test failures = fix or revert |
| No new dependencies without justification | 0 unjustified | Document in PR body why needed |
| No type:any / type:unknown (TypeScript) | 0 instances | Use proper types |
| No console.log left in production code | 0 instances | Remove before commit |
| Commit message format | conventional commits | feat: / fix: / test: / ci: / docs: |
| PR scope | exactly 1 theme | No unrelated changes bundled |

These constraints exist to force the solution into a narrow, high-quality region of the solution space.
Do not treat them as suggestions. They are hard walls.
"@
}

# -------------------------
# Codex exec wrapper (stdin pipe method)
# -------------------------
function Invoke-Codex {
  param(
    [Parameter(Mandatory=$true)][string]$Prompt,
    [Parameter(Mandatory=$true)][string]$LogFile,
    [string]$PromptLabel = "prompt"
  )

  $promptFile = Join-Path $LogDir "$PromptLabel-$CycleId.txt"
  Write-Utf8File $promptFile $Prompt

  $oldEap = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    Get-Content $promptFile -Raw | codex exec --full-auto - 2>&1 | Tee-Object -FilePath $LogFile | Out-Null
    if ($LASTEXITCODE -ne 0) {
      throw "codex exec failed (exit=$LASTEXITCODE). Prompt: $promptFile  Log: $LogFile"
    }
  } finally {
    $ErrorActionPreference = $oldEap
  }
}

# Report-phase codex wrapper (uses $Timestamp instead of $CycleId)
function Invoke-CodexReport {
  param(
    [Parameter(Mandatory=$true)][string]$Prompt,
    [Parameter(Mandatory=$true)][string]$LogFile
  )

  $promptFile = Join-Path $LogDir "prompt-report-$Timestamp.txt"
  Write-Utf8File $promptFile $Prompt

  $oldEap = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    Get-Content $promptFile -Raw | codex exec --full-auto - 2>&1 | Tee-Object -FilePath $LogFile | Out-Null
    if ($LASTEXITCODE -ne 0) {
      throw "codex report failed (exit=$LASTEXITCODE). Prompt: $promptFile  Log: $LogFile"
    }
  } finally {
    $ErrorActionPreference = $oldEap
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

$repoTopR = Invoke-Cmd "git rev-parse --show-toplevel"
if ($repoTopR.ExitCode -ne 0 -or -not $repoTopR.Output.Trim()) {
  throw "This directory is not inside a git repository: $ProjectDir`n$($repoTopR.Output)"
}
$RepoRoot = (Resolve-Path -LiteralPath $repoTopR.Output.Trim()).Path
Set-Location -LiteralPath $RepoRoot

# Required files
$AgentsPath = Join-Path $RepoRoot "AGENTS.md"
$PlanPath   = Join-Path $RepoRoot $PlanFile
if (-not (Test-Path $AgentsPath)) { throw "Missing AGENTS.md at repo root: $AgentsPath" }
if (-not (Test-Path $PlanPath))   { throw "Missing plan file: $PlanPath" }

# Logs + worktree root
$LogDir = Join-Path $RepoRoot ".codex-logs"
$WtRoot = Join-Path $RepoRoot ".wt"
if (-not (Test-Path $LogDir)) { New-Item -ItemType Directory -Force -Path $LogDir | Out-Null }
if (-not (Test-Path $WtRoot)) { New-Item -ItemType Directory -Force -Path $WtRoot | Out-Null }

# Memory hierarchy directories (Mnemis)
$MemDir = Join-Path $LogDir "memory"
foreach ($sub in @("lessons", "decisions", "patterns")) {
  $p = Join-Path $MemDir $sub
  if (-not (Test-Path $p)) { New-Item -ItemType Directory -Force -Path $p | Out-Null }
}

Ensure-InfoExclude $RepoRoot @(
  ".codex-logs/",
  ".wt/",
  "prompts/"
)

# Tools sanity
$codexVer = Invoke-Cmd "codex --version"
if ($codexVer.ExitCode -ne 0) { throw "codex CLI not found. Install it first.`n$($codexVer.Output)" }

# GitHub CLI optional checks
$ghOk = $true
$ghVer = Invoke-Cmd "gh --version"
if ($ghVer.ExitCode -ne 0) { $ghOk = $false }

if ($AutoCreatePR -and -not $ghOk) {
  Write-Host "[warn] gh CLI missing; disabling AutoCreatePR." -ForegroundColor Yellow
  $AutoCreatePR = $false
}
if ($AutoCreatePR -and $ghOk) {
  $ghAuth = Invoke-Cmd "gh auth status"
  if ($ghAuth.ExitCode -ne 0) {
    Write-Host "[warn] gh auth status failed; disabling AutoCreatePR." -ForegroundColor Yellow
    $AutoCreatePR = $false
  }
}

# Repo cleanliness guard
$dirtyR = Invoke-Cmd "git status --porcelain"
$dirty = $dirtyR.Output.TrimEnd()
if ($dirty -and -not $AllowDirtyStart) {
  throw "Dirty working tree detected at start. Fix or run with -AllowDirtyStart.`n$dirty"
}

$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " auto-dev v2 (worktree) loop started" -ForegroundColor Cyan
Write-Host " RepoRoot : $RepoRoot" -ForegroundColor Cyan
Write-Host " Max PRs  : $MaxPRs" -ForegroundColor Cyan
Write-Host " Sleep    : $SleepMinutes minutes" -ForegroundColor Cyan
Write-Host " AutoPush : $AutoPush  AutoPR : $AutoCreatePR  Draft : $DraftPR" -ForegroundColor Cyan
Write-Host " Retries  : $MaxRetries | MaxDiff: $MaxDiffLines | MaxFiles: $MaxFilesChanged" -ForegroundColor Cyan
Write-Host " Discord  : $(if ($DiscordWebhookUrl) { 'enabled' } else { 'disabled' })" -ForegroundColor Cyan
Write-Host " Memory   : $MemDir" -ForegroundColor Cyan
Write-Host " Started  : $(Get-Date)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Send-Notification -Title "Loop Started" -Message "auto-dev v2 loop started.`nMaxPRs: $MaxPRs | Retries: $MaxRetries | MaxDiff: $MaxDiffLines`nRepo: $RepoRoot" -Color "3447003"

# Baseline checks (once)
$BaselineFile = Join-Path $LogDir "baseline-$Timestamp.md"
if (-not $SkipBaselineChecks) {
  Write-Host "[baseline] Running baseline checks..." -ForegroundColor Gray
  $baseline = @()
  $baseline += "# Baseline Checks"
  $baseline += "Time: $(Get-Date)"
  $baseline += "Repo: $RepoRoot"
  $baseline += ""

  $baseline += "## git status --porcelain"
  $baseline += (Run-CmdChecked "baseline-git-status" "git status --porcelain")

  $baseline += "## git log --oneline -5"
  $baseline += (Run-CmdChecked "baseline-git-log" "git log --oneline -5")

  $baseline += "## npm run lint"
  $baseline += (Run-CmdChecked "baseline-npm-lint" "npm run lint")

  $tauriDir = Join-Path $RepoRoot "src-tauri"
  if (Test-Path $tauriDir) {
    $baseline += "## cargo fmt --check"
    $fmtR = Invoke-Cmd -CmdLine "cargo fmt --check" -WorkingDirectory $tauriDir
    $baseline += $fmtR.Output
    if ($fmtR.ExitCode -ne 0) {
      $baseline += "(pre-existing fmt drift detected; non-fatal)"
      Write-Host "[baseline] cargo fmt --check: pre-existing drift (non-fatal)" -ForegroundColor Yellow
    }

    $baseline += "## cargo test"
    $baseline += (Run-CmdChecked "baseline-cargo-test" "cargo test" $tauriDir)
  } else {
    $baseline += "## src-tauri not found; skipping cargo checks"
  }

  Write-Utf8File $BaselineFile ($baseline -join "`n")
  Write-Host "[baseline] saved: $BaselineFile" -ForegroundColor Gray
}

# Track created PR URLs
$CreatedPRs = New-Object System.Collections.Generic.List[string]

# Loop start time for progress tracking
$LoopStartTime = Get-Date

# Load spec templates and constraint wedges once
$SpecTemplateBlock = Get-SpecTemplates -RepoRoot $RepoRoot
$ConstraintBlock = Get-ConstraintWedges

for ($i = 1; $i -le $MaxPRs; $i++) {

  $cycleStartTime = Get-Date
  Write-Host ""
  Write-Host "--- [$i / $MaxPRs] Cycle start: $(Get-Date) ---" -ForegroundColor Yellow
  Send-Notification -Title "Cycle $i/$MaxPRs Started" -Message "Starting cycle $i of $MaxPRs at $(Get-Date -Format 'HH:mm:ss')" -Color "3447003"

  # Always sync main first (ff-only)
  Run-CmdChecked "git-switch-main" "git switch main" | Out-Null
  Run-CmdChecked "git-pull-ff" "git pull --ff-only" | Out-Null

  # Ensure clean before creating worktree
  $dirtyNowR = Invoke-Cmd "git status --porcelain"
  $dirtyNow = $dirtyNowR.Output.TrimEnd()
  if ($dirtyNow -and -not $AllowDirtyStart) {
    throw "Dirty working tree before cycle $i. Aborting.`n$dirtyNow"
  }

  $CycleId = "{0}-{1:D3}" -f $Timestamp, $i
  $ResearchFile = Join-Path $LogDir "research-$CycleId.md"
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

  # Load hierarchical memory (Mnemis)
  $MemoryBlock = Get-MemoryContext -LogDir $LogDir

  # Create isolated worktree
  $WtPath = Join-Path $WtRoot "wt-$CycleId"
  if (Test-Path $WtPath) { Remove-Item -Recurse -Force $WtPath }

  Run-CmdChecked "git-worktree-prune" "git worktree prune" | Out-Null

  $wtAddCmd = "git worktree add " + (Quote-CmdArg $WtPath) + " --detach main"
  $wtOut = Run-CmdChecked "git-worktree-add" $wtAddCmd
  if (-not (Test-Path $WtPath)) {
    throw "Failed to create worktree at $WtPath. git output:`n$wtOut"
  }

  try {
    Push-Location -LiteralPath $WtPath

    # =====================
    # STEP A0: Deep Research (Boris Cherny) (~0-5%)
    # =====================
    Write-Progress-Step -Step "A0" -Message "Deep research on target area..." -Percent "~3%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $PlanContent = Get-Content $PlanPath -Raw -Encoding utf8
    $PlanContent = Truncate $PlanContent 12000

    $ResearchPrompt = @"
You are a senior developer conducting deep research on a codebase.
Repository: $WtPath

Read and follow: $AgentsPath

Your task: Identify the SINGLE most important next PR from the master plan,
then deeply research the relevant parts of the codebase.

Master plan (truncated):
$PlanContent

Previous cycle result (truncated):
$PrevResult
$MemoryBlock

RESEARCH INSTRUCTIONS (Boris Cherny method):
- Read the relevant folders and files IN DEPTH. Not just signatures — understand the full logic.
- Use rg (ripgrep) to find all usages, callers, and dependencies.
- Understand the intricacies: edge cases, error handling, state management.
- Look for potential bugs, inconsistencies, and missing test coverage.
- DO NOT skim. Surface-level reading is NOT acceptable.

Save a detailed research report to:
$ResearchFile

The report MUST include:
- Which PR task you selected and why
- Files and modules examined (with line counts)
- How the current code works (data flow, state, dependencies)
- Existing test coverage for the target area
- Potential risks, bugs, or gotchas discovered
- Pre-existing issues that must NOT be fixed in this PR (out of scope)

DO NOT write any plan yet. DO NOT implement anything. Research ONLY.
"@

    $StepA0Log = Join-Path $LogDir "step-a0-$CycleId.log"
    Invoke-Codex -Prompt $ResearchPrompt -LogFile $StepA0Log -PromptLabel "prompt-a0"

    if (-not (Test-Path $ResearchFile)) {
      Write-Utf8File $ResearchFile "(Research file not generated. See $StepA0Log)"
    }

    Send-Notification -Title "STEP A0 Research Complete" -Message "Research saved: research-$CycleId.md" -Color "3066993"

    # =====================
    # STEP A: Plan (~5-12%)  [#2 spec templates + constraint wedges]
    # =====================
    Write-Progress-Step -Step "A" -Message "Planning next PR based on research..." -Percent "~8%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $ResearchContent = Get-Content $ResearchFile -Raw -Encoding utf8
    $ResearchContent = Truncate $ResearchContent 8000

    $PlanPrompt = @"
You are a senior developer writing an implementation plan.
Repository: $WtPath

Hard rules:
- Read and follow: $AgentsPath
- Read and follow: $PlanPath
- Base your plan on the research findings below. Do NOT ignore them.
- Do NOT guess. The research already examined the code deeply.
$ConstraintBlock
$SpecTemplateBlock
$MemoryBlock

Research findings:
$ResearchContent

Task:
Write a detailed implementation plan based on the research.
Output TWO files:

(1) A concrete instruction Markdown saved to:
$PlanNextFile

Structure the instruction using spec-driven templates where applicable:
- Requirements summary (what and why)
- Design rationale (how, based on research findings)
- Implementation checklist (step-by-step tasks with IDs: T-1, T-2, ...)
- Test and verification plan
- Stop conditions

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

IMPORTANT:
- Save the files EXACTLY at the given absolute paths.
- Do not start implementation yet.
"@

    $StepALog = Join-Path $LogDir "step-a-$CycleId.log"
    Invoke-Codex -Prompt $PlanPrompt -LogFile $StepALog -PromptLabel "prompt-a"

    if (-not (Test-Path $PlanNextFile)) { throw "Plan instruction file not generated: $PlanNextFile (see $StepALog)" }
    if (-not (Test-Path $MetaFile))     { throw "Meta JSON not generated: $MetaFile (see $StepALog)" }

    # Read meta
    $metaRaw = Get-Content $MetaFile -Raw -Encoding utf8
    $meta = $null
    try { $meta = $metaRaw | ConvertFrom-Json } catch { throw "Invalid JSON meta: $MetaFile" }
    if (-not $meta.branch_name -or -not $meta.pr_title -or -not $meta.commit_message) {
      throw "Meta JSON missing required keys. File: $MetaFile"
    }

    Send-Notification -Title "STEP A Plan Complete" -Message "Plan: **$($meta.pr_title)**`nBranch: ``$($meta.branch_name)``" -Color "3066993"

    # =====================
    # STEP A1: Self-Annotation (Boris Cherny annotation cycle) (~12-18%)
    # =====================
    Write-Progress-Step -Step "A1" -Message "Self-annotating plan (independent review)..." -Percent "~15%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $PlanDraft = Get-Content $PlanNextFile -Raw -Encoding utf8
    $PlanDraft = Truncate $PlanDraft 10000

    $AnnotatePrompt = @"
You are an independent senior architect reviewing a plan written by another developer.
You have NO context from the planning session. You must evaluate the plan critically.

Repository: $WtPath
AGENTS rules: $AgentsPath
Master plan: $PlanPath
Research: $ResearchFile
$ConstraintBlock

## Plan to review:
$PlanDraft

## Your review tasks:
1. Check if the plan addresses all findings from the research
2. Check if implementation steps are concrete enough (not vague)
3. Check if the plan respects ALL hard constraints (diff lines, file count, scope)
4. Check for missing edge cases, error handling, or test scenarios
5. Check if the plan accidentally includes out-of-scope changes
6. Check for over-engineering — prefer minimal-diff solutions

## Your output:
- Add inline annotations directly into the plan document where corrections are needed
- Format annotations as: **[ANNOTATION]:** your correction here
- After annotating, rewrite the ENTIRE plan with corrections applied
- Save the corrected plan to the SAME path: $PlanNextFile
- Update $MetaFile if the annotation changed branch name, title, or scope

If the plan is already excellent, still save it (unchanged) to confirm review passed.

DO NOT implement anything. Annotation and plan revision ONLY.
"@

    $StepA1Log = Join-Path $LogDir "step-a1-$CycleId.log"
    Invoke-Codex -Prompt $AnnotatePrompt -LogFile $StepA1Log -PromptLabel "prompt-a1"

    # Re-read meta in case annotation changed it
    if (Test-Path $MetaFile) {
      $metaRaw = Get-Content $MetaFile -Raw -Encoding utf8
      try { $meta = $metaRaw | ConvertFrom-Json } catch {}
    }

    Send-Notification -Title "STEP A1 Annotation Complete" -Message "Plan reviewed and revised for: **$($meta.pr_title)**" -Color "3066993"

    # =====================
    # STEP B: Implement (~18-50%)  [#6 self-healing retry loop]
    # =====================
    Write-Progress-Step -Step "B" -Message "Implementing on branch $($meta.branch_name)..." -Percent "~20%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $PlanNext = Get-Content $PlanNextFile -Raw -Encoding utf8
    $PlanNext = Truncate $PlanNext 14000

    $ImplPrompt = @"
Repository workdir: $WtPath

Follow AGENTS: $AgentsPath
Follow master plan: $PlanPath
$ConstraintBlock

Implement the PR described below.

Rules:
- Create and switch to branch: $($meta.branch_name)
- Implement exactly one PR theme (no unrelated refactors).
- Run tests listed in meta (if a test fails due to pre-existing drift, record it explicitly).
- Commit with message: $($meta.commit_message)
- DO NOT push.
- DO NOT create GitHub PR.
- Keep changes within constraint bounds (max $MaxDiffLines diff lines, max $MaxFilesChanged files).
- If constraints would be violated, STOP and explain in the result what must be split.

Instruction:
$PlanNext

At the end, print:
- current branch name
- latest commit hash
- git diff --stat origin/main...HEAD (or main...HEAD)
"@

    $StepBLog = Join-Path $LogDir "step-b-$CycleId.log"
    Invoke-Codex -Prompt $ImplPrompt -LogFile $StepBLog -PromptLabel "prompt-b"

    # --- #6 Self-healing retry loop ---
    $retryCount = 0
    $testsPass = $false

    while ($retryCount -lt $MaxRetries) {
      Write-Progress-Step -Step "B-test" -Message "Running tests (attempt $($retryCount + 1)/$MaxRetries)..." -Percent "~$([int](30 + $retryCount * 7))%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

      $testErrors = @()

      # npm lint
      $lintR = Invoke-Cmd "npm run lint"
      if ($lintR.ExitCode -ne 0) { $testErrors += "npm run lint failed:`n$($lintR.Output)" }

      # cargo tests
      $tauriDirWt = Join-Path $WtPath "src-tauri"
      if (Test-Path $tauriDirWt) {
        $fmtR = Invoke-Cmd -CmdLine "cargo fmt --check" -WorkingDirectory $tauriDirWt
        if ($fmtR.ExitCode -ne 0) {
          $fixR = Invoke-Cmd -CmdLine "cargo fmt" -WorkingDirectory $tauriDirWt
          $fmtR2 = Invoke-Cmd -CmdLine "cargo fmt --check" -WorkingDirectory $tauriDirWt
          if ($fmtR2.ExitCode -ne 0) {
            $testErrors += "cargo fmt --check failed (even after auto-fix):`n$($fmtR2.Output)"
          } else {
            $amendR = Invoke-Cmd "git add -A && git commit --amend --no-edit"
            Write-Host "[B-test] cargo fmt auto-fixed and commit amended." -ForegroundColor DarkGreen
          }
        }

        $testR = Invoke-Cmd -CmdLine "cargo test" -WorkingDirectory $tauriDirWt
        if ($testR.ExitCode -ne 0) { $testErrors += "cargo test failed:`n$($testR.Output)" }
      }

      if ($testErrors.Count -eq 0) {
        $testsPass = $true
        Write-Host "[B-test] All tests passed on attempt $($retryCount + 1)." -ForegroundColor Green
        break
      }

      $retryCount++

      if ($retryCount -ge $MaxRetries) {
        Write-Host "[B-test][warn] Tests still failing after $MaxRetries attempts. Proceeding." -ForegroundColor Yellow
        Send-Notification -Title "STEP B: Tests Failed" -Message "Tests failed after $MaxRetries retries.`n$($testErrors[0])" -Color "15158332"
        break
      }

      Write-Host "[B-test] Tests failed (attempt $retryCount). Asking Codex to fix..." -ForegroundColor Yellow

      $fixPrompt = @"
Repository workdir: $WtPath
Branch: $($meta.branch_name)

The following test failures occurred after implementation:

$($testErrors -join "`n`n---`n`n")

Rules:
- Fix ONLY the issues above. Do not refactor or change unrelated code.
- Stay on branch $($meta.branch_name).
- Amend the existing commit (git commit --amend --no-edit) after fixing.
- Do NOT push. Do NOT create PR.
- If a failure is pre-existing (not caused by your changes), document it but do not fix.

Fix the failures now.
"@

      $StepBFixLog = Join-Path $LogDir "step-b-fix-$CycleId-r$retryCount.log"
      Invoke-Codex -Prompt $fixPrompt -LogFile $StepBFixLog -PromptLabel "prompt-b-fix-r$retryCount"
    }

    Send-Notification -Title "STEP B Complete" -Message "Tests: $(if ($testsPass) { 'PASSED' } else { 'FAILED' }) | Retries: $retryCount/$MaxRetries" -Color $(if ($testsPass) { "3066993" } else { "16776960" })

    # =====================
    # STEP B2: Independent Code Review (~50-60%)  [#1]
    # =====================
    Write-Progress-Step -Step "B2" -Message "Independent code review..." -Percent "~55%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $diffForReview = Invoke-Cmd "git diff main...HEAD"
    $diffText = Truncate $diffForReview.Output 12000

    $diffStatForReview = Invoke-Cmd "git diff --stat main...HEAD"
    $diffStatText = $diffStatForReview.Output

    $ReviewPrompt = @"
You are an independent senior code reviewer. You have NO context from the implementation session.
You are reviewing a PR for the jarvis-desktop project.

AGENTS rules: $AgentsPath
Master plan: $PlanPath
Research: $ResearchFile
$ConstraintBlock

PR title: $($meta.pr_title)
Branch: $($meta.branch_name)

## Changed files (stat):
$diffStatText

## Full diff:
$diffText

## Review checklist:
1. Correctness: Does the code do what the PR title says? Any logic bugs?
2. Security: Path traversal? Unsafe input handling? Secrets exposed?
3. Quality: Dead code? Duplicated logic? Missing error handling?
4. Tests: Are new behaviors tested? Are edge cases covered?
5. Diff hygiene: Any unrelated changes? Formatting-only hunks that should be separate?
6. AGENTS compliance: Does it follow the rules in AGENTS.md?
7. Constraint compliance: Is diff <= $MaxDiffLines lines? Files <= $MaxFilesChanged?

## Your task:
- If you find issues that are FIXABLE (not pre-existing), fix them directly in the code.
- Amend the commit after fixes: git add -A && git commit --amend --no-edit
- If issues are minor or pre-existing, just document them.
- Save a review summary to: $LogDir\review-$CycleId.md
- The review summary must list: issues found, fixes applied, remaining concerns.
- Do NOT push. Do NOT create PR.

## Memory extraction:
Also save a brief lessons-learned note (2-5 bullets) to:
$LogDir\memory\lessons\cycle-$CycleId.md
Format: | what happened | root cause | prevention rule |
"@

    $StepB2Log = Join-Path $LogDir "step-b2-$CycleId.log"
    Invoke-Codex -Prompt $ReviewPrompt -LogFile $StepB2Log -PromptLabel "prompt-b2"

    # Quick re-test after review fixes
    $postReviewTestOk = $true
    $lintR2 = Invoke-Cmd "npm run lint"
    if ($lintR2.ExitCode -ne 0) { $postReviewTestOk = $false }
    if (Test-Path $tauriDirWt) {
      $testR2 = Invoke-Cmd -CmdLine "cargo test" -WorkingDirectory $tauriDirWt
      if ($testR2.ExitCode -ne 0) { $postReviewTestOk = $false }
    }

    if (-not $postReviewTestOk) {
      Write-Host "[B2][warn] Post-review tests failed." -ForegroundColor Yellow
      Send-Notification -Title "STEP B2: Post-Review Test Failed" -Message "Tests failed after code review fixes." -Color "15158332"
    } else {
      Write-Host "[B2] Post-review tests passed." -ForegroundColor Green
    }

    $reviewFile = Join-Path $LogDir "review-$CycleId.md"
    $reviewSummary = "(no review file)"
    if (Test-Path $reviewFile) {
      $reviewSummary = Get-Content $reviewFile -Raw -Encoding utf8
      $reviewSummary = Truncate $reviewSummary 500
    }
    Send-Notification -Title "STEP B2 Review Complete" -Message "Post-review tests: $(if ($postReviewTestOk) { 'PASSED' } else { 'FAILED' })`n$reviewSummary" -Color $(if ($postReviewTestOk) { "3066993" } else { "16776960" })

    # =====================
    # Evidence collection (~60-65%)
    # =====================
    Write-Progress-Step -Step "B+" -Message "Collecting evidence..." -Percent "~65%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $e = @()
    $e += "# Evidence $CycleId"
    $e += "Time: $(Get-Date)"
    $e += "Worktree: $WtPath"
    $e += "Branch(meta): $($meta.branch_name)"
    $e += "Self-heal retries used: $retryCount/$MaxRetries"
    $e += "Tests pass after impl: $testsPass"
    $e += "Tests pass after review: $postReviewTestOk"
    $e += ""

    $e += "## git status --porcelain"
    $e += (Run-CmdChecked "evidence-git-status" "git status --porcelain")

    $e += "## git branch --show-current"
    $e += (Run-CmdChecked "evidence-git-branch" "git branch --show-current")

    $e += "## git log -1 --oneline --decorate"
    $e += (Run-CmdChecked "evidence-git-log" "git log -1 --oneline --decorate")

    $e += "## git diff --stat main...HEAD"
    $e += (Run-CmdChecked "evidence-git-diffstat" "git diff --stat main...HEAD")

    $e += "## npm run lint"
    $e += (Run-CmdChecked "evidence-npm-lint" "npm run lint")

    $tauriDirWt = Join-Path $WtPath "src-tauri"
    if (Test-Path $tauriDirWt) {
      $e += "## cargo fmt --check"
      $fmtR = Invoke-Cmd -CmdLine "cargo fmt --check" -WorkingDirectory $tauriDirWt
      $e += $fmtR.Output
      if ($fmtR.ExitCode -ne 0) {
        $e += "(pre-existing fmt drift; non-fatal)"
        Write-Host "[evidence] cargo fmt drift detected (non-fatal)" -ForegroundColor Yellow
      }

      $e += "## cargo test"
      $e += (Run-CmdChecked "evidence-cargo-test" "cargo test" $tauriDirWt)
    } else {
      $e += "## src-tauri not found; skipping cargo checks"
    }

    Write-Utf8File $EvidenceFile ($e -join "`n")

    # =====================
    # STEP C: Summarize (~70-75%)
    # =====================
    Write-Progress-Step -Step "C" -Message "Summarizing..." -Percent "~75%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

    $SummaryPrompt = @"
You are a senior engineer writing a cycle summary.

Read:
- Research: $ResearchFile
- Instruction: $PlanNextFile
- Meta: $MetaFile
- Evidence: $EvidenceFile
- Review: $LogDir\review-$CycleId.md (if exists)
- AGENTS: $AgentsPath
- Plan: $PlanPath

Write a concise but complete result report and save to:
$ResultFile

Must include:
- Branch name
- Intended PR title
- What changed (files + bullets)
- Test results (lint/fmt/test) and whether failures are pre-existing
- Code review findings and fixes applied
- Self-healing retries used ($retryCount/$MaxRetries)
- Constraint compliance (diff lines, files changed vs limits)
- Remaining TODOs / blockers
- Whether it's safe to open a PR now

Output only the file; no extra chatter.
"@

    $StepCLog = Join-Path $LogDir "step-c-$CycleId.log"
    Invoke-Codex -Prompt $SummaryPrompt -LogFile $StepCLog -PromptLabel "prompt-c"

    if (-not (Test-Path $ResultFile)) {
      Write-Utf8File $ResultFile "Result file not generated. See logs: $StepCLog"
    }

    # =====================
    # STEP D: Auto push + PR (~80-85%)
    # =====================
    $prUrl = ""
    if ($AutoPush -or $AutoCreatePR) {
      Write-Progress-Step -Step "D" -Message "Pushing branch / creating PR..." -Percent "~85%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

      $cur = (Run-CmdChecked "git-branch-current" "git branch --show-current").Trim()
      if ($cur -ne $meta.branch_name) {
        throw "Current branch '$cur' != expected '$($meta.branch_name)'. Aborting push/PR."
      }

      if ($AutoPush) {
        Run-CmdChecked "git-push" ("git push -u origin {0}" -f $meta.branch_name) | Out-Null
      }

      if ($AutoCreatePR -and $ghOk) {
        $draftFlag = ""
        if ($DraftPR) { $draftFlag = "--draft" }

        $existing = ""
        $viewR = Invoke-Cmd ("gh pr view {0} --json url -q .url" -f $meta.branch_name)
        if ($viewR.ExitCode -eq 0) { $existing = $viewR.Output.Trim() }

        if (-not $existing) {
          $safeTitle = ($meta.pr_title -replace '"', "'")
          $body = @"
Summary:
- $safeTitle

How to test:
- npm run lint
- cd src-tauri && cargo fmt --check
- cd src-tauri && cargo test

Notes:
- Research: .codex-logs/research-$CycleId.md
- Evidence: .codex-logs/evidence-$CycleId.md
- Review: .codex-logs/review-$CycleId.md
- Self-healing retries: $retryCount/$MaxRetries
- Constraints: max $MaxDiffLines diff lines, max $MaxFilesChanged files
"@
          $tmpBody = Join-Path $LogDir "pr-body-$CycleId.txt"
          Write-Utf8File $tmpBody $body

          $cmd = "gh pr create --base main --head {0} --title {1} --body-file {2} {3}" -f `
            $meta.branch_name, (Quote-CmdArg $safeTitle), (Quote-CmdArg $tmpBody), $draftFlag

          $created = Invoke-Cmd $cmd
          if ($created.ExitCode -ne 0) {
            throw "[gh-pr-create] failed (exit=$($created.ExitCode)).`n$($created.Output)"
          }

          $m = [regex]::Match($created.Output, 'https://github\.com/\S+/pull/\d+')
          if ($m.Success) { $prUrl = $m.Value }
        } else {
          $prUrl = $existing
        }

        if ($prUrl) {
          $CreatedPRs.Add($prUrl) | Out-Null
          Append-Utf8File $ResultFile "`n`nPR: $prUrl`n"
        }
      }
    }

    Send-Notification -Title "STEP D Complete" -Message "PR: $prUrl`nBranch: ``$($meta.branch_name)``" -Color "3066993"

    # Return to RepoRoot BEFORE STEP E (worktree -> main repo)
    Pop-Location
    Set-Location -LiteralPath $RepoRoot

    # =====================
    # STEP E: Auto-merge (~90-95%) — FIXED: runs from RepoRoot
    # =====================
    if ($prUrl -and $AutoCreatePR -and $ghOk) {
      Write-Progress-Step -Step "E" -Message "Auto-merging PR into main..." -Percent "~93%" -Cycle $i -MaxCycles $MaxPRs -LoopStart $LoopStartTime

      $prNumMatch = [regex]::Match($prUrl, '/pull/(\d+)')
      if ($prNumMatch.Success) {
        $prNum = $prNumMatch.Groups[1].Value

        if ($DraftPR) {
          Write-Host "[E] Marking PR #$prNum as ready..." -ForegroundColor DarkGreen
          $readyR = Invoke-Cmd "gh pr ready $prNum"
          if ($readyR.ExitCode -ne 0) {
            Write-Host "[E][warn] gh pr ready failed: $($readyR.Output)" -ForegroundColor Yellow
          }
          Start-Sleep -Seconds 3
        }

        Write-Host "[E] Squash-merging PR #$prNum..." -ForegroundColor DarkGreen
        $mergeR = Invoke-Cmd "gh pr merge $prNum --squash --delete-branch"
        if ($mergeR.ExitCode -eq 0) {
          Write-Host "[E] PR #$prNum merged successfully." -ForegroundColor Green
          Append-Utf8File $ResultFile "`nMerge: PR #$prNum squash-merged into main.`n"
          Send-Notification -Title "PR #$prNum Merged" -Message "**$($meta.pr_title)**`nSquash-merged into main." -Color "3066993"
        } else {
          Write-Host "[E][error] Merge failed: $($mergeR.Output)" -ForegroundColor Red
          Append-Utf8File $ResultFile "`nMerge: FAILED for PR #$prNum - $($mergeR.Output)`n"
          Send-Notification -Title "PR #$prNum Merge FAILED" -Message "Error: $($mergeR.Output)" -Color "15158332"
        }

        Start-Sleep -Seconds 3
      } else {
        Write-Host "[E][warn] Could not extract PR number from: $prUrl" -ForegroundColor Yellow
      }
    }

    # Show result
    $cycleElapsed = ((Get-Date) - $cycleStartTime).ToString("hh\:mm\:ss")
    Write-Host ""
    Write-Host "--- Cycle $i COMPLETE (took: $cycleElapsed) ---" -ForegroundColor Green
    Write-Host "Result ($CycleId):" -ForegroundColor Cyan
    Get-Content $ResultFile -Encoding utf8

    Send-Notification -Title "Cycle $i/$MaxPRs Complete" -Message "Duration: $cycleElapsed`nPR: $($meta.pr_title)`nURL: $prUrl" -Color "3066993"

  }
  finally {
    # Ensure we're back at RepoRoot (may already be there after STEP E)
    try { Pop-Location } catch {}
    Set-Location -LiteralPath $RepoRoot

    if (-not $KeepWorktrees) {
      try {
        $rmCmd = "git worktree remove " + (Quote-CmdArg $WtPath) + " --force"
        Invoke-Cmd $rmCmd | Out-Null
      } catch {}
      try { if (Test-Path $WtPath) { Remove-Item -Recurse -Force $WtPath } } catch {}
    }
  }

  if ($i -lt $MaxPRs) {
    Write-Host "Sleeping $SleepMinutes minutes..." -ForegroundColor Gray
    Start-Sleep -Seconds ($SleepMinutes * 60)
  }
}

$totalElapsed = ((Get-Date) - $LoopStartTime).ToString("hh\:mm\:ss")
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " auto-dev loop COMPLETE" -ForegroundColor Cyan
Write-Host " Finished : $(Get-Date)" -ForegroundColor Cyan
Write-Host " Total    : $totalElapsed" -ForegroundColor Cyan
Write-Host " PRs done : $($CreatedPRs.Count)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Send-Notification -Title "Loop COMPLETE" -Message "All $MaxPRs cycles finished.`nTotal: $totalElapsed`nPRs: $($CreatedPRs.Count)`nURLs:`n$($CreatedPRs -join "`n")" -Color "3066993"

# -------------------------
# POST-LOOP: Night report + next plan + memory consolidation
# -------------------------
Write-Host ""
Write-Host "========================================" -ForegroundColor Magenta
Write-Host " Generating night report and next plan..." -ForegroundColor Magenta
Write-Host "========================================" -ForegroundColor Magenta

$ReportFile    = Join-Path $LogDir "night-report-$Timestamp.md"
$NextPlanFile  = Join-Path $LogDir "next-session-plan-$Timestamp.md"
$PatternsFile  = Join-Path $LogDir "memory\patterns.md"
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
$MgmtEvidence += (Run-CmdChecked "mgmt-git-log" "git log --oneline -20")

$MgmtEvidence += "## gh pr list (last 100)"
if ($ghOk) {
  $prListR = Invoke-Cmd "gh pr list --state all --limit 100"
  if ($prListR.ExitCode -eq 0) { $MgmtEvidence += $prListR.Output } else { $MgmtEvidence += "[gh pr list failed]`n$($prListR.Output)" }
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
- All code reviews: $LogDir\review-*.md
- All research reports: $LogDir\research-*.md
- All cycle lessons: $LogDir\memory\lessons\*.md
- Tasks: $RepoRoot\tasks\todo.md (if exists), $RepoRoot\tasks\lessons.md (if exists)
- Repo evidence: $MgmtEvidencePath
- PR URLs file: $PRListFile
- AGENTS: $AgentsPath
- Plan: $PlanPath

Produce THREE files:

(1) Night report saved to:
$ReportFile

Must include:
- PR-by-PR status (merged/open/local-only)
- Failures / incomplete work
- Code review findings summary
- Self-healing retry statistics
- Constraint compliance summary (diff lines, files changed)
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

(3) Recurring patterns update saved to:
$PatternsFile

Consolidate ALL cycle lessons into a categorized pattern list:
- Category: Build/Test patterns
- Category: Code quality patterns
- Category: Scope management patterns
- Category: Tool/environment patterns
Format each pattern as: | pattern | frequency | prevention rule |
This file is the long-term memory that future cycles will read.
"@

$StepReportLog = Join-Path $LogDir "step-report-$Timestamp.log"
Invoke-CodexReport -Prompt $ReportPrompt -LogFile $StepReportLog

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
Write-Host " Memory: $MemDir" -ForegroundColor Magenta
Write-Host "========================================" -ForegroundColor Magenta

Send-Notification -Title "Night Report Generated" -Message "Report: night-report-$Timestamp.md`nNext plan: next-session-plan-$Timestamp.md`nPatterns: memory/patterns.md`nLogs: $LogDir" -Color "3447003"
