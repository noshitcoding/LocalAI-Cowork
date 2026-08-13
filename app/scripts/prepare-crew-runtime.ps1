[CmdletBinding()]
param(
    [string]$PythonCommand = "python",
    [switch]$Force,
    [switch]$UpdateLock
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$expectedPythonVersion = "3.12.10"
$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$tauriRoot = Join-Path $appRoot "src-tauri"
$runtimeSourceRoot = Join-Path $tauriRoot "python\crew_runtime"
$requirementsPath = Join-Path $runtimeSourceRoot "requirements.txt"
$lockPath = Join-Path $runtimeSourceRoot "requirements.lock"
$runtimeScriptPath = Join-Path $runtimeSourceRoot "main.py"
$pythonArchivePath = Join-Path $tauriRoot "resources\python\windows.zip"
$wheelArchivePath = Join-Path $runtimeSourceRoot "wheels.zip"
$manifestPath = Join-Path $runtimeSourceRoot "runtime-bundle-manifest.json"
$pythonArchiveRelativePath = "src-tauri/resources/python/windows.zip"
$wheelArchiveRelativePath = "src-tauri/python/crew_runtime/wheels.zip"

Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash($stream)
        return -join ($hash | ForEach-Object { $_.ToString("x2") })
    }
    finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

function Get-FileDescriptor {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )

    $item = Get-Item -LiteralPath $Path
    return [ordered]@{
        path = $RelativePath
        bytes = $item.Length
        sha256 = Get-Sha256 -Path $Path
    }
}

function New-TaskTempDirectory {
    param([Parameter(Mandatory = $true)][string]$Label)

    $path = Join-Path ([System.IO.Path]::GetTempPath()) (
        "localai-cowork-{0}-{1}" -f $Label, [guid]::NewGuid().ToString("N")
    )
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    return [System.IO.Path]::GetFullPath($path)
}

function Remove-TaskTempDirectory {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $resolvedPath = [System.IO.Path]::GetFullPath($Path)
    $resolvedTempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolvedPath.StartsWith($resolvedTempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a non-temporary Crew runtime path: $resolvedPath"
    }

    Remove-Item -LiteralPath $resolvedPath -Recurse -Force
}

function Invoke-Python {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [switch]$CaptureOutput
    )

    $effectiveArguments = @($Arguments)
    for ($index = 1; $index -lt $effectiveArguments.Count; $index += 1) {
        if (
            $effectiveArguments[$index - 1] -eq "-c" -and
            $effectiveArguments[$index] -match "[`r`n]"
        ) {
            $encodedScript = [Convert]::ToBase64String(
                [System.Text.Encoding]::UTF8.GetBytes($effectiveArguments[$index])
            )
            $effectiveArguments[$index] = (
                "import base64;exec(base64.b64decode('{0}'))" -f $encodedScript
            )
        }
    }

    if ($CaptureOutput) {
        $output = & $Command @effectiveArguments
        if ($LASTEXITCODE -ne 0) {
            throw "Python command failed with exit code $LASTEXITCODE`: $Command $($Arguments -join ' ')"
        }
        return ($output | Out-String).Trim()
    }

    & $Command @effectiveArguments | ForEach-Object { Write-Host $_ }
    if ($LASTEXITCODE -ne 0) {
        throw "Python command failed with exit code $LASTEXITCODE`: $Command $($Arguments -join ' ')"
    }
}

function Get-PythonBuildInfo {
    param([Parameter(Mandatory = $true)][string]$Command)

    $script = @"
import json
import platform
import struct
import sys
print(json.dumps({
    "version": platform.python_version(),
    "basePrefix": sys.base_prefix,
    "bits": struct.calcsize("P") * 8,
}))
"@
    $json = Invoke-Python -Command $Command -Arguments @("-c", $script) -CaptureOutput
    return $json | ConvertFrom-Json
}

