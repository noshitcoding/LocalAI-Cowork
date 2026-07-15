$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$repoRoot = (Resolve-Path (Join-Path $appRoot "..")).Path
$installerDir = Join-Path $repoRoot "dist-installers"
$targetRoot = Join-Path $appRoot "src-tauri\target"

# Match the GitHub release workflow by default. Use TAURI_BUILD_TARGET to
# explicitly override this, for example x86_64-pc-windows-gnu.
if (-not $env:TAURI_BUILD_TARGET) {
    $env:TAURI_BUILD_TARGET = "x86_64-pc-windows-msvc"
}

function Import-MsvcEnvironment {
    if ($env:TAURI_BUILD_TARGET -notmatch "-msvc$") {
        return
    }

    if (Get-Command link.exe -ErrorAction SilentlyContinue) {
        return
    }

    $vswhereCandidates = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
        "$env:ProgramFiles\Microsoft Visual Studio\Installer\vswhere.exe"
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

    if (-not $vswhereCandidates) {
        throw "MSVC linker not found. Install Visual Studio Build Tools with the C++ build tools workload, or set TAURI_BUILD_TARGET=x86_64-pc-windows-gnu to build the GNU variant."
    }

    $vswhere = @($vswhereCandidates)[0]
    $installPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $installPath) {
        throw "Visual Studio C++ build tools not found. Install the 'Desktop development with C++' workload, or set TAURI_BUILD_TARGET=x86_64-pc-windows-gnu to build the GNU variant."
    }

    $vsDevCmd = Join-Path $installPath "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path -LiteralPath $vsDevCmd)) {
        throw "VsDevCmd.bat not found at $vsDevCmd"
    }

    cmd /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set" |
        ForEach-Object {
            if ($_ -match "^(.*?)=(.*)$") {
                [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
            }
        }

    if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
        throw "MSVC environment loaded, but link.exe is still unavailable."
    }
}

# Keep local release artifacts in a temp target dir unless the caller
# explicitly configured CARGO_TARGET_DIR.
if (-not $env:CARGO_TARGET_DIR -and $targetRoot -match "\s") {
    $env:CARGO_TARGET_DIR = Join-Path ([System.IO.Path]::GetTempPath()) "open-cowork-target"
}

$effectiveTargetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { $targetRoot }
$targetSegment = if ($env:TAURI_BUILD_TARGET) { "$($env:TAURI_BUILD_TARGET)\" } else { "" }
$bundleDir = Join-Path $effectiveTargetRoot "$($targetSegment)release\bundle\nsis"
$stableInstallerPath = Join-Path $installerDir "Open-Cowork-Setup.exe"

Push-Location $appRoot
try {
    Import-MsvcEnvironment
    npm run tauri -- build --target $env:TAURI_BUILD_TARGET
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri build fehlgeschlagen mit Exitcode $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$latestInstaller = Get-ChildItem -LiteralPath $bundleDir -Filter "*.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $latestInstaller) {
    throw "No NSIS installer found in $bundleDir"
}

New-Item -ItemType Directory -Path $installerDir -Force | Out-Null
Copy-Item -LiteralPath $latestInstaller.FullName -Destination $stableInstallerPath -Force

Write-Host "Installer built:"
Write-Host $latestInstaller.FullName
Write-Host ""
Write-Host "Copy for distribution:"
Write-Host $stableInstallerPath
