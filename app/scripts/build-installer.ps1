$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$repoRoot = (Resolve-Path (Join-Path $appRoot "..")).Path
$installerDir = Join-Path $repoRoot "dist-installers"
$targetRoot = Join-Path $appRoot "src-tauri\target"

# The Windows GNU Rust toolchain calls dlltool, which can fail when the
# target directory contains spaces. Keep local release artifacts in a temp
# target dir unless the caller explicitly configured CARGO_TARGET_DIR.
if (-not $env:CARGO_TARGET_DIR -and $targetRoot -match "\s") {
    $env:CARGO_TARGET_DIR = Join-Path ([System.IO.Path]::GetTempPath()) "open-cowork-target"
}

$effectiveTargetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { $targetRoot }
$bundleDir = Join-Path $effectiveTargetRoot "release\bundle\nsis"
$stableInstallerPath = Join-Path $installerDir "Open-Cowork-Setup.exe"

Push-Location $appRoot
try {
    npm run tauri build
}
finally {
    Pop-Location
}

$latestInstaller = Get-ChildItem -LiteralPath $bundleDir -Filter "*.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $latestInstaller) {
    throw "Kein NSIS-Installer gefunden in $bundleDir"
}

New-Item -ItemType Directory -Path $installerDir -Force | Out-Null
Copy-Item -LiteralPath $latestInstaller.FullName -Destination $stableInstallerPath -Force

Write-Host "Installer gebaut:"
Write-Host $latestInstaller.FullName
Write-Host ""
Write-Host "Kopie fuer Weitergabe:"
Write-Host $stableInstallerPath