function Test-PythonArchive {
    param([Parameter(Mandatory = $true)][string]$ArchivePath)

    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        return $false
    }

    $testRoot = New-TaskTempDirectory -Label "python-archive-check"
    try {
        [System.IO.Compression.ZipFile]::ExtractToDirectory($ArchivePath, $testRoot)
        $python = Join-Path $testRoot "python.exe"
        if (-not (Test-Path -LiteralPath $python)) {
            return $false
        }

        $version = Invoke-Python -Command $python -Arguments @(
            "-c",
            "import platform, sys, venv, ensurepip; print(platform.python_version())"
        ) -CaptureOutput
        if ($version -ne $expectedPythonVersion) {
            return $false
        }

        $venvRoot = Join-Path $testRoot "archive-venv-check"
        Invoke-Python -Command $python -Arguments @("-m", "venv", $venvRoot)
        $venvPython = Join-Path $venvRoot "Scripts\python.exe"
        return Test-Path -LiteralPath $venvPython
    }
    catch {
        Write-Warning "Bundled Python validation failed: $($_.Exception.Message)"
        return $false
    }
    finally {
        Remove-TaskTempDirectory -Path $testRoot
    }
}

function New-PythonArchive {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    $buildInfo = Get-PythonBuildInfo -Command $Command
    if ($buildInfo.version -ne $expectedPythonVersion -or $buildInfo.bits -ne 64) {
        throw (
            "Crew runtime packaging requires 64-bit Python {0}; {1} reports {2} ({3}-bit)." -f
            $expectedPythonVersion,
            $Command,
            $buildInfo.version,
            $buildInfo.bits
        )
    }

    $sourceRoot = [System.IO.Path]::GetFullPath([string]$buildInfo.basePrefix)
    $sourcePython = Join-Path $sourceRoot "python.exe"
    if (-not (Test-Path -LiteralPath $sourcePython)) {
        throw "Python base prefix does not contain python.exe: $sourceRoot"
    }

    $destinationDirectory = Split-Path -Parent $Destination
    New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
    $temporaryArchive = Join-Path (
        New-TaskTempDirectory -Label "python-archive-build"
    ) "windows.zip"
    try {
        [System.IO.Compression.ZipFile]::CreateFromDirectory(
            $sourceRoot,
            $temporaryArchive,
            [System.IO.Compression.CompressionLevel]::Optimal,
            $false
        )
        if (Test-Path -LiteralPath $Destination) {
            Remove-Item -LiteralPath $Destination -Force
        }
        Move-Item -LiteralPath $temporaryArchive -Destination $Destination -Force
    }
    finally {
        Remove-TaskTempDirectory -Path (Split-Path -Parent $temporaryArchive)
    }

    if (-not (Test-PythonArchive -ArchivePath $Destination)) {
        throw "Generated Python archive is not a portable, venv-capable Python $expectedPythonVersion runtime."
    }
}

function Get-ExpectedCrewAiVersion {
    $requirements = Get-Content -LiteralPath $requirementsPath
    $match = $requirements | Select-String -Pattern '^crewai(?:\[[^\]]+\])?==([0-9][^\s;]+)$' | Select-Object -First 1
    if (-not $match) {
        throw "requirements.txt must pin CrewAI to an exact version."
    }
    return $match.Matches[0].Groups[1].Value
}

function Get-WheelInventory {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$WheelDirectory
    )

    $inventoryScript = @'
import email
import hashlib
import json
import pathlib
import re
import sys
import urllib.parse
import zipfile

root = pathlib.Path(sys.argv[1])
overrides = {
    "aiohttp": "Apache-2.0 AND MIT",
    "cel-python": "Apache-2.0",
    "crewai": "MIT",
    "crewai-cli": "MIT",
    "crewai-core": "MIT",
    "pypdfium2": "BSD-3-Clause OR Apache-2.0",
    "tiktoken": "MIT",
}

classifier_licenses = {
    "License :: OSI Approved :: Apache Software License": "Apache-2.0",
    "License :: OSI Approved :: BSD License": "BSD-3-Clause",
    "License :: OSI Approved :: Boost Software License 1.0 (BSL-1.0)": "BSL-1.0",
    "License :: OSI Approved :: ISC License (ISCL)": "ISC",
    "License :: OSI Approved :: MIT License": "MIT",
    "License :: OSI Approved :: Mozilla Public License 2.0 (MPL 2.0)": "MPL-2.0",
    "License :: OSI Approved :: Python Software Foundation License": "Python-2.0",
    "License :: OSI Approved :: The Unlicense (Unlicense)": "Unlicense",
}

def canonical_name(value):
    return re.sub(r"[-_.]+", "-", value).lower()

