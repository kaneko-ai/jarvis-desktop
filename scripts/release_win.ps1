param(
  [string]$PipelineRoot = "..\jarvis-ml-pipeline"
)

$ErrorActionPreference = "Stop"

function Assert-LastExitCode([string]$Label) {
  if ($LASTEXITCODE -ne 0) {
    throw "$Label failed with exit code $LASTEXITCODE"
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

function To-ForwardSlash([string]$PathText) {
  return ($PathText -replace "\\", "/")
}

function Get-RelativePath([string]$BasePath, [string]$TargetPath) {
  $baseResolved = [System.IO.Path]::GetFullPath((Resolve-Path $BasePath).Path).TrimEnd([char[]]"\\/")
  $targetResolved = [System.IO.Path]::GetFullPath((Resolve-Path $TargetPath).Path)

  $prefix = $baseResolved + "\"
  if ($targetResolved.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    return To-ForwardSlash($targetResolved.Substring($prefix.Length))
  }
  if ($targetResolved.Equals($baseResolved, [System.StringComparison]::OrdinalIgnoreCase)) {
    return "."
  }

  $baseUri = [System.Uri]($baseResolved + "\")
  $targetUri = [System.Uri]$targetResolved
  $relativeUri = $baseUri.MakeRelativeUri($targetUri)
  return To-ForwardSlash([System.Uri]::UnescapeDataString($relativeUri.ToString()))
}

function Ensure-Dir([string]$PathText) {
  if (-not (Test-Path $PathText)) {
    New-Item -Path $PathText -ItemType Directory -Force | Out-Null
  }
}

function Copy-Artifact([string]$SourcePath, [string]$DestDir, [string]$DestSubDir) {
  Ensure-Dir $DestDir
  $targetSubDir = Join-Path $DestDir $DestSubDir
  Ensure-Dir $targetSubDir
  $fileName = [System.IO.Path]::GetFileName($SourcePath)
  $destPath = Join-Path $targetSubDir $fileName
  Copy-Item -Path $SourcePath -Destination $destPath -Force
  return $destPath
}

$scriptDir = (Resolve-Path (Split-Path -Parent $MyInvocation.MyCommand.Path)).Path
$desktopRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $scriptDir))
Set-Location $desktopRoot

Write-Host "=== JARVIS Desktop Local Release (Windows) ==="
Write-Host "Desktop root: $desktopRoot"

if (-not (Test-Path (Join-Path $desktopRoot "package.json"))) {
  throw "package.json not found in desktop root"
}
if (-not (Test-Path (Join-Path $desktopRoot "src-tauri\tauri.conf.json"))) {
  throw "src-tauri/tauri.conf.json not found"
}

$gitStatusText = ""
try {
  $gitStatusText = (& git status --porcelain 2>$null | Out-String).Trim()
} catch {
  $gitStatusText = ""
}
if (-not [string]::IsNullOrWhiteSpace($gitStatusText)) {
  Write-Warning "Working tree has local changes (continuing by design)."
}

$cargoPath = Resolve-CargoPath
if (-not $cargoPath) {
  throw "cargo is not found. Install Rust/Cargo and reopen shell."
}

$pipelineRootResolved = (Resolve-Path -Path $PipelineRoot).Path

Write-Host ""
Write-Host "[1/5] Gate: dependency install"
if (Test-Path (Join-Path $desktopRoot "package-lock.json")) {
  npm ci
  Assert-LastExitCode "npm ci"
} else {
  npm install
  Assert-LastExitCode "npm install"
}

Write-Host ""
Write-Host "[2/5] Gate: frontend build"
npm run build
Assert-LastExitCode "npm run build"

Write-Host ""
Write-Host "[3/5] Gate: cargo test -q"
Push-Location (Join-Path $desktopRoot "src-tauri")
try {
  cargo test -q
  Assert-LastExitCode "cargo test -q"
} finally {
  Pop-Location
}

Write-Host ""
Write-Host "[4/5] Gate: smoke_tauri_e2e.ps1 -RunDiagStrict"
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $desktopRoot "smoke_tauri_e2e.ps1") -PipelineRoot $pipelineRootResolved -RunDiagStrict
Assert-LastExitCode "smoke_tauri_e2e.ps1"

Write-Host ""
Write-Host "[5/5] Build: tauri bundle"
npx tauri build
Assert-LastExitCode "npx tauri build"

