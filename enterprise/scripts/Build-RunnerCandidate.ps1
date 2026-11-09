param([string]$Image = 'cowork-runner:admission-candidate')
$ErrorActionPreference = 'Stop'
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
$buildDirectory = Join-Path $temporaryRoot ('cowork-runner-build-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $buildDirectory | Out-Null
try {
    # Keep Docker's metadata probes outside cloud-backed folders and exclude
    # credentials, desktop bundles and local build outputs from this context.
    foreach ($file in @('Cargo.toml', 'Cargo.lock')) {
        Copy-Item -LiteralPath (Join-Path $sourceRoot $file) -Destination (Join-Path $buildDirectory $file)
    }
    foreach ($crate in @('crates/cowork-contracts', 'crates/cowork-runtime', 'server/cowork-runner', 'server/cowork-server', 'agents/cowork-device-agent', 'agents/cowork-local-daemon')) {
        $destination = Join-Path $buildDirectory $crate
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $sourceRoot "$crate/Cargo.toml") -Destination (Join-Path $destination 'Cargo.toml')
        Copy-Item -LiteralPath (Join-Path $sourceRoot "$crate/src") -Destination (Join-Path $destination 'src') -Recurse
    }
    Copy-Item -LiteralPath (Join-Path $sourceRoot 'server/cowork-server/migrations') -Destination (Join-Path $buildDirectory 'server/cowork-server/migrations') -Recurse
    Copy-Item -LiteralPath (Join-Path $sourceRoot 'server/cowork-runner/Dockerfile') -Destination (Join-Path $buildDirectory 'Dockerfile')
    & docker build --tag $Image $buildDirectory
    if ($LASTEXITCODE -ne 0) { throw "Runner build failed with exit code $LASTEXITCODE" }
} finally {
    $resolved = [IO.Path]::GetFullPath($buildDirectory)
    if ([IO.Path]::GetDirectoryName($resolved) -ne $temporaryRoot -or [IO.Path]::GetFileName($resolved) -notmatch '^cowork-runner-build-[a-f0-9]{32}$') {
        throw 'Refusing to remove a build directory outside the generated temporary scope'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
