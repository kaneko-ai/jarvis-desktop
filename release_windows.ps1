param(
  [switch]$SkipNpmCi
)

$ErrorActionPreference = "Stop"

function Require-Command([string]$Name) {
  $cmd = Get-Command $Name -ErrorAction SilentlyContinue
  if (-not $cmd) {
    throw "Missing required command: $Name"
  }
}

function Resolve-CargoPath {
  $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
  if ($cargoCmd) { return $cargoCmd.Source }
  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  $cargoExe = Join-Path $cargoBin "cargo.exe"
  if (Test-Path $cargoExe) {
    $env:Path = "$cargoBin;$env:Path"
    $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargoCmd) { return $cargoCmd.Source }
  }
  return $null
}

$desktopRoot = (Resolve-Path (Split-Path -Parent $MyInvocation.MyCommand.Path)).Path
Set-Location $desktopRoot

Require-Command "node"
Require-Command "npm"
$cargoPath = Resolve-CargoPath
if (-not $cargoPath) {
  throw "Missing required command: cargo (install Rust via rustup and reopen shell)."
}
Require-Command "rustc"

cargo tauri --version | Out-Null
if ($LASTEXITCODE -ne 0) {
  throw "cargo tauri command is unavailable. Install with: cargo install tauri-cli --version '^2.0' --locked"
}

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

cargo tauri build
if ($LASTEXITCODE -ne 0) {
  throw "cargo tauri build failed with exit code $LASTEXITCODE"
}

$bundleDir = Join-Path $desktopRoot "src-tauri\target\release\bundle"
Write-Host ""
Write-Host "Windows release build completed." -ForegroundColor Green
Write-Host "Bundle output: $bundleDir"
if (Test-Path $bundleDir) {
  Get-ChildItem -Path $bundleDir -Recurse -File | Select-Object -ExpandProperty FullName
}
