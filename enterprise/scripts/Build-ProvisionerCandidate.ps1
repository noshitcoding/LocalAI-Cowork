param([string]$Image = 'cowork-provisioner:update-candidate')
$ErrorActionPreference = 'Stop'
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
$buildDirectory = Join-Path $temporaryRoot ('cowork-provisioner-build-' + [guid]::NewGuid().ToString('N'))
$destination = Join-Path $buildDirectory 'server/cowork-provisioner'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
try {
    # Stage only the operator sources; never copy secrets or the fleet state.
    $dockerfileText = [IO.File]::ReadAllText((Join-Path $sourceRoot 'server/cowork-provisioner/Dockerfile'))
    $sourceNames = [regex]::Matches($dockerfileText, 'server/cowork-provisioner/([A-Za-z0-9_]+\.py)') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
    if ($sourceNames.Count -eq 0 -or $sourceNames -notcontains 'organisation_mfa.py') { throw 'Provisioner source inventory is incomplete' }
    foreach ($name in $sourceNames) {
        Copy-Item -LiteralPath (Join-Path $sourceRoot "server/cowork-provisioner/$name") -Destination (Join-Path $destination $name)
    }
    Copy-Item -LiteralPath (Join-Path $sourceRoot 'server/cowork-provisioner/Dockerfile') -Destination (Join-Path $buildDirectory 'Dockerfile')
    & docker build --tag $Image $buildDirectory
    if ($LASTEXITCODE -ne 0) { throw "Provisioner build failed with exit code $LASTEXITCODE" }
} finally {
    $resolved = [IO.Path]::GetFullPath($buildDirectory)
    if ([IO.Path]::GetDirectoryName($resolved) -ne $temporaryRoot -or [IO.Path]::GetFileName($resolved) -notmatch '^cowork-provisioner-build-[a-f0-9]{32}$') {
        throw 'Refusing to remove a build directory outside the generated temporary scope'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