def normalize_license(value):
    normalized = " ".join((value or "").split()).strip()
    replacements = [
        (r"\bPSF-2\.0\b", "Python-2.0"),
        (r"\bPSF\b", "Python-2.0"),
        (r"\bApache License 2(?:\.0)?\b", "Apache-2.0"),
        (r"\bApache License, Version 2(?:\.0)?\b", "Apache-2.0"),
        (r"\bApache Software License\b", "Apache-2.0"),
        (r"\bApache 2(?:\.0)?\b", "Apache-2.0"),
        (r"\bMIT License\b", "MIT"),
        (r"\bMozilla Public License 2\.0 \(MPL 2\.0\)\b", "MPL-2.0"),
        (r"\b3-Clause BSD License\b", "BSD-3-Clause"),
        (r"\bBSD 3-Clause License\b", "BSD-3-Clause"),
        (r"\b2-Clause BSD License\b", "BSD-2-Clause"),
        (r"\bISC License\b", "ISC"),
    ]
    for pattern, replacement in replacements:
        normalized = re.sub(pattern, replacement, normalized, flags=re.IGNORECASE)
    return normalized

def resolve_license(metadata, package_name):
    normalized_name = canonical_name(package_name)
    if normalized_name in overrides:
        return overrides[normalized_name]

    expression = normalize_license(metadata.get("License-Expression", ""))
    if expression:
        return expression

    classifiers = metadata.get_all("Classifier", [])
    resolved = sorted({
        classifier_licenses[classifier]
        for classifier in classifiers
        if classifier in classifier_licenses
    })
    if resolved:
        return " OR ".join(resolved)

    raw = normalize_license(metadata.get("License", ""))
    known = {
        "Apache-2.0",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "ISC",
        "MIT",
        "MIT OR Apache-2.0",
        "MPL-2.0",
        "MPL-2.0 AND MIT",
        "Python-2.0",
        "Unlicense",
    }
    if raw in known:
        return raw
    if raw.lower() == "bsd":
        return "BSD-3-Clause"
    raise RuntimeError(
        f"Could not resolve an SPDX license for {package_name}: "
        f"expression={metadata.get('License-Expression')!r}, license={metadata.get('License')!r}"
    )

packages = []
for wheel in sorted(root.glob("*.whl"), key=lambda path: path.name.lower()):
    with zipfile.ZipFile(wheel) as archive:
        metadata_names = [
            name for name in archive.namelist()
            if name.endswith(".dist-info/METADATA")
        ]
        if len(metadata_names) != 1:
            raise RuntimeError(f"{wheel.name} does not contain exactly one METADATA file")
        metadata = email.message_from_bytes(archive.read(metadata_names[0]))

    name = metadata.get("Name")
    version = metadata.get("Version")
    if not name or not version:
        raise RuntimeError(f"{wheel.name} is missing Name or Version metadata")
    normalized_name = canonical_name(name)
    packages.append({
        "name": name,
        "version": version,
        "license": resolve_license(metadata, name),
        "purl": (
            "pkg:pypi/"
            + urllib.parse.quote(normalized_name, safe="")
            + "@"
            + urllib.parse.quote(version, safe="")
        ),
        "filename": wheel.name,
        "sha256": hashlib.sha256(wheel.read_bytes()).hexdigest(),
    })

print(json.dumps(packages, sort_keys=True))
'@

    $json = Invoke-Python -Command $Command -Arguments @(
        "-c",
        $inventoryScript,
        $WheelDirectory
    ) -CaptureOutput
    $parsedPackages = $json | ConvertFrom-Json
    foreach ($package in $parsedPackages) {
        Write-Output $package
    }
}

function Assert-RequiredRuntimePackages {
    param(
        [Parameter(Mandatory = $true)][object[]]$Packages,
        [Parameter(Mandatory = $true)][string]$CrewAiVersion
    )

    $versions = @{}
    foreach ($package in $Packages) {
        $normalized = ([string]$package.name).ToLowerInvariant() -replace '[-_.]+', '-'
        $versions[$normalized] = [string]$package.version
    }

    $required = [ordered]@{
        "crewai" = $CrewAiVersion
        "pydantic" = "2.12.5"
        "pyyaml" = "6.0.3"
        "python-docx" = "1.2.0"
        "python-pptx" = "1.0.2"
    }
    foreach ($entry in $required.GetEnumerator()) {
        if (-not $versions.ContainsKey($entry.Key) -or $versions[$entry.Key] -ne $entry.Value) {
            throw "Offline wheelhouse requires $($entry.Key)==$($entry.Value)."
        }
    }
}

