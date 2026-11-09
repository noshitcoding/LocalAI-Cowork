$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$binary = Join-Path $repoRoot 'target\release\cowork-local-daemon.exe'
$crewScript = Join-Path $repoRoot 'app\src-tauri\python\crew_runtime\main.py'
$crewPython = Join-Path $env:APPDATA 'io.noshitcoding.opencowork\crew-runtime\venv\Scripts\python.exe'
foreach ($required in @($binary, $crewScript, $crewPython)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required Crew daemon test file is missing: $required"
    }
}

$testId = [Guid]::NewGuid().ToString('N')
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "open-cowork-daemon-crew-$testId"
$pipeName = "open-cowork-daemon-crew-$testId"
$daemon = $null
$model = $null
$completed = $false
$previousEnvironment = @{
    COWORK_DAEMON_DATA_DIR = $env:COWORK_DAEMON_DATA_DIR
    COWORK_DAEMON_IPC_ENDPOINT = $env:COWORK_DAEMON_IPC_ENDPOINT
    COWORK_DAEMON_IPC_TOKEN = $env:COWORK_DAEMON_IPC_TOKEN
    COWORK_DAEMON_DEVICE_ID = $env:COWORK_DAEMON_DEVICE_ID
    COWORK_CREW_PYTHON = $env:COWORK_CREW_PYTHON
    COWORK_CREW_SCRIPT = $env:COWORK_CREW_SCRIPT
}

function Invoke-Daemon {
    param(
        [Parameter(Mandatory = $true)][string]$Token,
        [Parameter(Mandatory = $true)][string]$Method,
        $Params = $null
    )
    $pipe = [IO.Pipes.NamedPipeClientStream]::new(
        '.', $pipeName, [IO.Pipes.PipeDirection]::InOut, [IO.Pipes.PipeOptions]::None
    )
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
            } | ConvertTo-Json -Compress -Depth 40))
            $raw = $reader.ReadLine()
            if ([string]::IsNullOrWhiteSpace($raw)) { throw 'Daemon returned an empty response' }
            $response = $raw | ConvertFrom-Json
            if ($response.error) {
                throw "Daemon $Method failed: $($response.error | ConvertTo-Json -Compress -Depth 10)"
            }
            return $response.result
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

