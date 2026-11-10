[CmdletBinding()]
param(
    [string]$AppDataDir = (Join-Path $env:APPDATA 'io.noshitcoding.opencowork'),
    [string]$SandboxExecutable
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tauriRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../src-tauri'))
$previousAppData = $env:LACOWORK_TEST_APP_DATA
$previousExecutable = $env:LACOWORK_TEST_SANDBOX_EXECUTABLE
$previousResourceDir = $env:LACOWORK_TEST_RESOURCE_DIR

# Use existing protected setup only. Never rotate the daily profile's sandbox
# account password, run the installer, or change the application's database.
if (-not (Test-Path -LiteralPath (Join-Path $AppDataDir 'native_windows_sandbox/setup.json'))) {
    throw 'Set up the native sandbox in the application first, then rerun this acceptance test.'
}

Push-Location $tauriRoot
try {
    if (-not $SandboxExecutable) {
        cargo build --locked --bin app
        if ($LASTEXITCODE -ne 0) { throw 'Native sandbox helper build failed.' }
        $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the Cargo target directory.' }
        $SandboxExecutable = Join-Path $metadata.target_directory 'debug/app.exe'
    }
    $env:LACOWORK_TEST_APP_DATA = (Resolve-Path -LiteralPath $AppDataDir).Path
    $env:LACOWORK_TEST_SANDBOX_EXECUTABLE = (Resolve-Path -LiteralPath $SandboxExecutable).Path
    $installedRoot = Split-Path -Parent $env:LACOWORK_TEST_SANDBOX_EXECUTABLE
    $env:LACOWORK_TEST_RESOURCE_DIR = if (Test-Path -LiteralPath (Join-Path $installedRoot 'runtime-layout.json')) { $installedRoot } else { Join-Path $tauriRoot 'resources' }
    cargo test --locked --lib native_windows_sandbox::tests::native_windows_ -- --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Native Windows sandbox acceptance failed.' }
    cargo test --locked --lib worker_sandbox::tests
    if ($LASTEXITCODE -ne 0) { throw 'Sandbox snapshot and diff acceptance failed.' }
} finally {
    $env:LACOWORK_TEST_APP_DATA = $previousAppData
    $env:LACOWORK_TEST_SANDBOX_EXECUTABLE = $previousExecutable
    $env:LACOWORK_TEST_RESOURCE_DIR = $previousResourceDir
    Pop-Location
}
