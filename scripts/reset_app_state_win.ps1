param(
  [switch]$Confirm
)

$ErrorActionPreference = "Stop"

if (-not $Confirm) {
  throw "RULE_CONFIRM_REQUIRED: This script is destructive. Re-run with -Confirm."
}

$appData = [Environment]::GetFolderPath("ApplicationData")
if ([string]::IsNullOrWhiteSpace($appData)) {
  throw "RULE_APPDATA_MISSING: APPDATA is not available."
}

$jarvisDir = Join-Path $appData "jarvis-desktop"
$targets = @(
  (Join-Path $jarvisDir "config.json"),
  (Join-Path $jarvisDir "audit.jsonl")
)

Write-Host "=== reset_app_state_win.ps1 ==="
Write-Host "APPDATA: $appData"
Write-Host "Target dir: $jarvisDir"
Write-Host "Delete targets:"
$targets | ForEach-Object { Write-Host " - $_" }

foreach ($path in $targets) {
  if (Test-Path -LiteralPath $path) {
    Remove-Item -LiteralPath $path -Force
    Write-Host "deleted: $path"
  } else {
    Write-Host "skip (not found): $path"
  }
}

Write-Host "done: app state reset completed"
