$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_personal_bridge_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$daemonData = Join-Path $testRoot 'daemon'
$projectWorkspace = Join-Path $testRoot 'workspace'
$serverProcess = $null
$daemonProcess = $null
$agentProcess = $null
$modelProcess = $null
$forwardProcesses = @()
$serverPort = 18094
$modelPort = 18095
$forwardPort = 18096
$deviceId = [guid]::NewGuid()
$userDeviceId = [guid]::NewGuid()
$pipeName = "open-cowork-bridge-$([guid]::NewGuid().ToString('N'))"
$pipeEndpoint = "\\.\pipe\$pipeName"
$daemonToken = -join ((1..64) | ForEach-Object { '0123456789abcdef'[(Get-Random -Maximum 16)] })
$agentTokenPath = Join-Path $testRoot 'agent-token.txt'
$daemonTokenPath = Join-Path $testRoot 'daemon-token.txt'
New-Item -ItemType Directory -Path $daemonData,$projectWorkspace -Force | Out-Null
[IO.File]::WriteAllText($daemonTokenPath, $daemonToken)
[IO.File]::WriteAllText((Join-Path $projectWorkspace '.coworkignore'), "ignored-secret.txt`n")
[IO.File]::WriteAllText((Join-Path $projectWorkspace 'ignored-secret.txt'), 'must remain local')

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 200
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{
    Method = $method
    Uri = "http://127.0.0.1:$serverPort/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 40)
  }
  Invoke-RestMethod @parameters
}

function Get-SseEvents([string]$path, [string]$token) {
  $response = Invoke-WebRequest -UseBasicParsing `
    -Uri "http://127.0.0.1:$serverPort/api/v1$path" `
    -Headers @{ authorization = "Bearer $token"; accept = 'text/event-stream' }
  $parsed = @()
  foreach ($line in ($response.Content -split "`r?`n")) {
    if ($line.StartsWith('data:')) {
      $parsed += ($line.Substring(5).TrimStart() | ConvertFrom-Json)
    }
  }
  return $parsed
}

function Invoke-Daemon([string]$method, $params) {
  $pipe = [IO.Pipes.NamedPipeClientStream]::new(
    '.', $pipeName, [IO.Pipes.PipeDirection]::InOut,
    [IO.Pipes.PipeOptions]::Asynchronous
  )
  try {
    $pipe.Connect(5000)
    $writer = [IO.StreamWriter]::new($pipe, [Text.UTF8Encoding]::new($false), 4096, $true)
    $reader = [IO.StreamReader]::new($pipe, [Text.UTF8Encoding]::new($false), $false, 4096, $true)
    try {
      $requestId = [guid]::NewGuid().ToString()
      $writer.WriteLine((@{ id = $requestId; token = $daemonToken; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 40))
      $writer.Flush()
      $response = $reader.ReadLine() | ConvertFrom-Json
      if ($response.id -ne $requestId) { throw 'local daemon response ID mismatch' }
      if ($response.error) { throw "local daemon $($response.error.code): $($response.error.message)" }
      return $response.result
    } finally {
      $reader.Dispose()
      $writer.Dispose()
    }
  } finally {
    $pipe.Dispose()
  }
}

function Wait-Run([string]$runId, [string]$token, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 250
    $run = Invoke-Json GET "/runs/$runId" $null $token
  } while ($run.state -notin @('completed','failed','interrupted','canceled','expired') -and (Get-Date) -lt $deadline)
  if ($run.state -notin @('completed','failed','interrupted','canceled','expired')) {
    throw "run $runId did not finish; state=$($run.state)"
  }
  return $run
}

