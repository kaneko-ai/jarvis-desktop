param(
  [string]$OutPath = "",
  [string]$PipelineRoot = "",
  [switch]$NoRedact
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

function Sanitize-PathCandidate {
  param([string]$Raw)
  if ([string]::IsNullOrWhiteSpace($Raw)) { return $null }
  $t = $Raw.Trim().Trim('"').Trim("'")
  if ($t -match '^(?i)https?://vscodecontentref/') { return $null }
  return $t
}

function Resolve-PathSafe {
  param([string]$Raw)
  $sanitized = Sanitize-PathCandidate -Raw $Raw
  if ([string]::IsNullOrWhiteSpace($sanitized)) { return $null }
  if (-not (Test-Path $sanitized)) { return $null }
  try { return (Resolve-Path $sanitized).Path } catch { return $null }
}

function Find-RepoRoot {
  param([string]$Start)
  $current = Resolve-PathSafe -Raw $Start
  if (-not $current) { return $null }

  for ($i = 0; $i -lt 10; $i++) {
    $hasPkg = Test-Path (Join-Path $current "package.json")
    $hasTauri = Test-Path (Join-Path $current "src-tauri")
    if ($hasPkg -and $hasTauri) { return $current }

    $parent = Split-Path $current -Parent
    if ([string]::IsNullOrWhiteSpace($parent) -or $parent -eq $current) { break }
    $current = $parent
  }
  return $null
}

function Run-Cmd {
  param(
    [string]$Name,
    [string]$Exe,
    [string[]]$Args = @(),
    [int]$TimeoutSec = 8
  )

  $result = [ordered]@{
    Name = $Name
    Exe = $Exe
    Exists = $false
    ExitCode = $null
    Output = ""
  }

  try {
    $cmd = Get-Command $Exe -ErrorAction Stop
    $result.Exists = $true

    $exePath = $cmd.Source
    if ($Exe -eq "npm") {
      $npmCmd = Join-Path (Split-Path $exePath -Parent) "npm.cmd"
      if (Test-Path $npmCmd) {
        $exePath = $npmCmd
      }
    }

    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
      $job = Start-Job -ScriptBlock {
        param($pExe, $pArgs)
        $ErrorActionPreference = "Continue"
        $output = & $pExe @pArgs 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
        if ($null -eq $exitCode) { $exitCode = 0 }
        [pscustomobject]@{
          Output = $output.Trim()
          ExitCode = $exitCode
        }
      } -ArgumentList $exePath, $Args

      if (Wait-Job -Job $job -Timeout $TimeoutSec) {
        $received = Receive-Job -Job $job -ErrorAction SilentlyContinue
        if ($received) {
          $result.Output = [string]$received.Output
          $result.ExitCode = [int]$received.ExitCode
        } else {
          $result.Output = "ERROR: no output returned"
          $result.ExitCode = 125
        }
      } else {
        Stop-Job -Job $job -Force -ErrorAction SilentlyContinue | Out-Null
        $result.Output = "ERROR: timeout after ${TimeoutSec}s"
        $result.ExitCode = 124
      }

      Remove-Job -Job $job -Force -ErrorAction SilentlyContinue | Out-Null
    }
    finally {
      $ErrorActionPreference = $prev
    }
  }
  catch {
    $result.Output = "ERROR: $($_.Exception.Message)"
    $result.ExitCode = 127
  }

  return [pscustomobject]$result
}

function Mask-Secret {
  param([object]$Value)
  if ($null -eq $Value) { return $null }
  $s = [string]$Value
  if ([string]::IsNullOrWhiteSpace($s)) { return "" }
  return "********"
}

function To-ReportPath {
  param([string]$MaybePath, [string]$RepoRoot)
  if ([string]::IsNullOrWhiteSpace($MaybePath)) {
    return (Join-Path $RepoRoot "diag_report.md")
  }

  $sanitized = Sanitize-PathCandidate -Raw $MaybePath
  if ([string]::IsNullOrWhiteSpace($sanitized)) {
    return (Join-Path $RepoRoot "diag_report.md")
  }

  if ([System.IO.Path]::IsPathRooted($sanitized)) {
    return $sanitized
  }
  return (Join-Path $RepoRoot $sanitized)
}

$diagnostic = [ordered]@{
  generated_at = (Get-Date).ToString("yyyy-MM-dd HH:mm:ss")
  script_root = $ScriptRoot
  repo_root = $null
  out_path = $null
  status = "ok"
  errors = @()
  notes = @()
  env = [ordered]@{
    JARVIS_PIPELINE_ROOT = $null
    JARVIS_PIPELINE_OUT_DIR = $null
    S2_API_KEY = "********"
  }
  commands = @()
  config = [ordered]@{
    path = $null
    exists = $false
    sanitized = $null
  }
  latest_run = [ordered]@{
    dir = $null
    input_json = $false
    result_json = $false
    tree_md = $false
  }
}