$tauriConfig = Get-Content (Join-Path $desktopRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$appVersion = [string]$tauriConfig.version
if ([string]::IsNullOrWhiteSpace($appVersion)) {
  throw "tauri.conf.json version is empty"
}
$productName = [string]$tauriConfig.productName
if ([string]::IsNullOrWhiteSpace($productName)) {
  $productName = "jarvis-desktop"
}

$releaseRoot = Join-Path $desktopRoot (Join-Path "dist\releases" $appVersion)
if (Test-Path $releaseRoot) {
  Remove-Item -Path $releaseRoot -Recurse -Force
}
Ensure-Dir $releaseRoot

$bundleRoot = Join-Path $desktopRoot "src-tauri\target\release\bundle"
$releaseBinRoot = Join-Path $desktopRoot "src-tauri\target\release"

$artifacts = New-Object System.Collections.Generic.List[object]

if (Test-Path $bundleRoot) {
  $versionPattern = [regex]::Escape("_$appVersion")
  $bundleFiles = Get-ChildItem -Path $bundleRoot -Recurse -File | Where-Object {
    $ext = $_.Extension.ToLowerInvariant()
    ($ext -eq ".msi" -or $ext -eq ".exe") -and $_.Name -match $versionPattern
  } | Sort-Object FullName

  foreach ($file in $bundleFiles) {
    $copied = Copy-Artifact -SourcePath $file.FullName -DestDir $releaseRoot -DestSubDir "installers"
    $hash = (Get-FileHash -Path $copied -Algorithm SHA256).Hash.ToLowerInvariant()
    $rel = Get-RelativePath -BasePath $desktopRoot -TargetPath $copied
    $artifacts.Add([PSCustomObject]@{
      path = $rel
      size_bytes = [int64]$file.Length
      sha256 = $hash
    })
  }
}

$mainExe = Join-Path $releaseBinRoot ("{0}.exe" -f $productName)
if (-not (Test-Path $mainExe)) {
  $fallbackExe = Get-ChildItem -Path $releaseBinRoot -File -Filter "*.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne "rustc.exe" -and $_.Name -ne "cargo.exe" } |
    Sort-Object Name |
    Select-Object -First 1
  if ($fallbackExe) {
    $mainExe = $fallbackExe.FullName
  }
}

if (Test-Path $mainExe) {
  $mainExeInfo = Get-Item $mainExe
  $copiedMain = Copy-Artifact -SourcePath $mainExeInfo.FullName -DestDir $releaseRoot -DestSubDir "bin"
  $mainHash = (Get-FileHash -Path $copiedMain -Algorithm SHA256).Hash.ToLowerInvariant()
  $mainRel = Get-RelativePath -BasePath $desktopRoot -TargetPath $copiedMain
  $artifacts.Add([PSCustomObject]@{
    path = $mainRel
    size_bytes = [int64]$mainExeInfo.Length
    sha256 = $mainHash
  })
}

$installerCount = ($artifacts | Where-Object {
  $_.path.ToLowerInvariant().Contains("/installers/") -and ($_.path.ToLowerInvariant().EndsWith(".msi") -or $_.path.ToLowerInvariant().EndsWith(".exe"))
}).Count
if ($installerCount -lt 1) {
  throw "No installer artifact found (.msi/.exe) under tauri bundle output."
}

$artifacts = $artifacts | Sort-Object path

$shaPath = Join-Path $releaseRoot "SHA256SUMS.txt"
$shaLines = @()
foreach ($a in $artifacts) {
  $shaLines += ("{0}  {1}" -f $a.sha256, $a.path)
}
Set-Content -Path $shaPath -Value $shaLines -Encoding utf8

$gitCommit = "unknown"
try {
  $gitCommit = (& git rev-parse HEAD 2>$null | Out-String).Trim()
  if ([string]::IsNullOrWhiteSpace($gitCommit)) {
    $gitCommit = "unknown"
  }
} catch {
  $gitCommit = "unknown"
}

$manifest = [ordered]@{
  app_name = $productName
  app_version = $appVersion
  git_commit = $gitCommit
  built_at_utc = [DateTime]::UtcNow.ToString("o")
  artifacts = @($artifacts)
}

$manifestPath = Join-Path $releaseRoot "release_manifest.json"
$manifest | ConvertTo-Json -Depth 8 | Set-Content -Path $manifestPath -Encoding utf8

$manifestRel = Get-RelativePath -BasePath $desktopRoot -TargetPath $manifestPath
$shaRel = Get-RelativePath -BasePath $desktopRoot -TargetPath $shaPath

Write-Host ""
Write-Host "Local release build completed." -ForegroundColor Green
Write-Host "manifest: $manifestRel"
Write-Host "checksums: $shaRel"
Write-Host "artifacts:"
foreach ($a in $artifacts) {
  Write-Host ("  - {0} ({1} bytes)" -f $a.path, $a.size_bytes)
}