try {
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    $workspace = Join-Path $testRoot 'workspace'
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    $probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $probe.Start()
    $modelPort = ([Net.IPEndPoint]$probe.LocalEndpoint).Port
    $probe.Stop()
    $powershell = (Get-Process -Id $PID).Path
    $model = Start-Process -FilePath $powershell -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
        "`"$(Join-Path $PSScriptRoot 'test-local-daemon-model-listener.ps1')`"",
        '-Port', $modelPort, '-Mode', 'quick', '-RequestCount', 8,
        '-LogPath', "`"$(Join-Path $testRoot 'model-requests.log')`""
    ) -PassThru -WindowStyle Hidden

    $env:COWORK_DAEMON_DATA_DIR = $testRoot
    $env:COWORK_DAEMON_IPC_ENDPOINT = "\\.\pipe\$pipeName"
    Remove-Item Env:COWORK_DAEMON_IPC_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:COWORK_DAEMON_DEVICE_ID -ErrorAction SilentlyContinue
    $env:COWORK_CREW_PYTHON = $crewPython
    $env:COWORK_CREW_SCRIPT = $crewScript
    $daemon = Start-Process -FilePath $binary `
        -RedirectStandardOutput (Join-Path $testRoot 'daemon-stdout.txt') `
        -RedirectStandardError (Join-Path $testRoot 'daemon-stderr.txt') `
        -PassThru -WindowStyle Hidden

    $tokenPath = Join-Path $testRoot 'ipc-token.txt'
    $devicePath = Join-Path $testRoot 'device-id.txt'
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and (-not (Test-Path $tokenPath) -or -not (Test-Path $devicePath))) {
        if ($daemon.HasExited) { throw "Crew daemon exited with $($daemon.ExitCode)" }
        Start-Sleep -Milliseconds 100
    }
    $token = (Get-Content -LiteralPath $tokenPath -Raw).Trim()
    $deviceId = (Get-Content -LiteralPath $devicePath -Raw).Trim()
    $projectId = [Guid]::NewGuid().ToString()
    $threadId = [Guid]::NewGuid().ToString()
    $null = Invoke-Daemon -Token $token -Method 'projects.bind_workspace' -Params @{
        project_id = $projectId
        workspace_path = $workspace
    }

    $providerSecret = "crew-provider-$testId"
    $streamId = "crew-stream-$testId"
    $crewRequest = @{
        id = "crew-$testId"
        streamId = $streamId
        name = 'Detached Crew smoke test'
        description = 'Proves that CrewAI is owned by the local daemon.'
        process = 'sequential'
        verbose = $false
        maxRpm = 60
        maxParallelTasks = 1
        stopOnFailure = $true
        retryCount = 0
        managerReviewEnabled = $false
        shareAllTaskOutputs = $true
        sharedOutputCharLimit = 20000
        providerConfigs = @{
            openAICompatible = @{
                baseUrl = "http://127.0.0.1:$modelPort/v1"
                model = 'crew-smoke-model'
                models = @('crew-smoke-model')
                apiKey = $providerSecret
                timeoutMs = 120000
                verifyTlsCertificates = $true
            }
        }
        agents = @(@{
            id = 'agent-1'
            name = 'Runtime tester'
            role = 'Test executor'
            goal = 'Return the deterministic model result.'
            backstory = 'You verify detached Crew execution.'
            providerKind = 'openai-compatible'
            tools = @()
            mcpServerNames = @()
            enabled = $true
            allowDelegation = $false
            verbose = $false
            maxIterations = 2
        })
        tasks = @(@{
            id = 'crew-task-1'
            description = 'Return a brief successful smoke test result.'
            expectedOutput = 'A brief result.'
            agentId = 'agent-1'
            context = @()
            dependencies = @()
            asyncExecution = $false
        })
        config = @{ baseUrl = 'http://127.0.0.1:11434'; model = ''; timeoutMs = 120000 }
        cwd = $workspace
        authorizedPaths = @(@{ path = $workspace; kind = 'folder'; access = 'read_write'; isPrimary = $true })
    }
    $created = Invoke-Daemon -Token $token -Method 'runs.create' -Params @{
        thread_id = $threadId
        project_id = $projectId
        project_revision = 1
        project_privacy = 'private_local'
        task = $null
        executor_target = @{ kind = 'personal_device'; device_id = $deviceId }
        required_capabilities = @('crew.python', 'files')
        input = @{
            prompt = 'Run the detached Crew smoke test.'
            client_thread_id = 'desktop-crew-thread'
            client_assistant_message_id = 'desktop-crew-result'
            client_crew_live_message_id = 'desktop-crew-monitor'
            crew_stream_id = $streamId
            source = 'crew_task'
        }
        model_profile_id = $null
        snapshot_id = $null
        idempotency_key = "crew-smoke-$testId"
        model_config = @{
            base_url = "http://127.0.0.1:$modelPort/v1"
            api_key = $null
            model = 'crew-smoke-model'
            timeout_ms = 120000
            max_steps = 1
            verify_tls_certificates = $true
            crew_request = $crewRequest
        }
    }
    $runId = $created.spec.id

    # Every invocation opens and closes a fresh IPC connection. The Crew process
    # therefore has no client/WebView lifetime to depend on while it completes.
    $deadline = (Get-Date).AddSeconds(150)
    do {
        Start-Sleep -Milliseconds 250
        $run = Invoke-Daemon -Token $token -Method 'runs.get' -Params @{ run_id = $runId }
    } while ($run.state -notin @('completed', 'failed', 'canceled', 'interrupted') -and (Get-Date) -lt $deadline)
    if ($run.state -ne 'completed') {
        $stderr = Get-Content -LiteralPath (Join-Path $testRoot 'daemon-stderr.txt') -Raw
        throw "Detached Crew run failed: $($run | ConvertTo-Json -Compress -Depth 20); daemon=$stderr"
    }
    if (-not $run.result.crew_response -or $run.result.crew_response.status -ne 'completed') {
        throw "Crew response was not retained in the durable result: $($run.result | ConvertTo-Json -Compress -Depth 20)"
    }
    $events = Invoke-Daemon -Token $token -Method 'runs.events' -Params @{ run_id = $runId; after = 0 }
    $crewEvents = @($events | Where-Object {
        $_.kind -eq 'model_delta' -and $_.payload.adapter -eq 'crewai' -and $_.payload.crew_event.localAiCoworkEvent -eq 'crew_log'
    })
    if ($crewEvents.Count -lt 1) { throw 'Daemon did not persist Crew live events' }
    foreach ($databaseFile in Get-ChildItem -LiteralPath $testRoot -Filter 'daemon.sqlite3*' -File) {
        $stream = [IO.File]::Open(
            $databaseFile.FullName,
            [IO.FileMode]::Open,
            [IO.FileAccess]::Read,
            [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
        )
        try {
            $memory = [IO.MemoryStream]::new()
            $stream.CopyTo($memory)
            $databaseText = [Text.Encoding]::UTF8.GetString($memory.ToArray())
            $memory.Dispose()
            if ($databaseText.Contains($providerSecret)) {
                throw "Crew provider secret is plaintext in $($databaseFile.Name)"
            }
        }
        finally {
            $stream.Dispose()
        }
    }

    $null = Invoke-Daemon -Token $token -Method 'daemon.shutdown'
    if (-not $daemon.WaitForExit(10000)) { throw 'Crew daemon did not shut down' }
    Write-Output 'detached_crew_run=ok'
    Write-Output 'crew_live_events=ok'
    Write-Output 'encrypted_crew_provider_config=ok'
    $completed = $true
}
catch {
    Write-Output "crew_test_root=$testRoot"
    foreach ($diagnostic in @('daemon-stdout.txt', 'daemon-stderr.txt', 'model-requests.log')) {
        $diagnosticPath = Join-Path $testRoot $diagnostic
        if (Test-Path -LiteralPath $diagnosticPath) {
            Write-Output "--- $diagnostic ---"
            Get-Content -LiteralPath $diagnosticPath -Raw
        }
    }
    Write-Error $_
    throw
}
finally {
    foreach ($process in @($daemon, $model)) {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            $null = $process.WaitForExit(5000)
        }
    }
    foreach ($name in $previousEnvironment.Keys) {
        $value = $previousEnvironment[$name]
        if ($null -eq $value) {
            Remove-Item "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item "Env:$name" $value
        }
    }
    if ($completed -and (Test-Path -LiteralPath $testRoot)) {
        $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
        $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if (-not $resolvedRoot.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove Crew test directory outside the temporary root: $resolvedRoot"
        }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
