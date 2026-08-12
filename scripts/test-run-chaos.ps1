$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_chaos_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$apiProcess = $null
$workerProcess = $null
$reaperProcess = $null
$runnerProcess = $null
$apiPort = 18096
$runnerPort = 18097
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 150
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
  return $result
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{ Method = $method; Uri = "http://127.0.0.1:$apiPort/api/v1$path"; Headers = $headers }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = $body | ConvertTo-Json -Compress -Depth 30
  }
  return Invoke-RestMethod @parameters
}

function Assert-HttpStatus([int]$expected, [scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null }
  catch {
    $response = $_.Exception.PSObject.Properties['Response']
    if ($null -eq $response -or $response.Value.StatusCode.value__ -ne $expected) { throw }
    return
  }
  throw "$description unexpectedly succeeded"
}

function New-ThreadAndRun($project, [string]$token, $target, [string]$title, $runInput) {
  $thread = Invoke-Json POST '/threads' @{
    project_id = $project.id; title = $title
    forked_from_thread_id = $null; forked_from_message_id = $null
  } $token
  return Invoke-Json POST '/runs' @{
    thread_id = $thread.id; project_id = $project.id; project_revision = $project.revision
    project_privacy = 'team_managed'; task = $null; executor_target = $target
    required_capabilities = @(); input = $runInput; model_profile_id = $null; snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $token
}

function Wait-RunState([guid]$runId, [string]$token, [string]$expected, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 150
    $run = Invoke-Json GET "/runs/$runId" $null $token
  } while ($run.state -ne $expected -and (Get-Date) -lt $deadline)
  if ($run.state -ne $expected) { throw "run $runId remained $($run.state), expected $expected" }
  return $run
}

