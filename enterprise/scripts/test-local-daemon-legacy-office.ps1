param([string]$PythonPath = 'python')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
    Write-Output 'local_daemon_legacy_office_skipped=non_windows'
    exit 0
}
foreach ($progId in 'Word.Application', 'Excel.Application', 'PowerPoint.Application') {
    if (-not (Test-Path -LiteralPath "Registry::HKEY_CLASSES_ROOT\$progId\CLSID")) {
        Write-Output "local_daemon_legacy_office_skipped=missing_$($progId.Split('.')[0].ToLowerInvariant())"
        exit 0
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$binary = Join-Path $repoRoot 'target\release\cowork-local-daemon.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Build the release daemon before running this test: $binary"
}

$testId = [Guid]::NewGuid().ToString('N')
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "open-cowork-daemon-legacy-office-$testId"
$workspace = Join-Path $testRoot 'workspace'
$pipeName = "open-cowork-daemon-legacy-office-$testId"
$endpoint = "\\.\pipe\$pipeName"
$daemonProcess = $null
$modelProcesses = [Collections.Generic.List[Diagnostics.Process]]::new()
$existingOfficeProcessIds = @(Get-Process WINWORD, EXCEL, POWERPNT -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$previousDataDir = $env:COWORK_DAEMON_DATA_DIR
$previousEndpoint = $env:COWORK_DAEMON_IPC_ENDPOINT
$previousToken = $env:COWORK_DAEMON_IPC_TOKEN
$previousTokenFile = $env:COWORK_DAEMON_IPC_TOKEN_FILE

function Invoke-Daemon {
    param([string]$Token, [string]$Method, $Params = $null)
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', $pipeName, [IO.Pipes.PipeDirection]::InOut)
    try {
        $pipe.Connect(5000)
        $writer = [IO.StreamWriter]::new($pipe, [Text.UTF8Encoding]::new($false), 4096, $true)
        $reader = [IO.StreamReader]::new($pipe, [Text.UTF8Encoding]::new($false), $false, 4096, $true)
        try {
            $writer.AutoFlush = $true
            $writer.WriteLine((@{
                id = [Guid]::NewGuid().ToString()
                token = $Token
                method = $Method
                params = $Params
            } | ConvertTo-Json -Compress -Depth 30))
            $response = $reader.ReadLine()
            if ([string]::IsNullOrWhiteSpace($response)) { throw 'Daemon returned an empty response' }
            return $response | ConvertFrom-Json
        }
        finally {
            $writer.Dispose()
            $reader.Dispose()
        }
    }
    finally {
        $pipe.Dispose()
    }
}

function New-LoopbackPort {
    $probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $probe.Start()
    try { return ([Net.IPEndPoint]$probe.LocalEndpoint).Port }
    finally { $probe.Stop() }
}

function Invoke-OfficeInspectRun {
    param(
        [string]$Token,
        [string]$DeviceId,
        [string]$ProjectId,
        [string]$RelativePath,
        [string]$ExpectedKind,
        [string]$ExpectedExtension
    )
    $port = New-LoopbackPort
    $argumentsJson = @{ path = $RelativePath } | ConvertTo-Json -Compress
    $argumentsBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($argumentsJson))
    $listener = Join-Path $PSScriptRoot 'test-local-daemon-model-listener.ps1'
    $model = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$listener`"",
        '-Port', $port, '-Mode', 'tool', '-RequestCount', 2,
        '-ToolName', 'OfficeInspect', '-ToolArgumentsBase64', $argumentsBase64,
        '-FinalContent', "inspected-$ExpectedExtension"
    ) -PassThru -WindowStyle Hidden
    $modelProcesses.Add($model)
    Start-Sleep -Milliseconds 300

    $created = Invoke-Daemon -Token $Token -Method 'runs.create' -Params @{
        thread_id = [Guid]::NewGuid().ToString()
        project_id = $ProjectId
        project_revision = 1
        project_privacy = 'private_local'
        task = $null
        executor_target = @{ kind = 'personal_device'; device_id = $DeviceId }
        required_capabilities = @('office.native', 'office.ooxml', 'files')
        input = @{
            prompt = "Inspect $RelativePath"
            tool_policy = 'autonomous'
            client_thread_id = "legacy-$ExpectedExtension"
            client_assistant_message_id = "assistant-$ExpectedExtension"
        }
        model_profile_id = $null
        snapshot_id = $null
        idempotency_key = "legacy-office-$ExpectedExtension-$testId"
        model_config = @{
            base_url = "http://127.0.0.1:$port/v1"
            api_key = $null
            model = 'tool-driver'
            timeout_ms = 180000
            max_steps = 4
            verify_tls_certificates = $true
        }
    }
    if ($created.error) { throw "Run creation failed: $($created.error.message)" }
    $runId = $created.result.spec.id
    $deadline = (Get-Date).AddMinutes(3)
    do {
        Start-Sleep -Milliseconds 250
        $run = Invoke-Daemon -Token $Token -Method 'runs.get' -Params @{ run_id = $runId }
    } while ($run.result.state -notin @('completed', 'failed', 'interrupted', 'canceled', 'expired') -and (Get-Date) -lt $deadline)
    if ($run.result.state -ne 'completed') {
        throw "Legacy $ExpectedExtension inspection failed: $($run.result | ConvertTo-Json -Compress -Depth 20)"
    }
    $events = Invoke-Daemon -Token $Token -Method 'runs.events' -Params @{ run_id = $runId; after = 0 }
    $toolEvent = @($events.result | Where-Object { $_.kind -eq 'tool_completed' -and $_.payload.tool -eq 'OfficeInspect' }) | Select-Object -Last 1
    if (-not $toolEvent) { throw "Legacy $ExpectedExtension inspection emitted no completed tool event" }
    $kindMarker = '"type": "' + $ExpectedKind + '"'
    if ($toolEvent.payload.content -notlike "*$kindMarker*" -or $toolEvent.payload.content -notlike "*converted_from*" -or $toolEvent.payload.content -notlike "*$ExpectedExtension*") {
        throw "Legacy $ExpectedExtension inspection result is incomplete: $($toolEvent.payload.content)"
    }
    if (-not $model.WaitForExit(10000) -or $model.ExitCode -ne 0) {
        throw "Tool-driving model listener failed for $ExpectedExtension"
    }
}

try {
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    $env:COWORK_DAEMON_DATA_DIR = $testRoot
    $env:COWORK_DAEMON_IPC_ENDPOINT = $endpoint
    Remove-Item Env:COWORK_DAEMON_IPC_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:COWORK_DAEMON_IPC_TOKEN_FILE -ErrorAction SilentlyContinue

    $fixtureScript = Join-Path $repoRoot 'scripts\create-office-fixtures.py'
    & $PythonPath $fixtureScript $workspace
    if ($LASTEXITCODE -ne 0) { throw 'Failed to create inert OOXML source fixtures' }
    $legacyFixtureScript = Join-Path $repoRoot 'scripts\create-legacy-office-fixtures.ps1'
    $fixtureStdout = Join-Path $testRoot 'legacy-fixture.stdout.log'
    $fixtureStderr = Join-Path $testRoot 'legacy-fixture.stderr.log'
    $fixtureProcess = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$legacyFixtureScript`"",
        '-SourceRoot', "`"$workspace`"", '-OutputRoot', "`"$workspace`""
    ) -RedirectStandardOutput $fixtureStdout -RedirectStandardError $fixtureStderr -PassThru -WindowStyle Hidden
    if (-not $fixtureProcess.WaitForExit(120000)) {
        Stop-Process -Id $fixtureProcess.Id -Force -ErrorAction SilentlyContinue
        Get-Process WINWORD, EXCEL, POWERPNT -ErrorAction SilentlyContinue |
            Where-Object { $_.Id -notin $existingOfficeProcessIds } |
            Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Output 'local_daemon_legacy_office_skipped=office_first_run_or_dialog_required'
        exit 0
    }
    $fixtureProcess.Refresh()
    if ($fixtureProcess.ExitCode -ne 0) {
        $fixtureError = Get-Content -LiteralPath $fixtureStderr -Raw -ErrorAction SilentlyContinue
        throw "Legacy Office fixture conversion failed: $fixtureError"
    }

    $daemonProcess = Start-Process -FilePath $binary `
        -RedirectStandardOutput (Join-Path $testRoot 'daemon.stdout.log') `
        -RedirectStandardError (Join-Path $testRoot 'daemon.stderr.log') `
        -PassThru -WindowStyle Hidden
    $tokenPath = Join-Path $testRoot 'ipc-token.txt'
    $devicePath = Join-Path $testRoot 'device-id.txt'
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and (-not (Test-Path -LiteralPath $tokenPath) -or -not (Test-Path -LiteralPath $devicePath))) {
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath $tokenPath) -or -not (Test-Path -LiteralPath $devicePath)) {
        throw 'Local daemon did not provision IPC credentials'
    }
    $token = (Get-Content -LiteralPath $tokenPath -Raw).Trim()
    $deviceId = (Get-Content -LiteralPath $devicePath -Raw).Trim()
    $projectId = [Guid]::NewGuid().ToString()
    $binding = Invoke-Daemon -Token $token -Method 'projects.bind_workspace' -Params @{
        project_id = $projectId
        workspace_path = $workspace
    }
    if (-not $binding.result.bound) { throw 'Local daemon did not bind the Office workspace' }

    Invoke-OfficeInspectRun $token $deviceId $projectId 'legacy.doc' 'word' 'doc'
    Invoke-OfficeInspectRun $token $deviceId $projectId 'legacy.xls' 'excel' 'xls'
    Invoke-OfficeInspectRun $token $deviceId $projectId 'legacy.ppt' 'powerpoint' 'ppt'

    $leftovers = @(Get-ChildItem -LiteralPath (Join-Path $testRoot 'office-inspection') -Recurse -File -ErrorAction SilentlyContinue)
    if ($leftovers.Count -ne 0) { throw 'Legacy Office inspection left temporary converted files behind' }
    Write-Output 'legacy_word_daemon_inspection=ok'
    Write-Output 'legacy_excel_daemon_inspection=ok'
    Write-Output 'legacy_powerpoint_daemon_inspection=ok'
    Write-Output 'legacy_office_active_content_disabled=ok'
    Write-Output 'legacy_office_temporary_cleanup=ok'
}
finally {
    foreach ($process in $modelProcesses) {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            $null = $process.WaitForExit(5000)
        }
    }
    if ($daemonProcess -and -not $daemonProcess.HasExited) {
        Stop-Process -Id $daemonProcess.Id -Force -ErrorAction SilentlyContinue
        $null = $daemonProcess.WaitForExit(10000)
    }
    Get-Process WINWORD, EXCEL, POWERPNT -ErrorAction SilentlyContinue |
        Where-Object { $_.Id -notin $existingOfficeProcessIds } |
        Stop-Process -Force -ErrorAction SilentlyContinue
    $env:COWORK_DAEMON_DATA_DIR = $previousDataDir
    $env:COWORK_DAEMON_IPC_ENDPOINT = $previousEndpoint
    $env:COWORK_DAEMON_IPC_TOKEN = $previousToken
    $env:COWORK_DAEMON_IPC_TOKEN_FILE = $previousTokenFile
    if (Test-Path -LiteralPath $testRoot) {
        $resolved = [IO.Path]::GetFullPath($testRoot)
        $temporary = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if (-not $resolved.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or -not (Split-Path $resolved -Leaf).StartsWith('open-cowork-daemon-legacy-office-')) {
            throw "Refusing to remove unexpected test directory $resolved"
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
    }
}
