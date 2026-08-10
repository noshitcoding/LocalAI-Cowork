$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class CoworkShutdownWindow {
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr FindWindow(string className, string windowName);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(
        IntPtr window, uint message, IntPtr wparam, IntPtr lparam,
        uint flags, uint timeout, out IntPtr result);
}
"@

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binary = Join-Path $repoRoot "target\release\cowork-local-daemon.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Build the release daemon before running this test: $binary"
}

$testId = [Guid]::NewGuid().ToString("N")
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) "open-cowork-daemon-lifecycle-$testId"
$pipeName = "open-cowork-daemon-lifecycle-$testId"
$endpoint = "\\.\pipe\$pipeName"
$process = $null
$second = $null
$modelProcess = $null
$quickModelProcess = $null
$toolModelProcess = $null
$previousDataDir = $env:COWORK_DAEMON_DATA_DIR
$previousEndpoint = $env:COWORK_DAEMON_IPC_ENDPOINT
$previousToken = $env:COWORK_DAEMON_IPC_TOKEN
$previousTokenFile = $env:COWORK_DAEMON_IPC_TOKEN_FILE
$previousDevice = $env:COWORK_DAEMON_DEVICE_ID
$previousModelBaseUrl = $env:COWORK_MODEL_BASE_URL
$previousModelName = $env:COWORK_MODEL_NAME

function Start-TestDaemon {
    param([string]$Suffix, [string[]]$Arguments = @())
    $start = @{
        FilePath = $binary
        RedirectStandardOutput = Join-Path $testRoot "stdout-$Suffix.txt"
        RedirectStandardError = Join-Path $testRoot "stderr-$Suffix.txt"
        PassThru = $true
        WindowStyle = 'Hidden'
    }
    if ($Arguments.Count -gt 0) { $start.ArgumentList = $Arguments }
    Start-Process @start
}