function Write-RuntimeLock {
    param([Parameter(Mandatory = $true)][object[]]$Packages)

    $lines = @(
        "# Windows x64 / CPython 3.12 CrewAI runtime lock. Generated from the verified wheelhouse."
        "# Update intentionally with: npm run prepare:crew-runtime -- -UpdateLock"
    )
    $lines += $Packages |
        Sort-Object {
            (([string]$_.name).ToLowerInvariant() -replace '[-_.]+', '-')
        } |
        ForEach-Object {
            $name = (([string]$_.name).ToLowerInvariant() -replace '[-_.]+', '-')
            "{0}=={1} --hash=sha256:{2}" -f $name, $_.version, $_.sha256
        }
    [System.IO.File]::WriteAllText(
        $lockPath,
        (($lines -join "`n") + "`n"),
        [System.Text.UTF8Encoding]::new($false)
    )
}

function New-WheelArchive {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$CrewAiVersion,
        [switch]$UpdateDependencyLock
    )

    $buildRoot = New-TaskTempDirectory -Label "wheelhouse-build"
    $wheelDirectory = Join-Path $buildRoot "wheels"
    New-Item -ItemType Directory -Path $wheelDirectory -Force | Out-Null
    try {
        $downloadArguments = @(
            "-m",
            "pip",
            "download",
            "--disable-pip-version-check",
            "--quiet",
            "--only-binary=:all:",
            "--platform",
            "win_amd64",
            "--python-version",
            "3.12",
            "--implementation",
            "cp",
            "--abi",
            "cp312",
            "--dest",
            $wheelDirectory
        )
        if ($UpdateDependencyLock) {
            $downloadArguments += @("-r", $requirementsPath)
        }
        else {
            $downloadArguments += @("--require-hashes", "-r", $lockPath)
        }
        Invoke-Python -Command $Command -Arguments $downloadArguments

        $packages = @(Get-WheelInventory -Command $Command -WheelDirectory $wheelDirectory)
        Assert-RequiredRuntimePackages -Packages $packages -CrewAiVersion $CrewAiVersion
        if ($UpdateDependencyLock) {
            Write-RuntimeLock -Packages $packages
        }

        $temporaryArchive = Join-Path $buildRoot "wheels.zip"
        [System.IO.Compression.ZipFile]::CreateFromDirectory(
            $wheelDirectory,
            $temporaryArchive,
            [System.IO.Compression.CompressionLevel]::Optimal,
            $false
        )
        if (Test-Path -LiteralPath $Destination) {
            Remove-Item -LiteralPath $Destination -Force
        }
        Move-Item -LiteralPath $temporaryArchive -Destination $Destination -Force
        return $packages
    }
    finally {
        Remove-TaskTempDirectory -Path $buildRoot
    }
}