try {
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host
  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_LISTEN_ADDR = "127.0.0.1:$apiPort"
  $env:COWORK_SERVER_CAPABILITIES = 'shell,files'
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  $env:COWORK_LEASE_SECONDS = '2'
  $env:COWORK_WORKER_POLL_MS = '50'
  Remove-Item Env:COWORK_MODEL_BASE_URL,Env:COWORK_MODEL_API_KEY,Env:COWORK_RUNNER_URL,Env:COWORK_RUNNER_SIGNING_KEY,Env:COWORK_S3_ENDPOINT,Env:COWORK_PUBLIC_ORIGIN,Env:COWORK_WEBAUTHN_RP_ID,Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue

  cargo build -p cowork-server | Out-Host
  $env:COWORK_MODE = 'api'
  $apiProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'api.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'api.stderr.log')
  Wait-Http "http://127.0.0.1:$apiPort/readyz" 30 | Out-Null

  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'chaos-admin@opencowork.invalid'; display_name = 'Chaos Admin'
    password = 'Chaos-Test-Password-42!'; device_id = [guid]::NewGuid().ToString()
  } $env:COWORK_BOOTSTRAP_TOKEN
  $team = Invoke-Json POST '/teams' @{ name = 'Chaos acceptance team' } $admin.access_token
  $project = Invoke-Json POST '/projects' @{
    name = 'Chaos acceptance project'; description = ''; privacy = 'team_managed'; team_id = $team.id
    preferred_executor_target = @{ kind = 'server_linux'; pool_id = $null }
    policy = @{ tool_policy = 'autonomous' }
  } $admin.access_token

  $env:COWORK_CHAOS_RUNNER_PORT = $runnerPort.ToString()
  $env:COWORK_CHAOS_RUNNER_DELAY_MS = '60000'
  $runnerScript = Join-Path $workspace 'scripts/fake-chaos-runner.mjs'
  $runnerProcess = Start-Process node -ArgumentList ('"{0}"' -f $runnerScript) `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'runner.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'runner.stderr.log')
  Wait-Http "http://127.0.0.1:$runnerPort/healthz" 15 | Out-Null

  $unsafeRun = New-ThreadAndRun $project $admin.access_token @{ kind = 'server_linux'; pool_id = $null } 'Killed worker' @{
    sandbox = @{
      schema_version = 2; run_id = [guid]::Empty; image = 'core'
      argv = @('/bin/sh', '-lc', 'perform-unsafe-side-effect'); environment = @{}
      stdin_base64 = $null; network = 'none'
      limits = @{ memory_bytes = 268435456; cpu_nanos = 500000000; pids = 32; timeout_seconds = 120; tmpfs_bytes = 67108864; output_bytes = 1048576 }
    }
  }
  $env:COWORK_MODE = 'worker'
  $env:COWORK_WORKER_ID = [guid]::NewGuid().ToString()
  $env:COWORK_RUNNER_URL = "http://127.0.0.1:$runnerPort"
  $env:COWORK_RUNNER_SIGNING_KEY = 'chaos-runner-signing-key-000000000000'
  $workerProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'worker.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'worker.stderr.log')

  $dispatchDeadline = (Get-Date).AddSeconds(15)
  do {
    Start-Sleep -Milliseconds 100
    $runnerCount = (Invoke-RestMethod "http://127.0.0.1:$runnerPort/count").jobs
    $unsafeCurrent = Invoke-Json GET "/runs/$($unsafeRun.spec.id)" $null $admin.access_token
    $unsafeCheckpoints = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
      "SELECT count(*) FROM run_checkpoints WHERE run_id='$($unsafeRun.spec.id)' AND NOT safe_to_resume;"
  } while (($runnerCount -ne 1 -or [int](($unsafeCheckpoints -join '').Trim()) -ne 1) -and $unsafeCurrent.state -notin @('failed', 'interrupted', 'completed') -and (Get-Date) -lt $dispatchDeadline)
  if ($runnerCount -ne 1 -or [int](($unsafeCheckpoints -join '').Trim()) -ne 1) {
    throw "worker never reached the unsafe dispatch checkpoint; state=$($unsafeCurrent.state), error=$($unsafeCurrent.error | ConvertTo-Json -Compress -Depth 10)"
  }

  Stop-Process $workerProcess.Id -Force
  $workerProcess.WaitForExit()
  $workerProcess = $null
  Remove-Item Env:COWORK_RUNNER_URL,Env:COWORK_RUNNER_SIGNING_KEY -ErrorAction SilentlyContinue
  $env:COWORK_WORKER_ID = [guid]::NewGuid().ToString()
  $reaperProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'reaper.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'reaper.stderr.log')

  $interrupted = Wait-RunState $unsafeRun.spec.id $admin.access_token 'interrupted' 15
  if ($interrupted.error.code -ne 'unsafe_tool_interrupted' -or $interrupted.error.details.safe_to_resume -ne $false -or $interrupted.error.details.manual_review_required -ne $true) {
    throw 'unsafe worker loss did not preserve manual-review semantics'
  }
  Start-Sleep -Seconds 1
  $runnerCount = (Invoke-RestMethod "http://127.0.0.1:$runnerPort/count").jobs
  $toolStarts = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT count(*) FROM run_events WHERE run_id='$($unsafeRun.spec.id)' AND kind='tool_started';"
  if ($runnerCount -ne 1 -or [int](($toolStarts -join '').Trim()) -ne 1) { throw 'unsafe sandbox action was dispatched more than once' }

  $executorId = [guid]::NewGuid()
  Invoke-Json POST '/executors' @{
    schema_version = 2; executor_id = $executorId; kind = 'personal_device'; pool_id = $null
    owner_user_id = $null; display_name = 'Chaos reconnect device'; protocol_version = 2
    capabilities = @(@{ schema_version = 2; name = 'files'; version = 'chaos'; attributes = @{} })
    labels = @{ os = 'chaos' }; max_concurrent_runs = 1
  } $admin.access_token | Out-Null
  $credential = Invoke-Json POST "/executors/$executorId/credentials" @{
    label = 'Chaos disposable credential'; expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $admin.access_token
  $executorToken = $credential.token
  Invoke-Json POST "/agent/executors/$executorId/heartbeat" @{
    protocol_version = 2; active_run_ids = @(); health = @{ status = 'ready' }
  } $executorToken | Out-Null

  $unsafeReconnectRun = New-ThreadAndRun $project $admin.access_token @{ kind = 'personal_device'; device_id = $executorId } 'Unsafe executor reconnect' @{ prompt = 'do not repeat' }
  $unsafeReconnectLease = Invoke-Json POST "/agent/executors/$executorId/claim" $null $executorToken
  if ($unsafeReconnectLease.run.spec.id -ne $unsafeReconnectRun.spec.id) { throw 'executor did not claim the unsafe reconnect run' }
  Invoke-Json POST "/agent/executors/$executorId/runs/$($unsafeReconnectRun.spec.id)/checkpoints" @{
    lease_token = $unsafeReconnectLease.lease_token; safe_to_resume = $false; executor_state = @{ phase = 'unsafe_action_dispatched' }
  } $executorToken | Out-Null
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d $databaseName -c `
    "UPDATE runs SET lease_expires_at=now()+interval '30 seconds' WHERE id='$($unsafeReconnectRun.spec.id)';" | Out-Null
  $executorSocket = [Net.WebSockets.ClientWebSocket]::new()
  try {
    $executorSocket.Options.SetRequestHeader('Authorization', "Bearer $executorToken")
    $executorSocket.ConnectAsync(
      [Uri]"ws://127.0.0.1:$apiPort/api/v1/agent/executors/$executorId/connect",
      [Threading.CancellationToken]::None
    ).GetAwaiter().GetResult()
    $unsafeReconnectInterrupted = Wait-RunState $unsafeReconnectRun.spec.id $admin.access_token 'interrupted' 10
  } finally {
    $executorSocket.Dispose()
  }
  if ($unsafeReconnectInterrupted.error.code -ne 'unsafe_tool_interrupted' `
      -or $unsafeReconnectInterrupted.error.details.safe_to_resume -ne $false `
      -or $unsafeReconnectInterrupted.error.details.manual_review_required -ne $true `
      -or $unsafeReconnectInterrupted.error.details.detected_during_reconnect -ne $true) {
    throw 'unsafe executor reconnect did not interrupt the run for manual review'
  }
  $activeRuns = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT active_runs FROM executors WHERE id='$executorId';"
  if ([int](($activeRuns -join '').Trim()) -ne 0) { throw 'unsafe reconnect did not release executor capacity' }

  $safeRun = New-ThreadAndRun $project $admin.access_token @{ kind = 'personal_device'; device_id = $executorId } 'Executor disconnect' @{ prompt = 'safe disconnect' }
  $safeLease = Invoke-Json POST "/agent/executors/$executorId/claim" $null $executorToken
  if ($safeLease.run.spec.id -ne $safeRun.spec.id) { throw 'executor did not claim the safe disconnect run' }
  Invoke-Json POST "/agent/executors/$executorId/runs/$($safeRun.spec.id)/checkpoints" @{
    lease_token = $safeLease.lease_token; safe_to_resume = $true; executor_state = @{ phase = 'idle' }
  } $executorToken | Out-Null
  $safeInterrupted = Wait-RunState $safeRun.spec.id $admin.access_token 'interrupted' 15
  if ($safeInterrupted.error.code -ne 'executor_lease_expired' -or $safeInterrupted.error.details.safe_to_resume -ne $true -or $safeInterrupted.error.details.manual_review_required -ne $false) {
    throw 'safe executor disconnect was classified incorrectly'
  }
  Assert-HttpStatus 409 {
    Invoke-Json POST "/agent/executors/$executorId/runs/$($safeRun.spec.id)/complete" @{
      lease_token = $safeLease.lease_token; result = @{ late = $true }
    } $executorToken
  } 'late completion with an expired lease'
  Assert-HttpStatus 409 {
    Invoke-Json POST "/agent/executors/$executorId/runs/$($safeRun.spec.id)/events" @{
      lease_token = $safeLease.lease_token; kind = 'warning'; payload = @{ late = $true }
    } $executorToken
  } 'late event with an expired lease'
  $unexpectedLease = Invoke-Json POST "/agent/executors/$executorId/claim" $null $executorToken
  if ($unexpectedLease -and $unexpectedLease.PSObject.Properties['run']) { throw 'interrupted run was reclaimed automatically' }

  Invoke-Json POST "/agent/executors/$executorId/heartbeat" @{
    protocol_version = 2; active_run_ids = @(); health = @{ status = 'reconnected' }
  } $executorToken | Out-Null
  $reconnectRun = New-ThreadAndRun $project $admin.access_token @{ kind = 'personal_device'; device_id = $executorId } 'Executor reconnect' @{ prompt = 'reconnect' }
  $reconnectLease = Invoke-Json POST "/agent/executors/$executorId/claim" $null $executorToken
  if ($reconnectLease.run.spec.id -ne $reconnectRun.spec.id) { throw 'reconnected executor was not reusable' }
  $completed = Invoke-Json POST "/agent/executors/$executorId/runs/$($reconnectRun.spec.id)/complete" @{
    lease_token = $reconnectLease.lease_token; result = @{ reconnected = $true }
  } $executorToken
  if ($completed.state -ne 'completed') { throw 'reconnected executor could not complete a new run' }
  $activeRuns = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT active_runs FROM executors WHERE id='$executorId';"
  if ([int](($activeRuns -join '').Trim()) -ne 0) { throw 'executor capacity was not released after reconnect completion' }

  $previousMaxSequence = [int](((docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT max(sequence) FROM run_events WHERE run_id='$($reconnectRun.spec.id)';") -join '').Trim())
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d $databaseName -c `
    "UPDATE runs SET finished_at=now()-interval '91 days', updated_at=now()-interval '91 days' WHERE id='$($reconnectRun.spec.id)'; UPDATE run_events SET created_at=now()-interval '91 days' WHERE run_id='$($reconnectRun.spec.id)';" | Out-Null
  $retentionDeadline = (Get-Date).AddSeconds(10)
  do {
    Start-Sleep -Milliseconds 150
    $retainedEvents = [int](((docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
      "SELECT count(*) FROM run_events WHERE run_id='$($reconnectRun.spec.id)';") -join '').Trim())
  } while ($retainedEvents -ne 0 -and (Get-Date) -lt $retentionDeadline)
  if ($retainedEvents -ne 0) { throw 'terminal run events were not removed after the 90-day retention window' }

  $retentionLease = [guid]::NewGuid()
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d $databaseName -c `
    "UPDATE runs SET state='running', assigned_executor_id='$executorId', lease_owner='$executorId', lease_token='$retentionLease', lease_expires_at=now()+interval '1 hour', finished_at=NULL, updated_at=now() WHERE id='$($reconnectRun.spec.id)';" | Out-Null
  $postRetentionEvent = Invoke-Json POST "/agent/executors/$executorId/runs/$($reconnectRun.spec.id)/events" @{
    lease_token = $retentionLease; kind = 'warning'; payload = @{ retention_cursor = 'verified' }
  } $executorToken
  if ($postRetentionEvent.sequence -le $previousMaxSequence) {
    throw 'event sequence was reused after retention removed prior events'
  }

  Write-Output 'killed_worker_lease_expiry=ok'
  Write-Output 'unsafe_dispatch_checkpoint=ok'
  Write-Output 'unsafe_action_not_repeated=ok'
  Write-Output 'unsafe_executor_reconnect_not_repeated=ok'
  Write-Output 'safe_executor_disconnect_classification=ok'
  Write-Output 'expired_lease_rejects_late_writes=ok'
  Write-Output 'executor_reconnect_and_capacity_cleanup=ok'
  Write-Output 'run_event_90_day_retention=ok'
  Write-Output 'post_retention_sequence_monotonic=ok'
} catch {
  foreach ($log in @('api.stdout.log', 'api.stderr.log', 'worker.stdout.log', 'worker.stderr.log', 'reaper.stdout.log', 'reaper.stderr.log', 'runner.stdout.log', 'runner.stderr.log')) {
    $path = Join-Path $testRoot $log
    if (Test-Path -LiteralPath $path) { Get-Content -LiteralPath $path }
  }
  throw
} finally {
  foreach ($process in @($workerProcess, $reaperProcess, $runnerProcess, $apiProcess)) {
    if ($process -and -not $process.HasExited) {
      Stop-Process $process.Id -Force -ErrorAction SilentlyContinue
      $process.WaitForExit()
    }
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
