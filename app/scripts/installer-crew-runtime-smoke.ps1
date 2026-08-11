[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [switch]$SkipRuntimeBootstrap,

    [switch]$SkipNativeSandbox,

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

function Invoke-LocalDaemonRequest {
    param(
        [Parameter(Mandatory = $true)][string]$Token,
        [Parameter(Mandatory = $true)][string]$Method,
        [Parameter(Mandatory = $true)][object]$Params
    )

    $pipeUser = $env:USERNAME -replace '[^A-Za-z0-9]', '_'
    $pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
        '.',
        "open-cowork-$pipeUser",
        [System.IO.Pipes.PipeDirection]::InOut,
        [System.IO.Pipes.PipeOptions]::None
    )
    $reader = $null
    $writer = $null
    try {
        $pipe.Connect(5000)
        $utf8 = [System.Text.UTF8Encoding]::new($false)
        $writer = [System.IO.StreamWriter]::new($pipe, $utf8, 4096, $true)
        $writer.NewLine = "`n"
        $writer.AutoFlush = $true
        $reader = [System.IO.StreamReader]::new($pipe, $utf8, $false, 4096, $true)
        $request = @{
            id = [Guid]::NewGuid().ToString()
            token = $Token
            method = $Method
            params = $Params
        } | ConvertTo-Json -Compress -Depth 20
        $writer.WriteLine($request)
        $response = $reader.ReadLine() | ConvertFrom-Json
        if ($response.error) {
            throw "$($response.error.code): $($response.error.message)"
        }
        return $response.result
    }
    finally {
        if ($reader) { $reader.Dispose() }
        if ($writer) { $writer.Dispose() }
        $pipe.Dispose()
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
        (Join-Path $installRoot "codex\runtime-bundle-manifest.json"),
        (Join-Path $installRoot "codex\LICENSE"),
        (Join-Path $installRoot "codex\vendor\x86_64-pc-windows-msvc\bin\codex.exe"),
        (Join-Path $installRoot "daemon\windows-x64\manifest.json"),
        (Join-Path $installRoot "daemon\windows-x64\cowork-local-daemon.exe"),
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
        throw "Installed runtime payload is incomplete: $($missingFiles -join ', ')"
    }

    $codexRoot = Join-Path $installRoot "codex"
    $codexManifestPath = Join-Path $codexRoot "runtime-bundle-manifest.json"
    $codexManifest = Get-Content -LiteralPath $codexManifestPath -Raw | ConvertFrom-Json
    if (
        $codexManifest.version -ne "0.147.0" -or
        $codexManifest.protocolSchema -ne "app-server-0.147.0" -or
        $codexManifest.target -ne "windows-x64"
    ) {
        throw "Installed Codex runtime manifest is incompatible."
    }
    foreach ($entry in @(
        @{ Label = "Codex executable"; RelativePath = [string]$codexManifest.binary; Sha256 = [string]$codexManifest.sha256 },
        @{ Label = "Codex license"; RelativePath = [string]$codexManifest.license; Sha256 = [string]$codexManifest.licenseSha256 }
    )) {
        if ([System.IO.Path]::IsPathRooted($entry.RelativePath)) {
            throw "$($entry.Label) path must be relative."
        }
        $candidate = [System.IO.Path]::GetFullPath((Join-Path $codexRoot $entry.RelativePath))
        $expectedPrefix = [System.IO.Path]::GetFullPath($codexRoot).TrimEnd('\') + '\'
        if (-not $candidate.StartsWith($expectedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "$($entry.Label) escapes the installed Codex resource directory."
        }
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "$($entry.Label) is missing from the installed application."
        }
        if ((Get-Sha256 -Path $candidate) -ne $entry.Sha256) {
            throw "$($entry.Label) SHA-256 does not match the Codex runtime manifest."
        }
    }
    $codexLicense = Get-Content -LiteralPath (Join-Path $codexRoot $codexManifest.license) -Raw
    if ($codexLicense -notmatch '(?m)^\s*Apache License\b') {
        throw "Installed Codex license is not Apache-2.0."
    }
    if (Test-Path -LiteralPath (Join-Path $codexRoot "auth.json")) {
        throw "Installed Codex payload contains a forbidden auth.json file."
    }
    $codexVersion = & (Join-Path $codexRoot $codexManifest.binary) --version
    if ($LASTEXITCODE -ne 0 -or ($codexVersion | Out-String) -notmatch '0\.147\.0') {
        throw "Installed Codex executable failed its version probe."
    }

    $daemonRoot = Join-Path $installRoot "daemon\windows-x64"
    $daemonManifestPath = Join-Path $daemonRoot "manifest.json"
    $daemonManifest = Get-Content -LiteralPath $daemonManifestPath -Raw | ConvertFrom-Json
    if (
        $daemonManifest.schemaVersion -ne 2 -or
        $daemonManifest.target -ne "windows-x64" -or
        $daemonManifest.binary -ne "cowork-local-daemon.exe" -or
        $daemonManifest.version -ne $ExpectedVersion -or
        [string]$daemonManifest.sha256 -notmatch '^[a-f0-9]{64}$'
    ) {
        throw "Installed local daemon manifest is incompatible."
    }
    $packagedDaemon = Join-Path $daemonRoot $daemonManifest.binary
    if ((Get-Sha256 -Path $packagedDaemon) -ne [string]$daemonManifest.sha256) {
        throw "Installed local daemon SHA-256 does not match its manifest."
    }
    $daemonFiles = @($daemonManifest.files)
    $pdfiumManifest = @($daemonFiles | Where-Object { $_.name -eq "pdfium.dll" })
    if (
        $daemonFiles.Count -ne 1 -or
        $pdfiumManifest.Count -ne 1 -or
        [string]$pdfiumManifest[0].sha256 -notmatch '^[a-f0-9]{64}$'
    ) {
        throw "Installed local daemon sidecar manifest is incompatible."
    }
    $packagedPdfium = Join-Path $daemonRoot $pdfiumManifest[0].name
    if ((Get-Sha256 -Path $packagedPdfium) -ne [string]$pdfiumManifest[0].sha256) {
        throw "Installed pdfium.dll SHA-256 does not match its manifest."
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

    if (-not $SkipNativeSandbox) {
        $sandboxResultPath = Join-Path $env:TEMP ("lacowork-native-sandbox-{0}.json" -f [Guid]::NewGuid())
        $sandboxAppData = Join-Path $env:APPDATA "io.noshitcoding.opencowork"
        try {
            $sandboxSmoke = Start-Process `
                -FilePath $appExecutable `
                -ArgumentList @(
                    "--lacowork-native-sandbox-smoke",
                    ('"' + $sandboxAppData + '"'),
                    ('"' + $sandboxResultPath + '"')
                ) `
                -Wait `
                -PassThru
            $sandboxResult = $null
            if (Test-Path -LiteralPath $sandboxResultPath -PathType Leaf) {
                $sandboxResult = Get-Content -LiteralPath $sandboxResultPath -Raw | ConvertFrom-Json
            }
            if ($sandboxSmoke.ExitCode -ne 0 -or -not $sandboxResult -or $sandboxResult.ok -ne $true) {
                $detail = if ($sandboxResult -and $sandboxResult.error) {
                    [string]$sandboxResult.error
                }
                else {
                    "the installed app returned no structured sandbox result"
                }
                throw "Installed native Windows sandbox smoke failed: $detail"
            }
            if (
                $sandboxResult.setupReady -ne $true -or
                $sandboxResult.repeatedSetupReady -ne $true -or
                $sandboxResult.account -ne "LACoworkOnline" -or
                $sandboxResult.group -ne "LACoworkSandbox" -or
                $sandboxResult.executionStatus -ne "completed" -or
                $sandboxResult.identityVerified -ne $true -or
                $sandboxResult.markerWritten -ne $true
            ) {
                throw "Installed native Windows sandbox smoke returned an incomplete result."
            }
        }
        finally {
            Remove-Item -LiteralPath $sandboxResultPath -Force -ErrorAction SilentlyContinue
        }
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

        $daemonSuffix = ([string]$daemonManifest.sha256).Substring(0, 16)
        $provisionedDaemon = Join-Path $env:LOCALAPPDATA "OpenCowork\daemon\bin\cowork-local-daemon-$daemonSuffix.exe"
        $daemonTokenPath = Join-Path $env:LOCALAPPDATA "OpenCowork\daemon\ipc-token.txt"
        $daemonUserPath = Join-Path $env:LOCALAPPDATA "OpenCowork\daemon\user-id.txt"
        $daemonDevicePath = Join-Path $env:LOCALAPPDATA "OpenCowork\daemon\device-id.txt"
        $daemonDeadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $daemonDeadline) {
            if (
                (Test-Path -LiteralPath $provisionedDaemon -PathType Leaf) -and
                (Test-Path -LiteralPath $daemonTokenPath -PathType Leaf) -and
                (Test-Path -LiteralPath $daemonUserPath -PathType Leaf) -and
                (Test-Path -LiteralPath $daemonDevicePath -PathType Leaf)
            ) {
                break
            }
            Start-Sleep -Milliseconds 250
        }
        if (
            -not (Test-Path -LiteralPath $provisionedDaemon -PathType Leaf) -or
            -not (Test-Path -LiteralPath $daemonTokenPath -PathType Leaf) -or
            -not (Test-Path -LiteralPath $daemonUserPath -PathType Leaf) -or
            -not (Test-Path -LiteralPath $daemonDevicePath -PathType Leaf)
        ) {
            throw "Installed app did not provision its local daemon binary and credentials."
        }
        if ((Get-Sha256 -Path $provisionedDaemon) -ne [string]$daemonManifest.sha256) {
            throw "Provisioned local daemon failed its integrity check."
        }
        $daemonToken = (Get-Content -LiteralPath $daemonTokenPath -Raw).Trim()
        $daemonUser = (Get-Content -LiteralPath $daemonUserPath -Raw).Trim()
        $daemonDevice = (Get-Content -LiteralPath $daemonDevicePath -Raw).Trim()
        $parsedDaemonDevice = [Guid]::Empty
        $contractUuidPattern = '^(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$'
        if (
            $daemonToken.Length -lt 64 -or
            -not [Guid]::TryParse($daemonDevice, [ref]$parsedDaemonDevice) -or
            $daemonUser -notmatch $contractUuidPattern
        ) {
            throw "Installed app provisioned invalid daemon credentials."
        }
        $daemonHealth = $null
        $daemonHealthDeadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $daemonHealthDeadline) {
            try {
                $daemonHealth = Invoke-LocalDaemonRequest -Token $daemonToken -Method 'health' -Params @{}
                break
            }
            catch {
                Start-Sleep -Milliseconds 250
            }
        }
        if (
            -not $daemonHealth -or
            $daemonHealth.status -ne 'ok' -or
            [string]$daemonHealth.daemon_version -ne $ExpectedVersion -or
            [string]$daemonHealth.device_id -ne $daemonDevice -or
            [string]$daemonHealth.user_id -ne $daemonUser -or
            [string]$daemonHealth.user_id -notmatch $contractUuidPattern
        ) {
            throw "Installed daemon health response violates the runtime identity contract."
        }
        $loginCommand = (Get-ItemProperty `
            -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" `
            -Name "OpenCoworkLocalDaemon" `
            -ErrorAction Stop).OpenCoworkLocalDaemon
        if ($loginCommand -ne ('"' + $provisionedDaemon + '"')) {
            throw "Installed app did not register the versioned local daemon for user login."
        }

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
        "Installer runtime smoke passed for Local AI Cowork {0} " +
        "(Codex {1}, daemon {2}, Python {3}, CrewAI {4}, automatic bootstrap verified: {5}, native sandbox verified: {6})."
    )
    Write-Host (
        $successMessage -f
        $ExpectedVersion,
        $codexManifest.version,
        $daemonManifest.version,
        $manifest.python.version,
        $manifest.smoke.crewaiVersion,
        (-not $SkipRuntimeBootstrap),
        (-not $SkipNativeSandbox)
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
            $daemonRunValue = Get-ItemProperty `
                -LiteralPath "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" `
                -Name "OpenCoworkLocalDaemon" `
                -ErrorAction SilentlyContinue
            if ($daemonRunValue) {
                throw "Uninstaller left the local daemon login registration behind."
            }
            if (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA "OpenCowork\daemon\bin")) {
                throw "Uninstaller left versioned local daemon binaries behind."
            }
        }
    }
}
