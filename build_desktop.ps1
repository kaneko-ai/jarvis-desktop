param(
  [string]$PipelineRoot = "",
  [switch]$SkipNpmCi
)

$ErrorActionPreference = "Stop"

$desktopRoot = (Resolve-Path (Split-Path -Parent $MyInvocation.MyCommand.Path)).Path
Set-Location $desktopRoot

& (Join-Path $desktopRoot "preflight_desktop.ps1") -PipelineRoot $PipelineRoot

if (-not $SkipNpmCi) {
  npm ci
  if ($LASTEXITCODE -ne 0) {
    throw "npm ci failed with exit code $LASTEXITCODE"
  }
}

npm run build
if ($LASTEXITCODE -ne 0) {
  throw "npm run build failed with exit code $LASTEXITCODE"
}

cargo build --manifest-path src-tauri/Cargo.toml
if ($LASTEXITCODE -ne 0) {
  throw "cargo build failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "Desktop build pipeline finished successfully." -ForegroundColor Green