function Invoke-OfflineRuntimeSmoke {
    param(
        [Parameter(Mandatory = $true)][string]$PythonArchive,
        [Parameter(Mandatory = $true)][string]$WheelArchive,
        [Parameter(Mandatory = $true)][string]$CrewAiVersion
    )

    $smokeRoot = New-TaskTempDirectory -Label "crew-runtime-smoke"
    try {
        $pythonRoot = Join-Path $smokeRoot "python"
        $wheelRoot = Join-Path $smokeRoot "wheels"
        New-Item -ItemType Directory -Path $pythonRoot -Force | Out-Null
        New-Item -ItemType Directory -Path $wheelRoot -Force | Out-Null
        [System.IO.Compression.ZipFile]::ExtractToDirectory($PythonArchive, $pythonRoot)
        [System.IO.Compression.ZipFile]::ExtractToDirectory($WheelArchive, $wheelRoot)

        $python = Join-Path $pythonRoot "python.exe"
        $venvRoot = Join-Path $smokeRoot "venv"
        Invoke-Python -Command $python -Arguments @("-m", "venv", $venvRoot)
        $venvPython = Join-Path $venvRoot "Scripts\python.exe"

        $previousNoIndex = $env:PIP_NO_INDEX
        $previousDisableVersionCheck = $env:PIP_DISABLE_PIP_VERSION_CHECK
        $previousCacheDirectory = $env:PIP_CACHE_DIR
        try {
            $env:PIP_NO_INDEX = "1"
            $env:PIP_DISABLE_PIP_VERSION_CHECK = "1"
            $env:PIP_CACHE_DIR = Join-Path $smokeRoot "pip-cache"
            Invoke-Python -Command $venvPython -Arguments @(
                "-m",
                "pip",
                "install",
                "--quiet",
                "--no-compile",
                "--no-index",
                "--find-links",
                $wheelRoot,
                "-r",
                $requirementsPath
            )
        }
        finally {
            $env:PIP_NO_INDEX = $previousNoIndex
            $env:PIP_DISABLE_PIP_VERSION_CHECK = $previousDisableVersionCheck
            $env:PIP_CACHE_DIR = $previousCacheDirectory
        }

        $previousLocalModelCostMap = $env:LITELLM_LOCAL_MODEL_COST_MAP
        try {
            $env:LITELLM_LOCAL_MODEL_COST_MAP = "True"
            $statusJson = Invoke-Python -Command $venvPython -Arguments @(
                $runtimeScriptPath,
                "status"
            ) -CaptureOutput
            $status = $statusJson | ConvertFrom-Json
            if (
                -not $status.runtimeCompatible -or
                -not $status.toolDependenciesInstalled -or
                $status.crewaiVersion -ne $CrewAiVersion -or
                $status.pythonVersion -ne $expectedPythonVersion
            ) {
                throw "Offline Crew runtime smoke failed: $statusJson"
            }

            Invoke-Python -Command $venvPython -Arguments @(
                "-m",
                "unittest",
                "discover",
                "-s",
                $runtimeSourceRoot,
                "-p",
                "test_crew_runtime.py"
            )
        }
        finally {
            $env:LITELLM_LOCAL_MODEL_COST_MAP = $previousLocalModelCostMap
        }

        return [ordered]@{
            verified = $true
            offline = $true
            testsPassed = $true
            pythonVersion = [string]$status.pythonVersion
            crewaiVersion = [string]$status.crewaiVersion
            runtimeCompatible = [bool]$status.runtimeCompatible
            toolDependenciesInstalled = [bool]$status.toolDependenciesInstalled
            runtimeSchemaVersion = [int]$status.runtimeSchemaVersion
        }
    }
    finally {
        Remove-TaskTempDirectory -Path $smokeRoot
    }
}

function Test-ExistingManifest {
    param(
        [Parameter(Mandatory = $true)][string]$ExpectedRequirementsHash,
        [Parameter(Mandatory = $true)][string]$ExpectedLockHash,
        [Parameter(Mandatory = $true)][string]$ExpectedCrewAiVersion
    )

    if (
        -not (Test-Path -LiteralPath $manifestPath) -or
        -not (Test-Path -LiteralPath $pythonArchivePath) -or
        -not (Test-Path -LiteralPath $wheelArchivePath)
    ) {
        return $false
    }

    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        $manifestPackages = @($manifest.packages | Where-Object {
            $_ -isnot [string] -and
            $_.PSObject.Properties.Name -contains "name" -and
            $_.PSObject.Properties.Name -contains "version" -and
            $_.PSObject.Properties.Name -contains "filename" -and
            $_.PSObject.Properties.Name -contains "sha256" -and
            -not [string]::IsNullOrWhiteSpace([string]$_.name) -and
            -not [string]::IsNullOrWhiteSpace([string]$_.version) -and
            -not [string]::IsNullOrWhiteSpace([string]$_.filename) -and
            [string]$_.sha256 -match '^[a-f0-9]{64}$'
        })
        $manifestCrewAiPackages = @($manifestPackages | Where-Object {
            (([string]$_.name).ToLowerInvariant() -replace '[-_.]+', '-') -eq "crewai" -and
            [string]$_.version -eq $ExpectedCrewAiVersion
        })
        return (
            $manifest.schemaVersion -eq 1 -and
            $manifest.python.version -eq $expectedPythonVersion -and
            $manifest.python.archive.sha256 -eq (Get-Sha256 -Path $pythonArchivePath) -and
            $manifest.wheelhouse.requirementsSha256 -eq $ExpectedRequirementsHash -and
            $manifest.wheelhouse.lockSha256 -eq $ExpectedLockHash -and
            $manifest.wheelhouse.archive.sha256 -eq (Get-Sha256 -Path $wheelArchivePath) -and
            $manifest.smoke.verified -eq $true -and
            $manifest.smoke.offline -eq $true -and
            $manifest.smoke.testsPassed -eq $true -and
            $manifest.smoke.crewaiVersion -eq $ExpectedCrewAiVersion -and
            $manifest.smoke.runtimeCompatible -eq $true -and
            $manifestPackages.Count -eq @($manifest.packages).Count -and
            $manifestCrewAiPackages.Count -eq 1
        )
    }
    catch {
        return $false
    }
}

