[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [switch]$SkipRuntimeBootstrap,

    [switch]$UninstallAfterVerification
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$installRoot = Join-Path $env:LOCALAPPDATA "Local AI Cowork"
$appExecutable = Join-Path $installRoot "app.exe"
$appProcess = $null

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-ManifestArchive {
    param(
        [Parameter(Mandatory = $true)][object]$Archive,
        [Parameter(Mandatory = $true)][string]$InstalledPath,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $InstalledPath -PathType Leaf)) {
        throw "$Label is missing from the installed application: $InstalledPath"
    }

    $item = Get-Item -LiteralPath $InstalledPath
    if ($item.Length -ne [long]$Archive.bytes) {
        throw "$Label size does not match the Crew runtime manifest."
    }
    if ((Get-Sha256 -Path $InstalledPath) -ne [string]$Archive.sha256) {
        throw "$Label SHA-256 does not match the Crew runtime manifest."
    }
}

try {
    $install = Start-Process `
        -FilePath $resolvedInstaller `
        -ArgumentList "/S" `
        -Wait `
        -PassThru
    if ($install.ExitCode -ne 0) {
        throw "Silent installer failed with exit code $($install.ExitCode)."
    }

    $requiredFiles = @(
        $appExecutable,
        (Join-Path $installRoot "python\windows.zip"),
        (Join-Path $installRoot "python\crew_runtime\wheels.zip"),
        (Join-Path $installRoot "python\crew_runtime\runtime-bundle-manifest.json"),
        (Join-Path $installRoot "python\crew_runtime\requirements.txt"),
        (Join-Path $installRoot "python\crew_runtime\requirements.lock"),
        (Join-Path $installRoot "python\crew_runtime\main.py"),
        (Join-Path $installRoot "python\crew_runtime\crew_tools.py")
    )
    $missingFiles = @($requiredFiles | Where-Object {
        -not (Test-Path -LiteralPath $_ -PathType Leaf)
    })
    if ($missingFiles.Count -gt 0) {
        throw "Installed Crew runtime payload is incomplete: $($missingFiles -join ', ')"
    }

    $manifestPath = Join-Path $installRoot "python\crew_runtime\runtime-bundle-manifest.json"
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if (
        $manifest.schemaVersion -ne 1 -or
        $manifest.python.version -ne "3.12.10" -or
        $manifest.smoke.verified -ne $true -or
        $manifest.smoke.offline -ne $true -or
        $manifest.smoke.testsPassed -ne $true -or
        $manifest.smoke.crewaiVersion -ne "1.15.8" -or
        $manifest.smoke.runtimeCompatible -ne $true
    ) {
        throw "Installed Crew runtime manifest does not describe a verified offline-compatible bundle."
    }

    Assert-ManifestArchive `
        -Archive $manifest.python.archive `
        -InstalledPath (Join-Path $installRoot "python\windows.zip") `
        -Label "Embedded Python archive"
    Assert-ManifestArchive `
        -Archive $manifest.wheelhouse.archive `
        -InstalledPath (Join-Path $installRoot "python\crew_runtime\wheels.zip") `
        -Label "CrewAI wheelhouse"

    $requirementsPath = Join-Path $installRoot "python\crew_runtime\requirements.txt"
    if ((Get-Sha256 -Path $requirementsPath) -ne [string]$manifest.wheelhouse.requirementsSha256) {
        throw "Installed requirements.txt SHA-256 does not match the Crew runtime manifest."
    }
    $lockPath = Join-Path $installRoot "python\crew_runtime\requirements.lock"
    if ((Get-Sha256 -Path $lockPath) -ne [string]$manifest.wheelhouse.lockSha256) {
        throw "Installed requirements.lock SHA-256 does not match the Crew runtime manifest."
    }

    $installedProduct = Get-ChildItem `
        -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall" |
        ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath } |
        Where-Object { $_.DisplayName -eq "Local AI Cowork" } |
        Select-Object -First 1
    if (-not $installedProduct) {
        throw "The Local AI Cowork uninstall registration is missing."
    }
    if ([string]$installedProduct.DisplayVersion -ne $ExpectedVersion) {
        throw (
            "Installed version mismatch: expected {0}, found {1}." -f
            $ExpectedVersion,
            $installedProduct.DisplayVersion
        )
    }

    if (-not $SkipRuntimeBootstrap) {
        $runtimeRoot = Join-Path $env:APPDATA "io.noshitcoding.opencowork\crew-runtime"
        $runtimePython = Join-Path $runtimeRoot "venv\Scripts\python.exe"
        $runtimeScript = Join-Path $installRoot "python\crew_runtime\main.py"
        $deadline = (Get-Date).AddMinutes(12)
        $runtimeStatus = $null
        $appProcess = Start-Process `
            -FilePath $appExecutable `
            -WindowStyle Hidden `
            -PassThru

        while ((Get-Date) -lt $deadline) {
            if (Test-Path -LiteralPath $runtimePython -PathType Leaf) {
                $previousLocalModelCostMap = $env:LITELLM_LOCAL_MODEL_COST_MAP
                try {
                    $env:LITELLM_LOCAL_MODEL_COST_MAP = "True"
                    $statusOutput = & $runtimePython $runtimeScript status 2>$null
                    if ($LASTEXITCODE -eq 0 -and $statusOutput) {
                        $runtimeStatus = ($statusOutput | Out-String) | ConvertFrom-Json
                        if (
                            $runtimeStatus.runtimeCompatible -eq $true -and
                            $runtimeStatus.toolDependenciesInstalled -eq $true -and
                            $runtimeStatus.pythonVersion -eq "3.12.10" -and
                            $runtimeStatus.crewaiVersion -eq "1.15.8"
                        ) {
                            break
                        }
                    }
                }
                catch {
                    $runtimeStatus = $null
                }
                finally {
                    $env:LITELLM_LOCAL_MODEL_COST_MAP = $previousLocalModelCostMap
                }
            }
            Start-Sleep -Seconds 5
        }

        if (
            -not $runtimeStatus -or
            $runtimeStatus.runtimeCompatible -ne $true -or
            $runtimeStatus.crewaiVersion -ne "1.15.8"
        ) {
            throw "Installed app did not provision the bundled CrewAI runtime within 12 minutes."
        }
    }

    $successMessage = (
        "Installer Crew runtime smoke passed for Local AI Cowork {0} " +
        "(Python {1}, CrewAI {2}, automatic bootstrap verified: {3})."
    )
    Write-Host (
        $successMessage -f
        $ExpectedVersion,
        $manifest.python.version,
        $manifest.smoke.crewaiVersion,
        (-not $SkipRuntimeBootstrap)
    )
}
finally {
    if ($appProcess -and -not $appProcess.HasExited) {
        Stop-Process -Id $appProcess.Id -Force -ErrorAction SilentlyContinue
        $null = $appProcess.WaitForExit(10000)
    }
    if ($UninstallAfterVerification) {
        $uninstaller = Join-Path $installRoot "uninstall.exe"
        if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
            $uninstall = Start-Process `
                -FilePath $uninstaller `
                -ArgumentList "/S" `
                -Wait `
                -PassThru
            if ($uninstall.ExitCode -ne 0) {
                throw "Silent test uninstall failed with exit code $($uninstall.ExitCode)."
            }
        }
    }
}