function Start-ServerForwarder([int]$attempt) {
  $env:COWORK_FORWARD_LISTEN_HOST = '127.0.0.1'
  $env:COWORK_FORWARD_LISTEN_PORT = $forwardPort.ToString()
  $env:COWORK_FORWARD_TARGET_HOST = '127.0.0.1'
  $env:COWORK_FORWARD_TARGET_PORT = $serverPort.ToString()
  return Start-Process node -ArgumentList ('"{0}"' -f (Join-Path $workspace 'scripts/tcp-forwarder.mjs')) `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot "forward-$attempt.stdout.log") `
    -RedirectStandardError (Join-Path $testRoot "forward-$attempt.stderr.log")
}

try {
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server -p cowork-local-daemon -p cowork-device-agent | Out-Host } finally { Pop-Location }

  $env:COWORK_FAKE_MODEL_PORT = $modelPort.ToString()
  $env:COWORK_FAKE_MODEL_DELAY_MS = '1500'
  $modelProcess = Start-Process python -ArgumentList ('"{0}"' -f (Join-Path $workspace 'scripts/fake-personal-device-model.py')) `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'model.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'model.stderr.log')
  Wait-Http "http://127.0.0.1:$modelPort/healthz" 15

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = "127.0.0.1:$serverPort"
  $env:COWORK_SERVER_CAPABILITIES = 'model.external'
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  Remove-Item Env:COWORK_RUNNER_URL -ErrorAction SilentlyContinue
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $serverProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http "http://127.0.0.1:$serverPort/readyz" 30
  $forwardProcess = Start-ServerForwarder 1
  $forwardProcesses += $forwardProcess
  Wait-Http "http://127.0.0.1:$forwardPort/readyz" 15

  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'personal-bridge@opencowork.invalid'
    display_name = 'Personal Bridge E2E'
    password = 'Personal-Bridge-Password-42!'
    device_id = $userDeviceId.ToString()
  } $env:COWORK_BOOTSTRAP_TOKEN
  $project = Invoke-Json POST '/projects' @{
    name = 'Personal daemon bridge'
    description = 'Disposable bridge project'
    privacy = 'private_local'
    team_id = $null
    preferred_executor_target = @{ kind = 'personal_device'; device_id = $deviceId.ToString() }
    policy = @{ tool_policy = 'autonomous' }
  } $admin.access_token
  $thread = Invoke-Json POST '/threads' @{
    project_id = $project.id
    title = 'Personal daemon bridge'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $admin.access_token
  Invoke-Json POST '/executors' @{
    schema_version = 2
    executor_id = $deviceId.ToString()
    kind = 'personal_device'
    pool_id = $null
    owner_user_id = $null
    display_name = 'Personal bridge device'
    protocol_version = 2
    capabilities = @(
      @{ schema_version = 2; name = 'model.ollama'; version = 'e2e'; attributes = @{} },
      @{ schema_version = 2; name = 'files'; version = 'e2e'; attributes = @{} }
    )
    labels = @{ os = 'windows' }
    personal_device_remote_control = 'off'
    max_concurrent_runs = 1
  } $admin.access_token | Out-Null
  $credential = Invoke-Json POST "/executors/$deviceId/credentials" @{
    label = 'Personal bridge E2E'
    expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $admin.access_token
  [IO.File]::WriteAllText($agentTokenPath, $credential.token)

  $env:COWORK_DAEMON_DATA_DIR = $daemonData
  $env:COWORK_DAEMON_IPC_ENDPOINT = $pipeEndpoint
  $env:COWORK_DAEMON_IPC_TOKEN_FILE = $daemonTokenPath
  $env:COWORK_DAEMON_DEVICE_ID = $deviceId.ToString()
  $env:COWORK_DAEMON_USER_ID = $admin.user_id
  $env:COWORK_MODEL_BASE_URL = "http://127.0.0.1:$modelPort/v1"
  $env:COWORK_MODEL_NAME = 'personal-bridge-model'
  $daemonProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-local-daemon.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'daemon.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'daemon.stderr.log')
  $daemonDeadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 200
    try { $health = Invoke-Daemon 'health' @{} } catch { $health = $null }
  } while (-not $health -and (Get-Date) -lt $daemonDeadline)
  if (-not $health -or $health.device_id -ne $deviceId.ToString()) { throw 'local daemon did not become ready with the executor identity' }
  Invoke-Daemon 'projects.bind_workspace' @{
    project_id = $project.id
    workspace_path = $projectWorkspace
  } | Out-Null
  $localMemoryId = [Guid]::NewGuid()
  $localMemory = Invoke-Daemon 'entities.upsert' @{
    entity_type = 'memory'
    id = $localMemoryId.ToString()
    payload = @{ content = 'Created offline before the background agent connected'; scope = 'user' }
    expected_revision = 0
  }

  $env:COWORK_SERVER_URL = "http://127.0.0.1:$forwardPort"
  $env:COWORK_AGENT_TOKEN_FILE = $agentTokenPath
  $env:COWORK_EXECUTOR_ID = $deviceId.ToString()
  $env:COWORK_AGENT_KIND = 'personal_device'
  $env:COWORK_EXECUTOR_NAME = 'Personal bridge device'
  $env:COWORK_AGENT_CAPABILITIES = 'model.ollama,files'
  $env:COWORK_PERSONAL_REMOTE_CONTROL = 'off'
  $env:COWORK_LOCAL_DAEMON_IPC_ENDPOINT = $pipeEndpoint
  $env:COWORK_LOCAL_DAEMON_IPC_TOKEN_FILE = $daemonTokenPath
  $env:COWORK_AGENT_WORKSPACE_ROOT = (Join-Path $testRoot 'agent-workspaces')
  $agentProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-device-agent.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'agent.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'agent.stderr.log')

  $onlineDeadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 250
    $catalog = Invoke-Json GET '/capabilities' $null $admin.access_token
    $registered = @($catalog.executors | Where-Object { $_.registration.executor_id -eq $deviceId.ToString() })
    $bridgeLabel = $null
    if ($registered.Count -eq 1) {
      foreach ($labelProperty in $registered[0].registration.labels.PSObject.Properties) {
        if ($labelProperty.Name -eq 'local_runtime_bridge') {
          $bridgeLabel = $labelProperty.Value
          break
        }
      }
    }
  } while (($registered.Count -ne 1 -or -not $registered[0].online -or $bridgeLabel -ne 'enabled') -and (Get-Date) -lt $onlineDeadline)
  if ($registered.Count -ne 1 -or -not $registered[0].online) { throw 'personal device agent did not register online' }
  if ($bridgeLabel -ne 'enabled') { throw 'executor did not advertise the local runtime bridge' }

  $syncDeadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 250
    $serverMemories = Invoke-Json GET '/sync/entities/memory?limit=100' $null $admin.access_token
    $serverMemory = @($serverMemories.items | Where-Object { $_.entity_id -eq $localMemoryId.ToString() })
  } while ($serverMemory.Count -ne 1 -and (Get-Date) -lt $syncDeadline)
  if ($serverMemory.Count -ne 1 -or $serverMemory[0].payload.content -ne $localMemory.result.payload.content) {
    throw 'background personal executor did not drain the daemon metadata outbox'
  }

  $serverSkillId = [Guid]::NewGuid()
  Invoke-Json POST '/sync/changes' @{
    changes = @(@{
      schema_version = 2
      operation_id = [Guid]::NewGuid().ToString()
      device_id = $userDeviceId.ToString()
      entity_type = 'skill'
      entity_id = $serverSkillId.ToString()
      base_revision = 0
      operation = 'upsert'
      payload = @{ name = 'Created on the server for the daemon inbox'; description = 'background sync' }
      client_timestamp = (Get-Date).ToUniversalTime().ToString('o')
    })
  } $admin.access_token | Out-Null
  $inboxDeadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 250
    $daemonSkills = Invoke-Daemon 'entities.list' @{ entity_type = 'skill'; include_tombstones = $true }
    $daemonSkill = @($daemonSkills | Where-Object { $_.id -eq $serverSkillId.ToString() })
  } while ($daemonSkill.Count -ne 1 -and (Get-Date) -lt $inboxDeadline)
  if ($daemonSkill.Count -ne 1 -or $daemonSkill[0].payload.name -ne 'Created on the server for the daemon inbox') {
    throw 'background personal executor did not apply the server metadata inbox'
  }
  $syncPeerId = "http://127.0.0.1:$forwardPort#$deviceId"
  $syncState = Invoke-Daemon 'sync.state' @{ peer_id = $syncPeerId }
  $syncConflicts = Invoke-Daemon 'sync.conflicts' @{ peer_id = $syncPeerId }
  if ($syncState.local_cursor -lt 1 -or $syncState.remote_cursor -lt 1 -or @($syncConflicts).Count -ne 0) {
    throw 'background metadata cursors did not advance cleanly'
  }

  $run = Invoke-Json POST '/runs' @{
    thread_id = $thread.id
    project_id = $project.id
    project_revision = $project.revision
    project_privacy = 'private_local'
    task = $null
    executor_target = @{ kind = 'personal_device'; device_id = $deviceId.ToString() }
    required_capabilities = @('files')
    input = @{ prompt = 'Write the deterministic bridge result.'; tool_policy = 'autonomous' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $admin.access_token
  $modelStartedDeadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 100
    $localEvents = @(Invoke-Daemon 'runs.events' @{ run_id = $run.spec.id; after = 0 })
  } while (@($localEvents | Where-Object { $_.kind -eq 'model_started' }).Count -eq 0 -and (Get-Date) -lt $modelStartedDeadline)
  if (@($localEvents | Where-Object { $_.kind -eq 'model_started' }).Count -eq 0) {
    try { $localState = Invoke-Daemon 'runs.get' @{ run_id = $run.spec.id } } catch { $localState = $null }
    $serverState = Invoke-Json GET "/runs/$($run.spec.id)" $null $admin.access_token
    $diagnosticCatalog = Invoke-Json GET '/capabilities' $null $admin.access_token
    throw "local daemon did not begin the model call before the recovery test; local=$($localState | ConvertTo-Json -Compress -Depth 20); server=$($serverState | ConvertTo-Json -Compress -Depth 20); executors=$($diagnosticCatalog.executors | ConvertTo-Json -Compress -Depth 20); events=$($localEvents | ConvertTo-Json -Compress -Depth 20)"
  }
  Stop-Process -Id $forwardProcess.Id -Force
  $forwardProcess.WaitForExit()
  Start-Sleep -Seconds 4
  $forwardProcess = Start-ServerForwarder 2
  $forwardProcesses += $forwardProcess
  Wait-Http "http://127.0.0.1:$forwardPort/readyz" 15
  $completed = Wait-Run $run.spec.id $admin.access_token 90
  if ($completed.state -ne 'completed' -or $completed.result.content -ne 'Personal daemon bridge completed.') {
    throw "personal bridge run failed: $($completed | ConvertTo-Json -Compress -Depth 20)"
  }
  if (-not $completed.result.project_version_id -or -not $completed.result.result_snapshot_manifest_id) {
    throw 'completed personal run did not publish a reviewable result project version'
  }
  $versions = @(Invoke-Json GET "/projects/$($project.id)/versions" $null $admin.access_token)
  $resultVersion = @($versions | Where-Object { $_.id -eq $completed.result.project_version_id })
  if ($resultVersion.Count -ne 1 -or $resultVersion[0].created_by_run_id -ne $run.spec.id) {
    throw 'result project version is not linked to the originating run'
  }
  $resultSnapshot = Invoke-Json GET "/snapshots/$($completed.result.result_snapshot_manifest_id)" $null $admin.access_token
  $resultPaths = @($resultSnapshot.files | ForEach-Object { $_.path })
  if ($resultPaths -notcontains 'bridge-result.txt' -or $resultPaths -contains 'ignored-secret.txt') {
    throw "result snapshot violated the .coworkignore boundary: $($resultPaths -join ',')"
  }
  if ($completed.result.project_version_id -eq $project.current_version_id) {
    throw 'run result was applied automatically instead of waiting for review'
  }
  $resultPath = Join-Path $projectWorkspace 'bridge-result.txt'
  if ([IO.File]::ReadAllText($resultPath) -ne "written by the shared local daemon runtime`n") {
    throw 'shared local daemon runtime did not write the bound project file'
  }
  $localRun = Invoke-Daemon 'runs.get' @{ run_id = $run.spec.id }
  if ($localRun.spec.id -ne $run.spec.id -or $localRun.state -ne 'completed') {
    throw 'server and local daemon did not preserve one run identity and terminal state'
  }
  $events = Get-SseEvents "/runs/$($run.spec.id)/events" $admin.access_token
  $eventKinds = @($events | ForEach-Object { $_.kind })
  foreach ($kind in @('model_started','tool_started','tool_completed','model_completed','completed')) {
    if ($eventKinds -notcontains $kind) {
      throw "server run is missing relayed event $kind; received=$($eventKinds -join ',')"
    }
  }
  $localFinalEvents = @(Invoke-Daemon 'runs.events' @{ run_id = $run.spec.id; after = 0 })
  foreach ($localEvent in @($localFinalEvents | Where-Object { $_.kind -in @('model_started','model_delta','tool_started','tool_completed','tool_failed','model_completed','warning') })) {
    if (@($events | Where-Object { $_.event_id -eq $localEvent.event_id }).Count -ne 1) {
      throw "local event $($localEvent.event_id) was not relayed exactly once"
    }
  }
  if (@($events | Where-Object { $_.kind -eq 'executor_heartbeat' -and $_.payload.lease_recovered }).Count -lt 1) {
    throw 'server did not record recovery of the existing executor lease'
  }
  $checkpoints = Invoke-Json GET "/runs/$($run.spec.id)/checkpoints" $null $admin.access_token
  if (@($checkpoints).Count -lt 2 -or @($checkpoints | Where-Object { -not $_.safe_to_resume }).Count -lt 1 -or @($checkpoints | Where-Object { $_.safe_to_resume }).Count -lt 1) {
    throw 'server did not persist both unsafe dispatch and safe completion checkpoints'
  }

  Write-Output 'personal_device_shared_runtime=ok'
  Write-Output 'personal_device_run_identity=ok'
  Write-Output 'personal_device_tool_events=ok'
  Write-Output 'personal_device_checkpoint_relay=ok'
  Write-Output 'personal_device_local_workspace=ok'
  Write-Output 'personal_device_result_version=ok'
  Write-Output 'personal_device_coworkignore_boundary=ok'
  Write-Output 'personal_device_no_automatic_apply=ok'
  Write-Output 'personal_device_disconnect_recovery=ok'
  Write-Output 'personal_device_relay_idempotency=ok'
  Write-Output 'personal_device_metadata_outbox=ok'
  Write-Output 'personal_device_metadata_inbox=ok'
  Write-Output 'personal_device_metadata_cursor_persistence=ok'
} catch {
  foreach ($log in @('server.stdout.log','server.stderr.log','daemon.stdout.log','daemon.stderr.log','agent.stdout.log','agent.stderr.log','model.stdout.log','model.stderr.log','forward-1.stdout.log','forward-1.stderr.log','forward-2.stdout.log','forward-2.stderr.log')) {
    $path = Join-Path $testRoot $log
    if (Test-Path -LiteralPath $path) { Write-Output "### $log"; Get-Content -LiteralPath $path -Tail 120 }
  }
  throw
} finally {
  foreach ($process in @($forwardProcesses) + @($agentProcess,$daemonProcess,$serverProcess,$modelProcess)) {
    if ($process -and -not $process.HasExited) {
      Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
      $process.WaitForExit()
    }
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