function Invoke-Daemon {
    param(
        [Parameter(Mandatory = $true)][string]$Token,
        [Parameter(Mandatory = $true)][string]$Method,
        $Params = $null
    )
    $pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
        ".",
        $pipeName,
        [System.IO.Pipes.PipeDirection]::InOut,
        [System.IO.Pipes.PipeOptions]::None
    )
    try {
        $pipe.Connect(5000)
        $writer = [System.IO.StreamWriter]::new($pipe, [System.Text.UTF8Encoding]::new($false), 4096, $true)
        $reader = [System.IO.StreamReader]::new($pipe, [System.Text.UTF8Encoding]::new($false), $false, 4096, $true)
        try {
            $writer.AutoFlush = $true
            $request = @{
                id = [Guid]::NewGuid().ToString()
                token = $Token
                method = $Method
                params = $Params
            } | ConvertTo-Json -Compress -Depth 30
            $writer.WriteLine($request)
            $response = $reader.ReadLine()
            if ([string]::IsNullOrWhiteSpace($response)) {
                throw "Daemon returned an empty response"
            }
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

try {
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    $env:COWORK_DAEMON_DATA_DIR = $testRoot
    $env:COWORK_DAEMON_IPC_ENDPOINT = $endpoint
    Remove-Item Env:COWORK_DAEMON_IPC_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:COWORK_DAEMON_IPC_TOKEN_FILE -ErrorAction SilentlyContinue
    Remove-Item Env:COWORK_DAEMON_DEVICE_ID -ErrorAction SilentlyContinue
    $workspace = Join-Path $testRoot 'workspace'
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    $probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $probe.Start(); $modelPort = ([Net.IPEndPoint]$probe.LocalEndpoint).Port; $probe.Stop()
    $powershellPath = (Get-Process -Id $PID).Path
    $listenerScript = Join-Path $PSScriptRoot 'test-local-daemon-model-listener.ps1'
    $modelProcess = Start-Process -FilePath $powershellPath -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$listenerScript`"",
        '-Port', $modelPort, '-Mode', 'stall', '-RequestCount', 1
    ) -PassThru -WindowStyle Hidden
    $env:COWORK_MODEL_BASE_URL = "http://127.0.0.1:$modelPort/v1"
    $env:COWORK_MODEL_NAME = 'shutdown-stall-model'

    $process = Start-TestDaemon -Suffix "primary"
    $tokenPath = Join-Path $testRoot "ipc-token.txt"
    $devicePath = Join-Path $testRoot "device-id.txt"
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        if ((Test-Path -LiteralPath $tokenPath) -and (Test-Path -LiteralPath $devicePath) -and -not $process.HasExited) {
            break
        }
        Start-Sleep -Milliseconds 100
        $process.Refresh()
    }
    if ($process.HasExited) {
        throw "Primary daemon exited unexpectedly with code $($process.ExitCode)"
    }
    if (-not (Test-Path -LiteralPath $tokenPath) -or -not (Test-Path -LiteralPath $devicePath)) {
        throw "Daemon did not provision its IPC credentials"
    }
    $token = (Get-Content -LiteralPath $tokenPath -Raw).Trim()
    $deviceId = (Get-Content -LiteralPath $devicePath -Raw).Trim()
    if ($token.Length -lt 64) { throw "Provisioned IPC token is too short" }
    $parsedDevice = [Guid]::Empty
    if (-not [Guid]::TryParse($deviceId, [ref]$parsedDevice) -or $parsedDevice -eq [Guid]::Empty) {
        throw "Provisioned device ID is invalid"
    }

    $health = Invoke-Daemon -Token $token -Method "health"
    if ($health.error -or $health.result.status -ne "ok" -or $health.result.device_id -ne $deviceId) {
        throw "Authenticated daemon health check failed"
    }
    $unauthorized = Invoke-Daemon -Token ("x" * 64) -Method "health"
    if ($unauthorized.error.code -ne "unauthorized" -or $unauthorized.result) {
        throw "Daemon IPC accepted an invalid token"
    }

    $entityId = [Guid]::NewGuid().ToString()
    $entityPayload = @{ title = 'Durable entity'; status = 'pending' }
    $entity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'task'
        id = $entityId
        payload = $entityPayload
        expected_revision = 0
    }
    if ($entity.result.revision -ne 1 -or $entity.result.etag -notlike '*:1"') {
        throw 'Daemon entity did not start at revision one'
    }
    $sameEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'task'
        id = $entityId
        payload = $entityPayload
        expected_revision = 1
    }
    if ($sameEntity.result.revision -ne 1) { throw 'Idempotent entity retry changed its revision' }
    $entityList = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{
        entity_type = 'task'
        include_tombstones = $false
    }
    if (@($entityList.result | Where-Object { $_.id -eq $entityId }).Count -ne 1) {
        throw 'Daemon entity list did not return the created task'
    }
    $entityChanges = Invoke-Daemon -Token $token -Method 'entities.changes' -Params @{ after = 0; limit = 20 }
    if (@($entityChanges.result.changes | Where-Object { $_.entity_id -eq $entityId }).Count -ne 1) {
        throw 'Daemon entity outbox was not idempotent'
    }
    $deletedEntity = Invoke-Daemon -Token $token -Method 'entities.delete' -Params @{
        entity_type = 'task'
        id = $entityId
        expected_revision = 1
    }
    if (-not $deletedEntity.result.tombstone -or $deletedEntity.result.revision -ne 2) {
        throw 'Daemon entity deletion did not create a revisioned tombstone'
    }

    $projectEntityId = [Guid]::NewGuid().ToString()
    $projectEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'project'
        id = $projectEntityId
        payload = @{
            title = 'Private durable project'
            instructions = 'Metadata survives without local paths.'
            thread_ids = @('thread-private')
            project_kind = 'private'
            files_location = 'personal_device'
        }
        expected_revision = 0
    }
    if ($projectEntity.result.revision -ne 1) {
        throw 'Private project metadata did not start at revision one'
    }
    if (($projectEntity.result.payload | ConvertTo-Json -Compress) -match 'workspace|resources|local_path') {
        throw 'Private project metadata unexpectedly contains local file information'
    }

    $threadEntityId = [Guid]::NewGuid().ToString()
    $messageEntityId = [Guid]::NewGuid().ToString()
    $threadEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'thread'
        id = $threadEntityId
        payload = @{
            title = 'Durable private chat'
            provider_settings = @{ backend = 'openai-compatible'; profileId = 'local-profile'; model = 'local-model' }
            runner = 'model'
        }
        expected_revision = 0
    }
    $messageEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'message'
        id = $messageEntityId
        payload = @{
            thread_id = $threadEntityId
            role = 'assistant'
            content = ''
            timestamp = 42
            attachment_descriptors = @(@{ kind = 'file'; label = 'report.xlsx'; availability = 'personal_device' })
        }
        expected_revision = 0
    }
    $finalMessageEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'message'
        id = $messageEntityId
        payload = @{
            thread_id = $threadEntityId
            role = 'assistant'
            content = 'Final durable answer'
            timestamp = 42
            attachment_descriptors = @(@{ kind = 'file'; label = 'report.xlsx'; availability = 'personal_device' })
        }
        expected_revision = 1
    }
    if ($threadEntity.result.revision -ne 1 -or $messageEntity.result.revision -ne 1 -or $finalMessageEntity.result.revision -ne 2) {
        throw 'Thread/message metadata revisions are invalid'
    }
    if (($finalMessageEntity.result.payload | ConvertTo-Json -Compress) -match '[A-Za-z]:[\\/]') {
        throw 'Synchronized message metadata unexpectedly contains a local path'
    }

    $crewEntityId = [Guid]::NewGuid().ToString()
    $providerEntityId = [Guid]::NewGuid().ToString()
    $mcpEntityId = [Guid]::NewGuid().ToString()
    $crewEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'crew'
        id = $crewEntityId
        payload = @{ definition = @{ id = $crewEntityId; name = 'Durable Crew'; tasks = @(); agents = @() } }
        expected_revision = 0
    }
    $providerEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'provider_profile'
        id = $providerEntityId
        payload = @{
            name = 'Laptop vLLM'
            preset = 'custom'
            model = 'qwen3:14b'
            endpoint_binding = 'per_device'
        }
        expected_revision = 0
    }
    $secretMetadataEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'secret_metadata'
        id = "provider:$providerEntityId"
        payload = @{
            owner_type = 'provider_profile'
            owner_id = $providerEntityId
            secret_kind = 'api_key'
            configured_on_source_device = $true
            value_included = $false
        }
        expected_revision = 0
    }
    $mcpEntity = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'mcp_metadata'
        id = $mcpEntityId
        payload = @{
            name = 'Private MCP'
            transport = 'stdio'
            executable_hint = 'mcp-server.exe'
            environment_keys = @('ACCESS_TOKEN')
            device_binding_required = $true
        }
        expected_revision = 0
    }
    if (@($crewEntity.result.revision, $providerEntity.result.revision, $secretMetadataEntity.result.revision, $mcpEntity.result.revision) -contains 0) {
        throw 'Crew/provider/MCP metadata did not persist at revision one'
    }
    $configurationMetadataJson = @(
        $crewEntity.result.payload,
        $providerEntity.result.payload,
        $secretMetadataEntity.result.payload,
        $mcpEntity.result.payload
    ) | ConvertTo-Json -Compress -Depth 10
    if ($configurationMetadataJson -match 'api[_-]?key.?[:=].?[A-Za-z0-9]' -or $configurationMetadataJson -match '[A-Za-z]:[\\/]') {
        throw 'Configuration metadata unexpectedly contains a secret value or local path'
    }

    $second = Start-TestDaemon -Suffix "duplicate"
    if (-not $second.WaitForExit(10000)) {
        throw "Duplicate daemon did not reject the second instance"
    }
    if ($second.ExitCode -eq 0) {
        throw "Duplicate daemon unexpectedly exited successfully"
    }

    Stop-Process -Id $process.Id -Force
    $null = $process.WaitForExit(10000)
    $process = Start-TestDaemon -Suffix "restart"
    $deadline = (Get-Date).AddSeconds(20)
    $health = $null
    do {
        try {
            $health = Invoke-Daemon -Token $token -Method "health"
            if ($health.result.status -eq "ok") { break }
        }
        catch {
            Start-Sleep -Milliseconds 100
        }
    } while ((Get-Date) -lt $deadline)
    if (-not $health -or $health.result.status -ne "ok") { throw "Daemon did not restart with persisted credentials" }
    if ((Get-Content -LiteralPath $tokenPath -Raw).Trim() -ne $token) {
        throw "Daemon rotated its local IPC token unexpectedly"
    }
    $persistedProjects = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{
        entity_type = 'project'
        include_tombstones = $false
    }
    $persistedProject = @($persistedProjects.result | Where-Object { $_.id -eq $projectEntityId })
    if ($persistedProject.Count -ne 1 -or $persistedProject[0].payload.thread_ids[0] -ne 'thread-private') {
        throw 'Private project metadata did not survive the daemon restart'
    }
    $persistedThreads = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{
        entity_type = 'thread'
        include_tombstones = $false
    }
    $persistedMessages = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{
        entity_type = 'message'
        include_tombstones = $false
    }
    if (@($persistedThreads.result | Where-Object { $_.id -eq $threadEntityId }).Count -ne 1) {
        throw 'Chat thread metadata did not survive the daemon restart'
    }
    $persistedMessage = @($persistedMessages.result | Where-Object { $_.id -eq $messageEntityId })
    if ($persistedMessage.Count -ne 1 -or $persistedMessage[0].revision -ne 2 -or $persistedMessage[0].payload.content -ne 'Final durable answer') {
        throw 'Final chat message state did not survive the daemon restart'
    }
    foreach ($entityCheck in @(
        @{ type = 'crew'; id = $crewEntityId },
        @{ type = 'provider_profile'; id = $providerEntityId },
        @{ type = 'secret_metadata'; id = "provider:$providerEntityId" },
        @{ type = 'mcp_metadata'; id = $mcpEntityId }
    )) {
        $persisted = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{
            entity_type = $entityCheck.type
            include_tombstones = $false
        }
        if (@($persisted.result | Where-Object { $_.id -eq $entityCheck.id }).Count -ne 1) {
            throw "Durable $($entityCheck.type) metadata did not survive the daemon restart"
        }
    }
    if ((Get-Content -LiteralPath $devicePath -Raw).Trim() -ne $deviceId) {
        throw "Daemon changed its persistent device ID unexpectedly"
    }

    $projectId = [Guid]::NewGuid().ToString()
    $threadId = [Guid]::NewGuid().ToString()
    $binding = Invoke-Daemon -Token $token -Method 'projects.bind_workspace' -Params @{
        project_id = $projectId
        workspace_path = $workspace
    }
    if (-not $binding.result.bound) { throw 'Daemon did not bind the shutdown-test workspace' }

    $quickProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $quickProbe.Start(); $quickModelPort = ([Net.IPEndPoint]$quickProbe.LocalEndpoint).Port; $quickProbe.Stop()
    $quickModelProcess = Start-Process -FilePath $powershellPath -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$listenerScript`"",
        '-Port', $quickModelPort, '-Mode', 'quick', '-RequestCount', 2
    ) -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 1000
    $modelSecret = "provider-secret-$testId"
    $detached = Invoke-Daemon -Token $token -Method 'runs.create' -Params @{
        thread_id = [Guid]::NewGuid().ToString()
        project_id = $projectId
        project_revision = 1
        project_privacy = 'private_local'
        task = $null
        executor_target = @{ kind = 'personal_device'; device_id = $deviceId }
        required_capabilities = @('model.external')
        input = @{
            prompt = 'Complete after this IPC client disconnects.'
            client_thread_id = 'desktop-thread'
            client_assistant_message_id = 'desktop-assistant'
        }
        model_profile_id = $null
        snapshot_id = $null
        idempotency_key = "detached-$testId"
        model_config = @{
            base_url = "http://127.0.0.1:$quickModelPort/v1"
            api_key = $modelSecret
            model = 'detached-model'
            timeout_ms = 30000
            max_steps = 4
            verify_tls_certificates = $true
        }
    }
    $detachedRunId = $detached.result.spec.id
    $deadline = (Get-Date).AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 100
        $detachedResult = Invoke-Daemon -Token $token -Method 'runs.get' -Params @{ run_id = $detachedRunId }
    } while ($detachedResult.result.state -notin @('completed', 'failed') -and (Get-Date) -lt $deadline)
    if ($detachedResult.result.state -ne 'completed' -or $detachedResult.result.result.content -ne 'detached client completed') {
        throw "Detached-client run did not complete: $($detachedResult.result | ConvertTo-Json -Compress -Depth 10)"
    }
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
    if ($databaseText.Contains($modelSecret)) {
                throw "Provider secret was stored in plaintext in $($databaseFile.Name)"
            }
        }
        finally {
            $stream.Dispose()
        }
    }

    $toolPort = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $toolPort.Start(); $toolModelPort = ([Net.IPEndPoint]$toolPort.LocalEndpoint).Port; $toolPort.Stop()
    $toolArguments = @{ title = 'Agent-created durable task'; description = 'Persist without the WebView.' } | ConvertTo-Json -Compress
    $toolArgumentsBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($toolArguments))
    $toolModelLog = Join-Path $testRoot 'tool-model.requests.log'
    $toolModelStdout = Join-Path $testRoot 'tool-model.stdout.log'
    $toolModelStderr = Join-Path $testRoot 'tool-model.stderr.log'
    $toolModelProcess = Start-Process -FilePath $powershellPath -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$listenerScript`"",
        '-Port', $toolModelPort, '-Mode', 'tool', '-RequestCount', 2,
        '-ToolName', 'TaskCreate', '-ToolArgumentsBase64', $toolArgumentsBase64,
        '-FinalContent', 'task-tool-completed', '-LogPath', "`"$toolModelLog`""
    ) -RedirectStandardOutput $toolModelStdout -RedirectStandardError $toolModelStderr -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 1000
    $toolRun = Invoke-Daemon -Token $token -Method 'runs.create' -Params @{
        thread_id = [Guid]::NewGuid().ToString()
        project_id = $projectId
        project_revision = 1
        project_privacy = 'private_local'
        task = $null
        executor_target = @{ kind = 'personal_device'; device_id = $deviceId }
        required_capabilities = @('task.manage')
        input = @{
            prompt = 'Create a durable task through the tool host.'
            tool_policy = 'autonomous'
            client_thread_id = 'tool-test-thread'
            client_assistant_message_id = 'tool-test-assistant'
        }
        model_profile_id = $null
        snapshot_id = $null
        idempotency_key = "tool-task-$testId"
        model_config = @{
            base_url = "http://127.0.0.1:$toolModelPort/v1"
            api_key = $null
            model = 'tool-driver'
            timeout_ms = 30000
            max_steps = 4
            verify_tls_certificates = $true
        }
    }
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 100
        $toolRunResult = Invoke-Daemon -Token $token -Method 'runs.get' -Params @{ run_id = $toolRun.result.spec.id }
    } while ($toolRunResult.result.state -notin @('completed', 'failed', 'interrupted') -and (Get-Date) -lt $deadline)
    if ($toolRunResult.result.state -ne 'completed' -or $toolRunResult.result.result.content -ne 'task-tool-completed') {
        $toolListenerDetails = @(
            Get-Content -LiteralPath $toolModelLog -Raw -ErrorAction SilentlyContinue
            Get-Content -LiteralPath $toolModelStderr -Raw -ErrorAction SilentlyContinue
            Get-Content -LiteralPath $toolModelStdout -Raw -ErrorAction SilentlyContinue
        ) -join "`n"
        throw "Daemon state tool run failed: $($toolRunResult.result | ConvertTo-Json -Compress -Depth 20) listener=$toolListenerDetails"
    }
    $agentTasks = Invoke-Daemon -Token $token -Method 'entities.list' -Params @{ entity_type = 'task'; include_tombstones = $false }
    if (@($agentTasks.result | Where-Object { $_.payload.title -eq 'Agent-created durable task' }).Count -ne 1) {
        throw 'TaskCreate did not persist its entity in the daemon'
    }
    if (-not $toolModelProcess.WaitForExit(10000)) { throw 'State-tool model listener did not stop' }
    $toolModelProcess.Refresh()
    if ($null -ne $toolModelProcess.ExitCode -and $toolModelProcess.ExitCode -ne 0) {
        throw "State-tool model listener exited with $($toolModelProcess.ExitCode)"
    }

    $scheduleId = [Guid]::NewGuid().ToString()
    $scheduledClientProjectId = "scheduled-project-$testId"
    $scheduledClientTaskId = "scheduled-task-$testId"
    $scheduledProviderId = "scheduled-provider-$testId"
    $scheduledMcpId = "scheduled-mcp-$testId"
    $scheduledMcpSecret = "mcp-secret-$testId"
    $scheduledTaskRuntimeId = [Guid]::NewGuid().ToString()
    $null = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'project'; id = $scheduledClientProjectId
        payload = @{ title = 'Scheduled project'; instructions = 'Old scheduled instructions.' }
        expected_revision = 0
    }
    $null = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'project'; id = $scheduledClientProjectId
        payload = @{ title = 'Scheduled project'; instructions = 'Current scheduled instructions.' }
        expected_revision = 1
    }
    $null = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'task'; id = $scheduledClientTaskId
        payload = @{ task_kind = 'work'; description = 'Old scheduled prompt'; expected_output = 'Old output' }
        expected_revision = 0
    }
    $null = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'task'; id = $scheduledClientTaskId
        payload = @{ task_kind = 'work'; description = 'Run the current scheduled task.'; expected_output = 'Current output' }
        expected_revision = 1
    }
    $null = Invoke-Daemon -Token $token -Method 'entities.upsert' -Params @{
        entity_type = 'provider_profile'; id = $scheduledProviderId
        payload = @{ name = 'Current scheduled provider'; model = 'scheduled-model'; timeout_ms = 30000; verify_tls_certificates = $true }
        expected_revision = 0
    }
    $providerBinding = Invoke-Daemon -Token $token -Method 'provider_bindings.upsert' -Params @{
        profile_id = $scheduledProviderId
        base_url = "http://127.0.0.1:$quickModelPort/v1"
        api_key = $modelSecret
    }
    if (-not $providerBinding.result.bound -or -not $providerBinding.result.has_api_key) {
        throw 'Daemon did not persist the encrypted per-device provider binding'
    }
    $providerBindingRead = Invoke-Daemon -Token $token -Method 'provider_bindings.get' -Params @{
        profile_id = $scheduledProviderId
    }
    if ($providerBindingRead.result.PSObject.Properties['api_key']) {
        throw 'Daemon exposed the provider secret through its binding metadata API'
    }
    $mcpBinding = Invoke-Daemon -Token $token -Method 'mcp_bindings.upsert' -Params @{
        server_id = $scheduledMcpId
        name = 'Current scheduled MCP'
        command = 'current-mcp-server.exe'
        args = @('--stdio', '--current')
        env = @{ MCP_TOKEN = $scheduledMcpSecret }
    }
    if (-not $mcpBinding.result.bound -or $mcpBinding.result.argument_count -ne 2) {
        throw 'Daemon did not persist the encrypted per-device MCP binding'
    }
    $mcpBindingRead = Invoke-Daemon -Token $token -Method 'mcp_bindings.get' -Params @{
        server_id = $scheduledMcpId
    }
    $mcpBindingMetadata = $mcpBindingRead.result | ConvertTo-Json -Compress -Depth 10
    if (
        $mcpBindingRead.result.PSObject.Properties['command'] -or
        $mcpBindingRead.result.PSObject.Properties['args'] -or
        $mcpBindingRead.result.PSObject.Properties['env'] -or
        $mcpBindingMetadata.Contains($scheduledMcpSecret)
    ) {
        throw 'Daemon exposed an MCP command, argument, or secret through its binding metadata API'
    }
    $scheduleParams = @{
        id = $scheduleId
        expression = '*/2 * * * * *'
        timezone = 'Europe/Berlin'
        enabled = $true
        run_request = @{
            thread_id = [Guid]::NewGuid().ToString()
            project_id = $projectId
            project_revision = 1
            project_privacy = 'private_local'
            task = @{ id = $scheduledTaskRuntimeId; revision = 1 }
            executor_target = @{ kind = 'personal_device'; device_id = $deviceId }
            required_capabilities = @('model.external')
            input = @{
                prompt = 'Run from the daemon scheduler.'
                client_thread_id = 'scheduled-desktop-thread'
                client_project_id = $scheduledClientProjectId
                client_task_id = $scheduledClientTaskId
                client_provider_profile_id = $scheduledProviderId
                client_mcp_server_ids = @($scheduledMcpId)
                resolve_current_versions = $true
                resolve_current_provider_binding = $true
                resolve_current_mcp_bindings = $true
                client_assistant_message_id = 'assigned-at-trigger'
            }
            model_profile_id = $null
            snapshot_id = $null
            idempotency_key = "schedule-template-$testId"
        }
        model_config = @{
            base_url = 'http://127.0.0.1:9/v1'
            api_key = $modelSecret
            model = 'scheduled-model'
            timeout_ms = 30000
            max_steps = 4
            verify_tls_certificates = $true
            mcp_servers = @(@{
                name = 'Frozen MCP'
                command = 'frozen-mcp-server.exe'
                args = @('--frozen')
                env = @{ MCP_TOKEN = 'frozen-mcp-secret' }
            })
        }
    }
    $schedule = Invoke-Daemon -Token $token -Method 'schedules.upsert' -Params $scheduleParams
    if (-not $schedule.result.next_run_at) { throw 'Daemon schedule did not calculate its next run' }
    Write-Output 'schedule_registered=ok'
    $deadline = (Get-Date).AddSeconds(20)
    $scheduledRun = $null
    do {
        Start-Sleep -Milliseconds 200
        $listedRuns = Invoke-Daemon -Token $token -Method 'runs.list' -Params @{ limit = 20 }
        $scheduledRun = @($listedRuns.result.items | Where-Object {
            $property = $_.spec.input.PSObject.Properties['schedule_id']
            $property -and $property.Value -eq $scheduleId
        }) | Select-Object -First 1
    } while ((-not $scheduledRun -or $scheduledRun.state -notin @('completed', 'failed')) -and (Get-Date) -lt $deadline)
    if (-not $scheduledRun -or $scheduledRun.state -ne 'completed' -or -not $scheduledRun.spec.input.scheduled) {
        throw "Daemon-owned schedule did not complete independently: $($scheduledRun | ConvertTo-Json -Compress -Depth 10)"
    }
    if (
        $scheduledRun.spec.project.revision -ne 2 -or
        $scheduledRun.spec.task.revision -ne 2 -or
        $scheduledRun.spec.input.prompt -ne 'Run the current scheduled task.' -or
        $scheduledRun.spec.input.current_project_instructions -ne 'Current scheduled instructions.' -or
        $scheduledRun.spec.input.resolved_entity_revisions.provider_profile -ne 1 -or
        $scheduledRun.spec.input.resolved_device_provider_binding -ne $true -or
        $scheduledRun.spec.input.resolved_device_mcp_bindings -ne $true -or
        $scheduledRun.spec.input.resolved_mcp_server_ids[0] -ne $scheduledMcpId
    ) {
        throw "Daemon schedule did not resolve current entity versions: $($scheduledRun.spec | ConvertTo-Json -Compress -Depth 12)"
    }
    Write-Output 'schedule_current_versions=ok'
    Write-Output 'schedule_trigger_completed=ok'
    $scheduleParams.enabled = $false
    $disabledSchedule = Invoke-Daemon -Token $token -Method 'schedules.upsert' -Params $scheduleParams
    if ($disabledSchedule.result.enabled -or $disabledSchedule.result.next_run_at) {
        throw 'Daemon schedule did not disable cleanly'
    }
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
            if ($databaseText.Contains($modelSecret) -or $databaseText.Contains($scheduledMcpSecret) -or $databaseText.Contains('frozen-mcp-secret')) {
                throw "A provider or MCP binding secret was stored in plaintext in $($databaseFile.Name)"
            }
        }
        finally {
            $stream.Dispose()
        }
    }
    Write-Output 'schedule_current_mcp_binding=ok'

    $created = Invoke-Daemon -Token $token -Method 'runs.create' -Params @{
        thread_id = $threadId
        project_id = $projectId
        project_revision = 1
        project_privacy = 'private_local'
        task = $null
        executor_target = @{ kind = 'personal_device'; device_id = $deviceId }
        required_capabilities = @('files')
        input = @{ prompt = 'Wait for the deliberately stalled model response.' }
        model_profile_id = $null
        snapshot_id = $null
        idempotency_key = "shutdown-$testId"
    }
    $runId = $created.result.spec.id
    $deadline = (Get-Date).AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 100
        $running = Invoke-Daemon -Token $token -Method 'runs.get' -Params @{ run_id = $runId }
    } while ($running.result.state -ne 'running' -and (Get-Date) -lt $deadline)
    if ($running.result.state -ne 'running') { throw "Shutdown-test run did not enter running state: $($running.result.state)" }
    $activeRuns = Invoke-Daemon -Token $token -Method 'runs.list_active'
    if (@($activeRuns.result.items | Where-Object { $_.spec.id -eq $runId }).Count -ne 1) {
        throw 'Active run listing omitted an older in-flight run'
    }

    $shutdownWindow = [CoworkShutdownWindow]::FindWindow('OpenCoworkLocalDaemonShutdownWindow', $null)
    if ($shutdownWindow -eq [IntPtr]::Zero) { throw 'Daemon did not create its Windows session-end window' }
    $shutdownResult = [IntPtr]::Zero
    $sent = [CoworkShutdownWindow]::SendMessageTimeout(
        $shutdownWindow,
        0x0011,
        [IntPtr]::Zero,
        [IntPtr]::Zero,
        0x0002,
        5000,
        [ref]$shutdownResult
    )
    if ($sent -eq [IntPtr]::Zero -or $shutdownResult.ToInt64() -ne 1) {
        throw 'Daemon did not accept the simulated WM_QUERYENDSESSION broadcast'
    }
    if (-not $process.WaitForExit(10000)) { throw 'Daemon did not complete its shutdown checkpoint' }

    $process = Start-TestDaemon -Suffix 'post-shutdown'
    $deadline = (Get-Date).AddSeconds(20)
    $health = $null
    do {
        try {
            $health = Invoke-Daemon -Token $token -Method 'health'
            if ($health.result.status -eq 'ok') { break }
        }
        catch { Start-Sleep -Milliseconds 100 }
    } while ((Get-Date) -lt $deadline)
    if (-not $health -or $health.result.status -ne 'ok') { throw 'Daemon did not restart after graceful shutdown' }
    $interrupted = Invoke-Daemon -Token $token -Method 'runs.get' -Params @{ run_id = $runId }
    if ($interrupted.result.state -ne 'interrupted' -or $interrupted.result.error.code -ne 'daemon_shutdown') {
        throw "Graceful shutdown did not persist the interrupted checkpoint: $($interrupted.result | ConvertTo-Json -Compress -Depth 10)"
    }
    $previousProcess = $process
    $process = Start-TestDaemon -Suffix 'replacement' -Arguments @('--replace')
    if (-not $previousProcess.WaitForExit(15000)) {
        throw 'Replacement daemon did not checkpoint and stop the prior version'
    }
    $deadline = (Get-Date).AddSeconds(20)
    $replacementHealth = $null
    do {
        try {
            $replacementHealth = Invoke-Daemon -Token $token -Method 'health'
            if ($replacementHealth.result.status -eq 'ok') { break }
        }
        catch { Start-Sleep -Milliseconds 100 }
    } while ((Get-Date) -lt $deadline)
    if (-not $replacementHealth -or $replacementHealth.result.status -ne 'ok') {
        throw 'Replacement daemon did not take over the stable IPC endpoint'
    }
    $ipcShutdown = Invoke-Daemon -Token $token -Method 'daemon.shutdown'
    if (-not $ipcShutdown.result.accepted) { throw 'Daemon rejected authenticated IPC shutdown request' }
    if (-not $process.WaitForExit(10000)) { throw 'Daemon did not stop after authenticated IPC shutdown' }

    Write-Output "credential_self_provisioning=ok"
    Write-Output "authenticated_named_pipe=ok"
    Write-Output "single_instance_lock=ok"
    Write-Output "restart_identity_persistence=ok"
    Write-Output "graceful_shutdown_checkpoint=ok"
    Write-Output "shutdown_interruption_recovery=ok"
    Write-Output "windows_session_end_signal=ok"
    Write-Output "authenticated_ipc_shutdown=ok"
    Write-Output "in_place_daemon_replacement=ok"
    Write-Output "detached_client_run_continuation=ok"
    Write-Output "encrypted_run_model_config=ok"
    Write-Output "daemon_owned_schedule=ok"
    Write-Output "schedule_timezone=ok"
    Write-Output "revisioned_entity_sync=ok"
    Write-Output "private_project_metadata_persistence=ok"
    Write-Output "durable_chat_metadata_persistence=ok"
    Write-Output "durable_configuration_metadata_persistence=ok"
    Write-Output "complete_active_run_reconciliation=ok"
    Write-Output "daemon_owned_state_tool_dispatch=ok"
}
finally {
    foreach ($candidate in @($process, $second)) {
        if ($candidate -and -not $candidate.HasExited) {
            Stop-Process -Id $candidate.Id -Force -ErrorAction SilentlyContinue
            $null = $candidate.WaitForExit(10000)
        }
    }
    foreach ($listenerProcess in @($modelProcess, $quickModelProcess, $toolModelProcess)) {
        if ($listenerProcess -and -not $listenerProcess.HasExited) {
            Stop-Process -Id $listenerProcess.Id -Force -ErrorAction SilentlyContinue
            $null = $listenerProcess.WaitForExit(5000)
        }
    }
    $env:COWORK_DAEMON_DATA_DIR = $previousDataDir
    $env:COWORK_DAEMON_IPC_ENDPOINT = $previousEndpoint
    $env:COWORK_DAEMON_IPC_TOKEN = $previousToken
    $env:COWORK_DAEMON_IPC_TOKEN_FILE = $previousTokenFile
    $env:COWORK_DAEMON_DEVICE_ID = $previousDevice
    $env:COWORK_MODEL_BASE_URL = $previousModelBaseUrl
    $env:COWORK_MODEL_NAME = $previousModelName

    if (Test-Path -LiteralPath $testRoot) {
        $resolvedRoot = [System.IO.Path]::GetFullPath($testRoot)
        $tempPrefix = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
        if (-not $resolvedRoot.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove test directory outside the temporary root: $resolvedRoot"
        }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