if (-not (Test-Path -LiteralPath $requirementsPath)) {
    throw "Crew runtime requirements are missing: $requirementsPath"
}
if (-not (Test-Path -LiteralPath $runtimeScriptPath)) {
    throw "Crew runtime entry point is missing: $runtimeScriptPath"
}
if (-not $UpdateLock -and -not (Test-Path -LiteralPath $lockPath)) {
    throw "Crew runtime dependency lock is missing: $lockPath. Regenerate it explicitly with -UpdateLock."
}

$expectedCrewAiVersion = Get-ExpectedCrewAiVersion
$requirementsHash = Get-Sha256 -Path $requirementsPath
$lockHash = if (Test-Path -LiteralPath $lockPath) {
    Get-Sha256 -Path $lockPath
}
else {
    ""
}
if (-not $Force -and -not $UpdateLock -and (Test-ExistingManifest `
    -ExpectedRequirementsHash $requirementsHash `
    -ExpectedLockHash $lockHash `
    -ExpectedCrewAiVersion $expectedCrewAiVersion)) {
    Write-Host "Crew runtime bundle is current and already passed the offline smoke test."
    Write-Host $manifestPath
    exit 0
}

New-Item -ItemType Directory -Path (Split-Path -Parent $pythonArchivePath) -Force | Out-Null
if ($Force -or -not (Test-PythonArchive -ArchivePath $pythonArchivePath)) {
    Write-Host "Preparing portable Python $expectedPythonVersion..."
    New-PythonArchive -Command $PythonCommand -Destination $pythonArchivePath
}
else {
    Write-Host "Reusing validated portable Python $expectedPythonVersion archive."
}

Write-Host "Resolving the complete CPython 3.12 Windows wheelhouse..."
$packages = @(New-WheelArchive `
    -Command $PythonCommand `
    -Destination $wheelArchivePath `
    -CrewAiVersion $expectedCrewAiVersion `
    -UpdateDependencyLock:$UpdateLock)
$lockHash = Get-Sha256 -Path $lockPath

Write-Host "Running offline CrewAI installation smoke test..."
$smoke = Invoke-OfflineRuntimeSmoke `
    -PythonArchive $pythonArchivePath `
    -WheelArchive $wheelArchivePath `
    -CrewAiVersion $expectedCrewAiVersion

$manifest = [ordered]@{
    schemaVersion = 1
    python = [ordered]@{
        name = "CPython"
        version = $expectedPythonVersion
        license = "Python-2.0"
        purl = "pkg:generic/cpython@$expectedPythonVersion"
        archive = Get-FileDescriptor `
            -Path $pythonArchivePath `
            -RelativePath $pythonArchiveRelativePath
    }
    wheelhouse = [ordered]@{
        requirementsSha256 = $requirementsHash
        lockSha256 = $lockHash
        archive = Get-FileDescriptor `
            -Path $wheelArchivePath `
            -RelativePath $wheelArchiveRelativePath
    }
    packages = $packages
    smoke = $smoke
}

$manifestJson = ($manifest | ConvertTo-Json -Depth 10) + [Environment]::NewLine
[System.IO.File]::WriteAllText(
    $manifestPath,
    $manifestJson,
    [System.Text.UTF8Encoding]::new($false)
)

Write-Host "Crew runtime bundle prepared and verified:"
Write-Host $pythonArchivePath
Write-Host $wheelArchivePath
Write-Host $manifestPath
