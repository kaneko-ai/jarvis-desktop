param(
  [string]$DesktopRoot = "",
  [string]$PipelineRoot = "",
  [string]$OutFile = "diag_report.md"
)

$ErrorActionPreference = "Stop"

function Get-CmdVersion([string]$CommandName, [string[]]$Args) {
  $cmd = Get-Command $CommandName -ErrorAction SilentlyContinue
  if (-not $cmd) { return "NOT FOUND" }
  try {
    return ((& $CommandName @Args 2>&1 | Out-String).Trim())
  } catch {
    return "FAILED: $($_.Exception.Message)"
  }
}

function Mask-Secret([object]$Value) {
  if ($null -eq $Value) { return $null }
  $s = [string]$Value
  if ([string]::IsNullOrWhiteSpace($s)) { return "" }
  return "***REDACTED***"
}

function Resolve-ExistingPath([string]$RawPath) {
  if ([string]::IsNullOrWhiteSpace($RawPath)) { return $null }
  if (Test-Path $RawPath) { return (Resolve-Path $RawPath).Path }
  return $null
}

try {
  $desktopRootResolved = if (-not [string]::IsNullOrWhiteSpace($DesktopRoot)) {
    Resolve-ExistingPath $DesktopRoot
  } else {
    Resolve-Path (Join-Path $PSScriptRoot "..") | ForEach-Object { $_.Path }
  }

  if (-not $desktopRootResolved) {
    throw "Desktop root could not be resolved."
  }

  $configPath = Join-Path $env:APPDATA "jarvis-desktop\config.json"
  $configObj = $null
  $configReadError = $null
  if (Test-Path $configPath) {
    try {
      $configObj = (Get-Content -Raw -Path $configPath | ConvertFrom-Json)
    } catch {
      $configReadError = $_.Exception.Message
    }
  }

  $pipelineRootResolved = Resolve-ExistingPath $PipelineRoot
  if (-not $pipelineRootResolved -and $configObj) {
    $pipelineRootResolved = Resolve-ExistingPath ([string]$configObj.JARVIS_PIPELINE_ROOT)
  }
  if (-not $pipelineRootResolved) {
    $pipelineRootResolved = Resolve-ExistingPath $env:JARVIS_PIPELINE_ROOT
  }

  $outDirRaw = $null
  if ($configObj -and $configObj.JARVIS_PIPELINE_OUT_DIR) {
    $outDirRaw = [string]$configObj.JARVIS_PIPELINE_OUT_DIR
  } elseif ($env:JARVIS_PIPELINE_OUT_DIR) {
    $outDirRaw = [string]$env:JARVIS_PIPELINE_OUT_DIR
  }

  $outDirResolved = $null
  if (-not [string]::IsNullOrWhiteSpace($outDirRaw)) {
    if ([System.IO.Path]::IsPathRooted($outDirRaw)) {
      $outDirResolved = Resolve-ExistingPath $outDirRaw
    } elseif ($pipelineRootResolved) {
      $outDirResolved = Resolve-ExistingPath (Join-Path $pipelineRootResolved $outDirRaw)
    }
  }
  if (-not $outDirResolved -and $pipelineRootResolved) {
    $outDirResolved = Resolve-ExistingPath (Join-Path $pipelineRootResolved "logs\runs")
  }

  $latestRunDir = $null
  if ($outDirResolved) {
    $latest = Get-ChildItem -Directory -Path $outDirResolved -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1
    if ($latest) { $latestRunDir = $latest.FullName }
  }

  $artifactChecks = @()
  if ($latestRunDir) {
    $artifactChecks += [PSCustomObject]@{ Name = "input.json"; Exists = (Test-Path (Join-Path $latestRunDir "input.json")) }
    $artifactChecks += [PSCustomObject]@{ Name = "result.json"; Exists = (Test-Path (Join-Path $latestRunDir "result.json")) }
    $artifactChecks += [PSCustomObject]@{ Name = "paper_graph/tree/tree.md"; Exists = (Test-Path (Join-Path $latestRunDir "paper_graph\tree\tree.md")) }
  }

  $cfgSanitized = $null
  if ($configObj) {
    $cfgSanitized = [ordered]@{}
    foreach ($p in $configObj.PSObject.Properties) {
      if ($p.Name -eq "S2_API_KEY") {
        $cfgSanitized[$p.Name] = Mask-Secret $p.Value
      } else {
        $cfgSanitized[$p.Name] = $p.Value
      }
    }
  }

  $nodeVer = Get-CmdVersion "node" @("--version")
  $npmVer = Get-CmdVersion "npm" @("--version")
  $pythonVer = Get-CmdVersion "python" @("--version")
  $cargoVer = Get-CmdVersion "cargo" @("-V")
  $rustcVer = Get-CmdVersion "rustc" @("-V")
  $generatedAt = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

  $lines = @()
  $lines += "# Desktop Diagnostic Report"
  $lines += ""
  $lines += "- Generated: $generatedAt"
  $lines += "- Desktop root: $desktopRootResolved"
  $lines += "- Pipeline root (resolved): $pipelineRootResolved"
  $lines += "- Out dir (resolved): $outDirResolved"
  $lines += ""
  $lines += "## Environment"
  $lines += "- OS: $([System.Environment]::OSVersion.VersionString)"
  $lines += "- node: $nodeVer"
  $lines += "- npm: $npmVer"
  $lines += "- python: $pythonVer"
  $lines += "- cargo: $cargoVer"
  $lines += "- rustc: $rustcVer"
  $lines += ""
  $lines += "## Config"
  $lines += "- config path: $configPath"
  $lines += "- exists: $(Test-Path $configPath)"
  if ($configReadError) {
    $lines += "- parse error: $configReadError"
  }
  if ($cfgSanitized) {
    $lines += ""
    $lines += '```json'
    $lines += (($cfgSanitized | ConvertTo-Json -Depth 5) -replace '\r?\n$', "")
    $lines += '```'
  }

  $lines += ""
  $lines += "## Latest Run"
  $lines += "- latest run dir: $latestRunDir"
  if ($artifactChecks.Count -gt 0) {
    foreach ($a in $artifactChecks) {
      $lines += "- $($a.Name): $($a.Exists)"
    }
  } else {
    $lines += "- artifacts: latest run not found"
  }

  $outPath = if ([System.IO.Path]::IsPathRooted($OutFile)) {
    $OutFile
  } else {
    Join-Path $desktopRootResolved $OutFile
  }

  $lines -join "`r`n" | Set-Content -Encoding UTF8 -Path $outPath
  Write-Host "Diagnostic report generated: $outPath"
}
catch {
  Write-Error "collect_diag.ps1 failed: $($_.Exception.Message)"
  exit 1
}