$repoRoot = $null
$outPathResolved = $null

try {
  $repoRoot = Find-RepoRoot -Start $ScriptRoot
  if (-not $repoRoot) {
    throw "Repo root not found from script location. Expected package.json and src-tauri in same directory."
  }
  $diagnostic.repo_root = $repoRoot

  $outPathResolved = To-ReportPath -MaybePath $OutPath -RepoRoot $repoRoot
  $diagnostic.out_path = $outPathResolved

  $configPath = Join-Path $env:APPDATA "jarvis-desktop\config.json"
  $diagnostic.config.path = $configPath
  $diagnostic.config.exists = Test-Path $configPath

  $configObj = $null
  if ($diagnostic.config.exists) {
    try {
      $configObj = Get-Content -Path $configPath -Raw | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
      $diagnostic.notes += "config parse failed: $($_.Exception.Message)"
    }
  }

  $pipelineFromArg = Resolve-PathSafe -Raw $PipelineRoot
  $pipelineFromEnv = Resolve-PathSafe -Raw $env:JARVIS_PIPELINE_ROOT
  $pipelineFromCfg = $null
  if ($configObj -and $configObj.PSObject.Properties.Name -contains "JARVIS_PIPELINE_ROOT") {
    $pipelineFromCfg = Resolve-PathSafe -Raw ([string]$configObj.JARVIS_PIPELINE_ROOT)
  }

  $pipelineResolved = $pipelineFromArg
  if (-not $pipelineResolved) { $pipelineResolved = $pipelineFromEnv }
  if (-not $pipelineResolved) { $pipelineResolved = $pipelineFromCfg }
  if (-not $pipelineResolved) {
    $autoPipeline = Resolve-PathSafe -Raw (Join-Path (Split-Path $repoRoot -Parent) "jarvis-ml-pipeline")
    if ($autoPipeline) {
      $pipelineResolved = $autoPipeline
      $diagnostic.notes += "pipeline root auto-detected from sibling folder"
    }
  }

  $diagnostic.env.JARVIS_PIPELINE_ROOT = $pipelineResolved
  $diagnostic.env.JARVIS_PIPELINE_OUT_DIR = $null
  $diagnostic.env.S2_API_KEY = Mask-Secret -Value $env:S2_API_KEY

  $cfgOutRaw = $null
  if ($configObj -and $configObj.PSObject.Properties.Name -contains "JARVIS_PIPELINE_OUT_DIR") {
    $cfgOutRaw = Sanitize-PathCandidate -Raw ([string]$configObj.JARVIS_PIPELINE_OUT_DIR)
  }

  $envOutRaw = Sanitize-PathCandidate -Raw $env:JARVIS_PIPELINE_OUT_DIR
  $selectedOutRaw = $cfgOutRaw
  if (-not $selectedOutRaw) { $selectedOutRaw = $envOutRaw }

  $outDirResolved = $null
  if ($selectedOutRaw) {
    if ([System.IO.Path]::IsPathRooted($selectedOutRaw)) {
      $outDirResolved = Resolve-PathSafe -Raw $selectedOutRaw
    }
    elseif ($pipelineResolved) {
      $outDirResolved = Resolve-PathSafe -Raw (Join-Path $pipelineResolved $selectedOutRaw)
    }
  }
  if (-not $outDirResolved -and $pipelineResolved) {
    $outDirResolved = Resolve-PathSafe -Raw (Join-Path $pipelineResolved "logs\runs")
  }

  $diagnostic.env.JARVIS_PIPELINE_OUT_DIR = $outDirResolved
  if (-not $pipelineResolved) {
    $diagnostic.notes += "pipeline root is not resolved"
  }

  if ($configObj) {
    $cfgSafe = [ordered]@{}
    foreach ($p in $configObj.PSObject.Properties) {
      if ($p.Name -eq "S2_API_KEY") {
        $cfgSafe[$p.Name] = (Mask-Secret -Value $p.Value)
      }
      else {
        $cfgSafe[$p.Name] = $p.Value
      }
    }
    $diagnostic.config.sanitized = $cfgSafe
  }

  $cmdSpecs = @(
    @{ Name = "python"; Exe = "python"; Args = @("--version") },
    @{ Name = "node"; Exe = "node"; Args = @("-v") },
    @{ Name = "npm"; Exe = "npm"; Args = @("-v") },
    @{ Name = "cargo"; Exe = "cargo"; Args = @("-V") },
    @{ Name = "rustc"; Exe = "rustc"; Args = @("-V") },
    @{ Name = "git"; Exe = "git"; Args = @("--version") }
  )

  foreach ($spec in $cmdSpecs) {
    $diagnostic.commands += (Run-Cmd -Name $spec.Name -Exe $spec.Exe -Args $spec.Args)
  }

  $latestRunDir = $null
  if ($outDirResolved -and (Test-Path $outDirResolved)) {
    $latest = Get-ChildItem -Path $outDirResolved -Directory -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1
    if ($latest) {
      $latestRunDir = $latest.FullName
    }
  }

  $diagnostic.latest_run.dir = $latestRunDir
  if ($latestRunDir) {
    $diagnostic.latest_run.input_json = Test-Path (Join-Path $latestRunDir "input.json")
    $diagnostic.latest_run.result_json = Test-Path (Join-Path $latestRunDir "result.json")
    $diagnostic.latest_run.tree_md = Test-Path (Join-Path $latestRunDir "paper_graph\tree\tree.md")
  }
  else {
    $diagnostic.latest_run.input_json = $false
    $diagnostic.latest_run.result_json = $false
    $diagnostic.latest_run.tree_md = $false
    $diagnostic.notes += "latest run directory was not found"
  }
}
catch {
  $diagnostic.status = "error"
  $diagnostic.errors += $_.Exception.Message
}
finally {
  if (-not $repoRoot) {
    $repoRoot = Resolve-PathSafe -Raw $ScriptRoot
    if (-not $repoRoot) { $repoRoot = (Get-Location).Path }
  }
  if (-not $outPathResolved) {
    $outPathResolved = To-ReportPath -MaybePath $OutPath -RepoRoot $repoRoot
  }

  $outDirParent = Split-Path -Parent $outPathResolved
  if (-not [string]::IsNullOrWhiteSpace($outDirParent)) {
    New-Item -ItemType Directory -Path $outDirParent -Force | Out-Null
  }

  $cmdLines = @()
  foreach ($c in $diagnostic.commands) {
    $cmdLines += "- $($c.Name): exists=$($c.Exists) exit=$($c.ExitCode)"
    if (-not [string]::IsNullOrWhiteSpace($c.Output)) {
      $cmdLines += "  - output: $($c.Output -replace "`r?`n", " | ")"
    }
  }
  if ($cmdLines.Count -eq 0) { $cmdLines += "- no command checks executed" }

  $cfgJson = "{}"
  if ($diagnostic.config.Contains("sanitized")) {
    $cfgJson = ($diagnostic.config.sanitized | ConvertTo-Json -Depth 6)
  }

  $errLines = @()
  foreach ($e in $diagnostic.errors) { $errLines += "- $e" }
  if ($errLines.Count -eq 0) { $errLines += "- none" }

  $noteLines = @()
  foreach ($n in $diagnostic.notes) { $noteLines += "- $n" }
  if ($noteLines.Count -eq 0) { $noteLines += "- none" }

  $report = @"
# Desktop Diagnostic Report

- status: $($diagnostic.status)
- generated_at: $($diagnostic.generated_at)
- script_root: $($diagnostic.script_root)
- repo_root: $($diagnostic.repo_root)
- report_path: $outPathResolved

## Environment Resolution

- JARVIS_PIPELINE_ROOT: $($diagnostic.env.JARVIS_PIPELINE_ROOT)
- JARVIS_PIPELINE_OUT_DIR: $($diagnostic.env.JARVIS_PIPELINE_OUT_DIR)
- S2_API_KEY(env): $($diagnostic.env.S2_API_KEY)

## Command Checks
$($cmdLines -join "`r`n")

## Config

- path: $($diagnostic.config.path)
- exists: $($diagnostic.config.exists)

~~~json
$cfgJson
~~~

## Latest Run

- run_dir: $($diagnostic.latest_run.dir)
- input.json: $($diagnostic.latest_run.input_json)
- result.json: $($diagnostic.latest_run.result_json)
- paper_graph/tree/tree.md: $($diagnostic.latest_run.tree_md)

## Notes
$($noteLines -join "`r`n")

## Errors
$($errLines -join "`r`n")
"@

  if ($PSVersionTable.PSVersion.Major -ge 6) {
    $report | Out-File -FilePath $outPathResolved -Encoding utf8NoBOM
  } else {
    $report | Out-File -FilePath $outPathResolved -Encoding utf8
  }

  Write-Host "Diagnostic report generated: $outPathResolved"
  if ($diagnostic.status -ne "ok") {
    throw "collect_diag completed with errors. See report: $outPathResolved"
  }
  $global:LASTEXITCODE = 0
}
