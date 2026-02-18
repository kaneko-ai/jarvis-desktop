param(
  [string]$PipelineRoot = "..\..\jarvis-ml-pipeline",
  [string]$OutDir = "",
  [int]$StartupWaitSec = 20,
  [switch]$RunDiag,
  [switch]$RunDiagStrict
)

$ErrorActionPreference = "Stop"
$script:tauriProc = $null
$script:exitCode = 1
$script:summary = @{}

function Assert-LastExitCode([string]$label) {
  if ($LASTEXITCODE -ne 0) {
    throw "$label failed with exit code $LASTEXITCODE"
  }
}

function Resolve-CargoPath {
  # Prefer existing PATH, then fallback to rustup default location.
  $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
  if ($cargoCmd) { return $true }

  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  $cargoExe = Join-Path $cargoBin "cargo.exe"
  if (Test-Path $cargoExe) {
    $env:Path = "$cargoBin;$env:Path"
  }
  return [bool](Get-Command cargo -ErrorAction SilentlyContinue)
}

try {
  $desktopRoot = (Resolve-Path (Split-Path -Parent $MyInvocation.MyCommand.Path)).Path
  Set-Location $desktopRoot

  Write-Host "=== JARVIS Desktop Smoke (E2E) ==="
  Write-Host "Desktop root: $desktopRoot"

  if (-not (Test-Path (Join-Path $desktopRoot "package.json"))) {
    throw "RULE_DESKTOP_ROOT_INVALID: package.json not found in desktop root."
  }
  if (-not (Test-Path (Join-Path $desktopRoot "src-tauri"))) {
    throw "RULE_DESKTOP_ROOT_INVALID: src-tauri directory not found in desktop root."
  }

  $resolvedPipeline = (Resolve-Path -Path $PipelineRoot).Path
  $env:JARVIS_PIPELINE_ROOT = $resolvedPipeline

  if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $resolvedOutDir = Join-Path $resolvedPipeline "logs\runs"
  } else {
    if ([System.IO.Path]::IsPathRooted($OutDir)) {
      $candidateOut = $OutDir
    } else {
      $candidateOut = Join-Path $resolvedPipeline $OutDir
    }
    $resolvedOutDir = $candidateOut
  }
  $env:JARVIS_PIPELINE_OUT_DIR = $resolvedOutDir

  Write-Host "JARVIS_PIPELINE_ROOT=$($env:JARVIS_PIPELINE_ROOT)"
  Write-Host "JARVIS_PIPELINE_OUT_DIR=$($env:JARVIS_PIPELINE_OUT_DIR)"

  if (-not (Test-Path (Join-Path $desktopRoot "node_modules"))) {
    Write-Host "`n[1/8] node_modules missing -> npm ci"
    npm ci
    Assert-LastExitCode "npm ci"
  } else {
    Write-Host "`n[1/8] node_modules exists -> skip npm ci"
  }

  Write-Host "[2/8] npm run build"
  npm run build
  Assert-LastExitCode "npm run build"

  Write-Host "[3/8] cargo/rustc check"
  if (-not (Resolve-CargoPath)) {
    throw "cargo is not found. Install Rust/Cargo and ensure PATH is updated."
  }
  cargo -V
  Assert-LastExitCode "cargo -V"
  rustc -V
  Assert-LastExitCode "rustc -V"

  Write-Host "[4/8] start npx tauri dev in background"
  $ts = Get-Date -Format "yyyyMMdd_HHmmss"
  $stdoutLog = Join-Path $desktopRoot "tauri-dev-smoke.$ts.stdout.log"
  $stderrLog = Join-Path $desktopRoot "tauri-dev-smoke.$ts.stderr.log"

  $script:tauriProc = Start-Process `
    -FilePath "npx.cmd" `
    -ArgumentList @("tauri", "dev") `
    -WorkingDirectory $desktopRoot `
    -PassThru `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError $stderrLog

  Start-Sleep -Seconds $StartupWaitSec
  $script:tauriProc.Refresh()
  if ($script:tauriProc.HasExited) {
    Write-Host "`n--- tauri stdout tail ---"
    if (Test-Path $stdoutLog) { Get-Content $stdoutLog -Tail 80 }
    Write-Host "`n--- tauri stderr tail ---"
    if (Test-Path $stderrLog) { Get-Content $stderrLog -Tail 80 }
    throw "tauri dev exited early (exit=$($script:tauriProc.ExitCode))"
  }
  Write-Host "tauri dev running (PID=$($script:tauriProc.Id))"

  Write-Host "[5/8] run desktop backend pipeline path and verify run artifacts"
  New-Item -Path $resolvedOutDir -ItemType Directory -Force | Out-Null

  $beforeRuns = @{}
  Get-ChildItem -Path $resolvedOutDir -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $beforeRuns[$_.FullName.ToLowerInvariant()] = $true
  }

  $runDir = $null
  $runId = $null
  $lastOutputText = ""
  $maxAttempts = 3
  # Retry transient API failures (429/timeout/needs_retry) but still fail hard on persistent issues.
  for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
    Write-Host "  - pipeline attempt $attempt/$maxAttempts"

    $pipelineArgs = @(
      "run", "-q",
      "--manifest-path", (Join-Path $desktopRoot "src-tauri\Cargo.toml"),
      "--",
      "--smoke-run-template-tree",
      "arxiv:1706.03762",
      "1",
      "5"
    )
    $prevErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    Push-Location $desktopRoot
    try {
      $cliOutput = & cargo @pipelineArgs 2>&1
      $cliExitCode = $LASTEXITCODE
    } finally {
      Pop-Location
      $ErrorActionPreference = $prevErrorAction
    }

    $combined = (($cliOutput | ForEach-Object { "$_" }) -join "`n").Trim()
    $lastOutputText = $combined

    $runDirFromOutput = $null
    $combinedLines = @($combined -split "`r?`n")
    $jsonLine = $combinedLines | Where-Object { $_.Trim().StartsWith("{") -and $_.Trim().EndsWith("}") } | Select-Object -Last 1
    if ($jsonLine) {
      try {
        $runRes = $jsonLine | ConvertFrom-Json
        if ($runRes.run_id) {
          $runId = [string]$runRes.run_id
        }
        if ($runRes.run_dir) {
          $candidate = [string]$runRes.run_dir
          if ([System.IO.Path]::IsPathRooted($candidate)) {
            $runDirFromOutput = $candidate
          } else {
            $runDirFromOutput = Join-Path $resolvedOutDir (Split-Path $candidate -Leaf)
          }
        }
      } catch {
        # fall through to directory inference
      }
    }

    $afterDirs = Get-ChildItem -Path $resolvedOutDir -Directory -ErrorAction SilentlyContinue
    $newDirs = @($afterDirs | Where-Object { -not $beforeRuns.ContainsKey($_.FullName.ToLowerInvariant()) })

    if ($runDirFromOutput -and (Test-Path $runDirFromOutput)) {
      $runDir = (Resolve-Path $runDirFromOutput).Path
    } elseif ($newDirs.Count -gt 0) {
      $runDir = ($newDirs | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
    }

    $retryHint = ($combined -match "(?i)429|needs_retry|timeout")
    if ($cliExitCode -ne 0) {
      if ($retryHint -and $attempt -lt $maxAttempts) {
        Start-Sleep -Seconds (5 * $attempt)
        continue
      }
      throw "pipeline CLI failed with exit code $cliExitCode`n$combined"
    }

    if ($runDir -and (Test-Path $runDir)) {
      $requiredCheck = @(
        (Join-Path $runDir "input.json"),
        (Join-Path $runDir "result.json"),
        (Join-Path $runDir "paper_graph\tree\tree.md")
      )
      $allPresent = $true
      foreach ($p in $requiredCheck) {
        if (-not (Test-Path $p)) {
          $allPresent = $false
          break
        }
      }
      if ($allPresent) { break }
    }

    if ($attempt -lt $maxAttempts) {
      Start-Sleep -Seconds (5 * $attempt)
    }
  }

  if (-not $runDir -or -not (Test-Path $runDir)) {
    throw "No new run directory detected under $resolvedOutDir`n$lastOutputText"
  }

  Write-Host "[6/9] verify run listing includes created run_id"
  $runIdFromDir = if ($runId) { $runId } else { Split-Path $runDir -Leaf }
  $listedRunIds = Get-ChildItem -Path $resolvedOutDir -Directory -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -ExpandProperty Name
  if (-not ($listedRunIds -contains $runIdFromDir)) {
    throw "run listing does not include created run_id: $runIdFromDir"
  }

  Write-Host "[7/10] verify required artifacts in $runDir"
  $required = @(
    "input.json",
    "result.json",
    "paper_graph\tree\tree.md"
  )
  foreach ($rel in $required) {
    $p = Join-Path $runDir $rel
    if (-not (Test-Path $p)) {
      throw "Missing artifact: $p"
    }
  }

  $script:summary = @{
    TauriPid = $script:tauriProc.Id
    OutDir = $resolvedOutDir
    RunDir = $runDir
    TauriStdoutLog = $stdoutLog
    TauriStderrLog = $stderrLog
  }

  Write-Host "[8/10] verify input.json desktop metadata contract"
  $inputJsonPath = Join-Path $runDir "input.json"
  $inputObj = Get-Content $inputJsonPath -Raw | ConvertFrom-Json
  $hasDesktopContract = $false
  if ($inputObj.PSObject.Properties.Name -contains "desktop") {
    $d = $inputObj.desktop
    if ($d -and $d.PSObject.Properties.Name -contains "template_id" -and $d.PSObject.Properties.Name -contains "canonical_id") {
      if (-not [string]::IsNullOrWhiteSpace([string]$d.template_id) -and -not [string]::IsNullOrWhiteSpace([string]$d.canonical_id)) {
        $hasDesktopContract = $true
      }
    }
  }

  if (-not $hasDesktopContract) {
    throw "Desktop metadata missing in input.json (CLI sample path)."
  }

  Write-Host "[9/10] pipeline artifact verification passed"
  
    if ($RunDiag) {
      Write-Host "[10/10] optional diagnostics: collect_diag.ps1"
      $diagScript = Join-Path $desktopRoot "scripts\collect_diag.ps1"
      if (Test-Path $diagScript) {
        $diagPath = Join-Path $desktopRoot "diag_report.md"
        Write-Host ("Diagnostic command: powershell.exe -NoProfile -ExecutionPolicy Bypass -File " + $diagScript + " -PipelineRoot " + $resolvedPipeline + " -OutPath " + $diagPath)
        $prevErrorAction = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
          $diagArgs = @(
            "-NoProfile",
            "-ExecutionPolicy", "Bypass",
            "-File", $diagScript,
            "-PipelineRoot", $resolvedPipeline,
            "-OutPath", $diagPath
          )
          & powershell.exe @diagArgs
          $diagExit = $LASTEXITCODE
        } finally {
          $ErrorActionPreference = $prevErrorAction
        }
        if ($diagExit -ne 0 -or -not (Test-Path $diagPath)) {
          $diagMsg = "diagnostics failed or report missing (exit=$diagExit)"
          Write-Host "WARNING: $diagMsg" -ForegroundColor Yellow
          if ($RunDiagStrict) {
            throw "RunDiagStrict: $diagMsg"
          }
        } else {
          Write-Host "Diagnostics report generated: $diagPath"
        }
      } else {
        $diagMsg = "diagnostics script not found: $diagScript"
        Write-Host "WARNING: $diagMsg" -ForegroundColor Yellow
        if ($RunDiagStrict) {
          throw "RunDiagStrict: $diagMsg"
        }
      }
    } else {
      Write-Host "[10/10] smoke completed successfully"
    }
  $script:exitCode = 0
}
catch {
  Write-Host "`nSmoke failed: $($_.Exception.Message)" -ForegroundColor Red
  $script:exitCode = 1
}
finally {
  if ($script:tauriProc -and -not $script:tauriProc.HasExited) {
    Write-Host "Stopping tauri dev process tree (PID=$($script:tauriProc.Id))"
    cmd /c "taskkill /PID $($script:tauriProc.Id) /T /F" | Out-Null
  }

  if ($script:exitCode -eq 0) {
    Write-Host "`n=== Final Summary ==="
    Write-Host "Tauri startup: OK"
    Write-Host "Pipeline run: OK"
    Write-Host "Run dir: $($script:summary.RunDir)"
    Write-Host "Verified artifacts: input.json, result.json, paper_graph/tree/tree.md"
    Write-Host "Manual next action: launch 'npx tauri dev' and run UI flow."
  } else {
    Write-Host "`n=== Final Summary ==="
    Write-Host "Smoke status: FAILED"
  }
}

exit $script:exitCode
