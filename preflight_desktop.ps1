param(
  [string]$PipelineRoot = "",
  [switch]$RequireApiKey
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
  throw $Message
}

function Require-Command([string]$Name) {
  $cmd = Get-Command $Name -ErrorAction SilentlyContinue
  if (-not $cmd) {
    Fail "Missing required command: $Name"
  }
  return $cmd.Source
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

function Resolve-PipelineRoot([string]$Candidate, [string]$DesktopRoot) {
  if (-not [string]::IsNullOrWhiteSpace($env:JARVIS_PIPELINE_ROOT)) {
    return (Resolve-Path $env:JARVIS_PIPELINE_ROOT).Path
  }
  if (-not [string]::IsNullOrWhiteSpace($Candidate)) {
    return (Resolve-Path $Candidate).Path
  }
  return (Resolve-Path (Join-Path $DesktopRoot "..\..\jarvis-ml-pipeline")).Path
}

function Assert-PipelineLayout([string]$Root) {
  $required = @(
    (Join-Path $Root "pyproject.toml"),
    (Join-Path $Root "jarvis_cli.py"),
    (Join-Path $Root "jarvis_core")
  )
  foreach ($path in $required) {
    if (-not (Test-Path $path)) {
      Fail "Pipeline root is invalid. Missing: $path"
    }
  }
}

$desktopRoot = (Resolve-Path (Split-Path -Parent $MyInvocation.MyCommand.Path)).Path
Set-Location $desktopRoot

if (-not (Test-Path (Join-Path $desktopRoot "package.json"))) {
  Fail "Desktop root check failed: package.json not found."
}
if (-not (Test-Path (Join-Path $desktopRoot "src-tauri"))) {
  Fail "Desktop root check failed: src-tauri directory not found."
}

$nodePath = Require-Command "node"
$npmPath = Require-Command "npm"
$pythonPath = Require-Command "python"
$cargoPath = Resolve-CargoPath
if (-not $cargoPath) {
  Fail "Missing required command: cargo (install Rust via rustup and reopen shell)."
}
$rustcPath = Require-Command "rustc"

$resolvedPipeline = Resolve-PipelineRoot -Candidate $PipelineRoot -DesktopRoot $desktopRoot
Assert-PipelineLayout -Root $resolvedPipeline
$env:JARVIS_PIPELINE_ROOT = $resolvedPipeline

if ([string]::IsNullOrWhiteSpace($env:JARVIS_PIPELINE_OUT_DIR)) {
  $env:JARVIS_PIPELINE_OUT_DIR = Join-Path $resolvedPipeline "logs\runs"
}

node --version | Out-Host
npm --version | Out-Host
python --version | Out-Host
cargo -V | Out-Host
rustc -V | Out-Host

if ($RequireApiKey -and [string]::IsNullOrWhiteSpace($env:S2_API_KEY)) {
  Fail "S2_API_KEY is required but not set."
}
if ([string]::IsNullOrWhiteSpace($env:S2_API_KEY)) {
  Write-Host "S2_API_KEY is not set (optional)." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Preflight passed." -ForegroundColor Green
Write-Host "JARVIS_PIPELINE_ROOT=$($env:JARVIS_PIPELINE_ROOT)"
Write-Host "JARVIS_PIPELINE_OUT_DIR=$($env:JARVIS_PIPELINE_OUT_DIR)"
Write-Host "node=$nodePath"
Write-Host "npm=$npmPath"
Write-Host "python=$pythonPath"
Write-Host "cargo=$cargoPath"
Write-Host "rustc=$rustcPath"
