$ErrorActionPreference = 'Stop'

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-server-desktop-e2e-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$runnerProcess = $null
$serverProcess = $null
$workerProcess = $null
$fakeModelProcess = $null
$password = 'Desktop-E2E-Password-42!'
$deviceId = [guid]::NewGuid()

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 250
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token) {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{
    Method = $method
    Uri = "http://127.0.0.1:18080/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30)
  }
  return Invoke-RestMethod @parameters
}

function Assert-HttpStatus([scriptblock]$operation, [int]$status, [string]$description) {
  try { & $operation | Out-Null; throw "$description unexpectedly succeeded" }
  catch {
    if ($_.Exception.Response.StatusCode.value__ -ne $status) { throw }
  }
}

try {
  foreach ($name in @(
    'bootstrap_token.txt', 'postgres_password.txt', 'minio_root_user.txt',
    'minio_root_password.txt', 'runner_signing_key.txt', 'storage_master_key.txt'
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $secretRoot $name))) {
      throw "missing deployment secret $name; run deploy/init-secrets.ps1 first"
    }
  }

  Push-Location $workspace
  try { cargo build -p cowork-runner -p cowork-server | Out-Host } finally { Pop-Location }

  $runnerKeyPath = (Resolve-Path (Join-Path $secretRoot 'runner_signing_key.txt')).Path
  $env:COWORK_RUNNER_SIGNING_KEY_FILE = $runnerKeyPath
  $env:COWORK_RUNNER_LISTEN_ADDR = '127.0.0.1:18090'
  $env:COWORK_RUNNER_CORE_IMAGE = 'open-cowork-sandbox-core:0.3.0'
  $env:COWORK_RUNNER_GUI_IMAGE = 'open-cowork-sandbox-gui:0.3.0'
  $env:COWORK_SANDBOX_EGRESS_NETWORK = 'open-cowork-sandbox-egress'
  $env:COWORK_SANDBOX_HTTP_PROXY = 'http://egress-proxy:3128'
  $runnerProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-runner.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'runner.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'runner.stderr.log')
  Wait-Http 'http://127.0.0.1:18090/healthz' 20

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18080'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/cowork"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_RUNNER_URL = 'http://127.0.0.1:18090'
  $env:COWORK_RUNNER_SIGNING_KEY_FILE = $runnerKeyPath
  $env:COWORK_SERVER_CAPABILITIES = 'model.external,files,shell,git,web.fetch,browser.headless,browser.visible,desktop.linux,office.ooxml,office.libreoffice'
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18080/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $credentials = @{
    email = 'desktop-e2e@opencowork.invalid'
    display_name = 'Desktop E2E'
    password = $password
    device_id = $deviceId.ToString()
  }
  try {
    $tokens = Invoke-Json 'POST' '/auth/bootstrap' $credentials $bootstrapToken
  } catch {
    $tokens = Invoke-Json 'POST' '/auth/login' @{
      email = $credentials.email
      password = $password
      device_id = $deviceId.ToString()
    } $null
  }
  $accessToken = $tokens.access_token

  $project = Invoke-Json 'POST' '/projects' @{
    name = "Desktop E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
    description = 'Disposable desktop integration project'
    privacy = 'private_local'
    team_id = $null
    preferred_executor_target = @{ kind = 'server_linux' }
    policy = @{ tool_policy = 'autonomous' }
  } $accessToken
  $thread = Invoke-Json 'POST' '/threads' @{
    project_id = $project.id
    title = 'Desktop integration'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $accessToken
  $run = Invoke-Json 'POST' '/runs' @{
    thread_id = $thread.id
    project_id = $project.id
    project_revision = $project.revision
    project_privacy = 'private_local'
    task = $null
    executor_target = @{ kind = 'server_linux' }
    required_capabilities = @('desktop.linux')
    input = @{ prompt = 'desktop integration test' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $accessToken

  $updated = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "UPDATE runs SET state='running', revision=revision+1, started_at=now(), updated_at=now() WHERE id='$($run.spec.id)' RETURNING id;"
  if (-not ($updated -match [regex]::Escape($run.spec.id))) { throw 'test run could not be activated' }

  $session = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions" @{
    width = 1024
    height = 768
  } $accessToken
  $listed = Invoke-Json 'GET' "/runs/$($run.spec.id)/desktop-sessions" $null $accessToken
  if ($listed.Count -ne 1 -or $listed[0].id -ne $session.id) { throw 'desktop session list mismatch' }

  $grant = Invoke-Json 'POST' '/auth/reauthenticate' @{
    password = $password
    purpose = 'desktop_control'
  } $accessToken
  $ticket = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions/$($session.id)/tickets" @{
    control = $true
    reauthentication_token = $grant.token
  } $accessToken

  $webSocket = [Net.WebSockets.ClientWebSocket]::new()
  try {
    $ticketValue = [Uri]::EscapeDataString($ticket.token)
    $uri = [Uri]::new("ws://127.0.0.1:18080/api/v1/desktop-sessions/$($session.id)/stream?ticket=$ticketValue")
    $null = $webSocket.ConnectAsync($uri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $buffer = [byte[]]::new(64)
    $received = $webSocket.ReceiveAsync([ArraySegment[byte]]::new($buffer), [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $handshake = [Text.Encoding]::ASCII.GetString($buffer, 0, $received.Count)
    if (-not $handshake.StartsWith('RFB 003.')) { throw "unexpected API RFB handshake: $handshake" }
    $inputBytes = [byte[]](1, 2, 3, 4, 5, 6)
    $null = $webSocket.SendAsync([ArraySegment[byte]]::new($inputBytes), [Net.WebSockets.WebSocketMessageType]::Binary, $true, [Threading.CancellationToken]::None).GetAwaiter().GetResult()

    # A second authorized viewer must remain able to observe the session while
    # the control socket is active. It is routed to the runner's dedicated
    # view-only RFB port and cannot inject desktop input.
    $viewTicket = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions/$($session.id)/tickets" @{
      control = $false
      reauthentication_token = $null
    } $accessToken
    $viewWebSocket = [Net.WebSockets.ClientWebSocket]::new()
    try {
      $viewTicketValue = [Uri]::EscapeDataString($viewTicket.token)
      $viewUri = [Uri]::new("ws://127.0.0.1:18080/api/v1/desktop-sessions/$($session.id)/stream?ticket=$viewTicketValue")
      $null = $viewWebSocket.ConnectAsync($viewUri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
      $viewBuffer = [byte[]]::new(64)
      $viewReceived = $viewWebSocket.ReceiveAsync([ArraySegment[byte]]::new($viewBuffer), [Threading.CancellationToken]::None).GetAwaiter().GetResult()
      $viewHandshake = [Text.Encoding]::ASCII.GetString($viewBuffer, 0, $viewReceived.Count)
      if (-not $viewHandshake.StartsWith('RFB 003.')) { throw "unexpected view-only RFB handshake during takeover: $viewHandshake" }
      $null = $viewWebSocket.CloseOutputAsync([Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'viewer complete', [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    } finally {
      $viewWebSocket.Dispose()
    }
    $null = $webSocket.CloseOutputAsync([Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'e2e complete', [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    Start-Sleep -Milliseconds 250
  } finally {
    $webSocket.Dispose()
  }
  Start-Sleep -Seconds 2

  Invoke-Json 'DELETE' "/runs/$($run.spec.id)/desktop-sessions/$($session.id)" $null $accessToken | Out-Null
  $actions = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT action FROM audit_events WHERE target_id='$($session.id)' ORDER BY created_at;"
  $joinedActions = ($actions -join ',')
  foreach ($expected in @('desktop_session.takeover_start', 'desktop_session.input_summary', 'desktop_session.takeover_end', 'desktop_session.end')) {
    if ($joinedActions -notmatch [regex]::Escape($expected)) { throw "missing audit action $expected" }
  }

  # Prove the outbound executor workspace channel without requiring Office on
  # the Linux CI host: a personal device uses the exact same lease-bound
  # snapshot and artifact endpoints as a managed Windows executor.
  $agentExecutorId = [guid]::NewGuid()
  $executor = Invoke-Json 'POST' '/executors' @{
    schema_version = 1
    executor_id = $agentExecutorId.ToString()
    kind = 'personal_device'
    pool_id = $null
    owner_user_id = $null
    display_name = 'Executor data plane E2E'
    protocol_version = 1
    capabilities = @(
      @{ schema_version = 1; name = 'files'; version = 'e2e'; attributes = @{} },
      @{ schema_version = 1; name = 'desktop.windows'; version = 'e2e'; attributes = @{} }
    )
    labels = @{ os = 'e2e' }
    personal_device_remote_control = 'off'
    max_concurrent_runs = 1
  } $accessToken
  if ($executor.registration.personal_device_remote_control -ne 'off') {
    throw 'personal executor did not preserve the explicit remote-control mode'
  }
  $executorCredential = Invoke-Json 'POST' "/executors/$agentExecutorId/credentials" @{
    label = 'Disposable E2E credential'
    expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $accessToken
  $snapshotBytes = [Text.Encoding]::UTF8.GetBytes('Windows executor snapshot input')
  $snapshotHasher = [Security.Cryptography.SHA256]::Create()
  try {
    $snapshotDigest = ([BitConverter]::ToString($snapshotHasher.ComputeHash($snapshotBytes))).Replace('-', '').ToLowerInvariant()
  } finally {
    $snapshotHasher.Dispose()
  }
  $snapshotUpload = Invoke-Json 'POST' '/snapshots' @{
    project_id = $project.id
    total_bytes = $snapshotBytes.Length
    files = @(@{
      path = 'documents/input.txt'
      size = $snapshotBytes.Length
      mode = 420
      modified_at = (Get-Date).ToUniversalTime().ToString('o')
      chunks = @(@{ digest = $snapshotDigest; plaintext_size = $snapshotBytes.Length })
    })
    expires_at = (Get-Date).ToUniversalTime().AddDays(1).ToString('o')
  } $accessToken
  $null = Invoke-WebRequest -UseBasicParsing -Method PUT `
    -Uri "http://127.0.0.1:18080/api/v1/snapshots/$($snapshotUpload.manifest_id)/chunks/$snapshotDigest" `
    -Headers @{ authorization = "Bearer $accessToken" } -ContentType 'application/octet-stream' -Body $snapshotBytes
  $snapshot = Invoke-Json 'POST' "/snapshots/$($snapshotUpload.manifest_id)/commit" @{} $accessToken
  $agentThread = Invoke-Json 'POST' '/threads' @{
    project_id = $project.id
    title = 'Executor data plane'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $accessToken
  $agentRun = Invoke-Json 'POST' '/runs' @{
    thread_id = $agentThread.id
    project_id = $project.id
    project_revision = $project.revision
    project_privacy = 'private_local'
    task = $null
    executor_target = @{ kind = 'personal_device'; device_id = $agentExecutorId.ToString() }
    required_capabilities = @('files')
    input = @{ prompt = 'executor data plane test' }
    model_profile_id = $null
    snapshot_id = $snapshot.id
    idempotency_key = [guid]::NewGuid().ToString()
  } $accessToken
  $agentHeaders = @{ authorization = "Bearer $($executorCredential.token)" }
  $lease = Invoke-RestMethod -Method POST `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/claim" -Headers $agentHeaders
  if ($lease.run.spec.id -ne $agentRun.spec.id) { throw 'executor did not claim its snapshot run' }

  Assert-HttpStatus {
    Invoke-Json 'POST' "/runs/$($agentRun.spec.id)/desktop-sessions" @{ width = 1024; height = 768 } $accessToken
  } 409 'personal desktop with remote control disabled'

  $executor = Invoke-Json 'POST' '/executors' @{
    schema_version = 2
    executor_id = $agentExecutorId.ToString()
    kind = 'personal_device'
    pool_id = $null
    owner_user_id = $null
    display_name = 'Executor data plane E2E'
    protocol_version = 2
    capabilities = @(
      @{ schema_version = 2; name = 'files'; version = 'e2e'; attributes = @{} },
      @{ schema_version = 2; name = 'desktop.windows'; version = 'e2e'; attributes = @{} }
    )
    labels = @{ os = 'e2e' }
    personal_device_remote_control = 'unattended'
    max_concurrent_runs = 1
  } $accessToken
  if ($executor.registration.personal_device_remote_control -ne 'unattended') {
    throw 'personal executor did not switch to unattended remote control'
  }
  $personalSession = Invoke-Json 'POST' "/runs/$($agentRun.spec.id)/desktop-sessions" @{
    width = 1024
    height = 768
  } $accessToken
  if ($personalSession.executor_id -ne $agentExecutorId.ToString()) {
    throw 'personal desktop session used the wrong executor'
  }
  Invoke-Json 'DELETE' "/runs/$($agentRun.spec.id)/desktop-sessions/$($personalSession.id)" $null $accessToken | Out-Null

  # An executor credential cannot relax the owner's server-side ceiling. This
  # N-1-shaped refresh omits the field; current agents may advertise a local
  # enforcement mode in labels, but neither form changes the server policy.
  $legacyRefresh = Invoke-RestMethod -Method POST `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/register" `
    -Headers $agentHeaders -ContentType 'application/json' -Body (@{
      schema_version = 1
      executor_id = $agentExecutorId.ToString()
      kind = 'personal_device'
      pool_id = $null
      owner_user_id = $null
      display_name = 'Executor data plane E2E legacy refresh'
      protocol_version = 1
      capabilities = @(
        @{ schema_version = 1; name = 'files'; version = 'e2e'; attributes = @{} },
        @{ schema_version = 1; name = 'desktop.windows'; version = 'e2e'; attributes = @{} }
      )
      labels = @{ os = 'e2e' }
      max_concurrent_runs = 1
    } | ConvertTo-Json -Compress -Depth 20)
  if ($legacyRefresh.registration.personal_device_remote_control -ne 'unattended') {
    throw 'N-1 executor refresh reset the personal remote-control mode'
  }
  $agentRefreshBody = @{
    schema_version = 2
    executor_id = $agentExecutorId.ToString()
    kind = 'personal_device'
    pool_id = $null
    owner_user_id = $null
    display_name = 'Executor data plane E2E agent refresh'
    protocol_version = 2
    capabilities = @(
      @{ schema_version = 2; name = 'files'; version = 'e2e'; attributes = @{} },
      @{ schema_version = 2; name = 'desktop.windows'; version = 'e2e'; attributes = @{} }
    )
    labels = @{ os = 'e2e'; local_remote_control_mode = 'off' }
    personal_device_remote_control = 'off'
    max_concurrent_runs = 1
  }
  $agentRefresh = Invoke-RestMethod -Method POST `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/register" `
    -Headers $agentHeaders -ContentType 'application/json' `
    -Body ($agentRefreshBody | ConvertTo-Json -Compress -Depth 20)
  if ($agentRefresh.registration.personal_device_remote_control -ne 'unattended' -or `
      $agentRefresh.registration.labels.local_remote_control_mode -ne 'off') {
    throw 'agent refresh either changed the server policy or lost the local enforcement status'
  }
  $leasedHeaders = @{
    authorization = "Bearer $($executorCredential.token)"
    'x-cowork-lease-token' = $lease.lease_token
  }
  $agentManifest = Invoke-RestMethod -Method GET `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/runs/$($agentRun.spec.id)/snapshot" `
    -Headers $leasedHeaders
  if ($agentManifest.id -ne $snapshot.id -or $agentManifest.files[0].path -ne 'documents/input.txt') {
    throw 'executor snapshot manifest mismatch'
  }
  $agentChunk = Invoke-WebRequest -UseBasicParsing -Method GET `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/runs/$($agentRun.spec.id)/snapshot/chunks/$snapshotDigest" `
    -Headers $leasedHeaders
  $agentChunkBytes = if ($agentChunk.Content -is [byte[]]) { $agentChunk.Content } else { [Text.Encoding]::UTF8.GetBytes([string]$agentChunk.Content) }
  if ([Text.Encoding]::UTF8.GetString($agentChunkBytes) -ne 'Windows executor snapshot input') {
    throw 'executor snapshot chunk plaintext mismatch'
  }
  $agentArtifactBytes = [Text.Encoding]::UTF8.GetBytes('Windows Office result')
  $agentArtifactPath = [Uri]::EscapeDataString('artifacts/windows/result.txt')
  $agentArtifactSource = [Uri]::EscapeDataString('MicrosoftOffice')
  $agentArtifact = Invoke-RestMethod -Method POST `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/runs/$($agentRun.spec.id)/artifacts?path=$agentArtifactPath&source=$agentArtifactSource" `
    -Headers $leasedHeaders -ContentType 'application/octet-stream' -Body $agentArtifactBytes
  if ($agentArtifact.storage -ne 'object_store_encrypted') { throw 'executor artifact was not encrypted' }
  $agentArtifactList = Invoke-Json 'GET' "/runs/$($agentRun.spec.id)/artifacts" $null $accessToken
  if ($agentArtifactList.Count -ne 1 -or $agentArtifactList[0].id -ne $agentArtifact.id) {
    throw 'executor artifact was not exposed through the normal run API'
  }
  $null = Invoke-RestMethod -Method POST `
    -Uri "http://127.0.0.1:18080/api/v1/agent/executors/$agentExecutorId/runs/$($agentRun.spec.id)/complete" `
    -Headers $agentHeaders -ContentType 'application/json' `
    -Body (@{ lease_token = $lease.lease_token; result = @{ artifact_id = $agentArtifact.id } } | ConvertTo-Json -Compress)

  Write-Output 'personal_desktop_remote_control_modes=ok'
  Write-Output 'personal_desktop_legacy_mode_preservation=ok'

  $team = Invoke-Json 'POST' '/teams' @{
    name = "Artifact E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
  } $accessToken
  $artifactProject = Invoke-Json 'POST' '/projects' @{
    name = "Artifact E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
    description = 'Disposable encrypted artifact integration project'
    privacy = 'team_managed'
    team_id = $team.id
    preferred_executor_target = @{ kind = 'server_linux' }
    policy = @{ tool_policy = 'autonomous' }
  } $accessToken
  $artifactThread = Invoke-Json 'POST' '/threads' @{
    project_id = $artifactProject.id
    title = 'Artifact integration'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $accessToken
  $artifactRun = Invoke-Json 'POST' '/runs' @{
    thread_id = $artifactThread.id
    project_id = $artifactProject.id
    project_revision = $artifactProject.revision
    project_privacy = 'team_managed'
    task = $null
    executor_target = @{ kind = 'server_linux' }
    required_capabilities = @('browser.headless')
    input = @{ prompt = 'capture a browser artifact' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $accessToken

  $fakeModelScript = Join-Path $workspace 'scripts/fake-openai-server.py'
  $fakeModelProcess = Start-Process -FilePath 'python' -ArgumentList ('"{0}"' -f $fakeModelScript) `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'model.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'model.stderr.log')
  Wait-Http 'http://127.0.0.1:18091/healthz' 10
  $env:COWORK_MODE = 'worker'
  $env:COWORK_MODEL_BASE_URL = 'http://127.0.0.1:18091/v1'
  $env:COWORK_MODEL_NAME = 'deterministic-e2e'
  $env:COWORK_WORKER_POLL_MS = '100'
  $workerProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'worker.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'worker.stderr.log')

  $deadline = (Get-Date).AddSeconds(120)
  do {
    Start-Sleep -Milliseconds 500
    $artifactRunState = Invoke-Json 'GET' "/runs/$($artifactRun.spec.id)" $null $accessToken
  } while ($artifactRunState.state -notin @('completed', 'failed', 'interrupted') -and (Get-Date) -lt $deadline)
  if ($artifactRunState.state -ne 'completed') {
    throw "artifact run ended in state $($artifactRunState.state): $($artifactRunState.error.message)"
  }
  $encryptedArtifacts = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT count(*) FROM run_artifacts WHERE run_id='$($artifactRun.spec.id)' AND key_scope_type='team' AND encrypted_data_key IS NOT NULL AND nonce IS NOT NULL AND wrap_nonce IS NOT NULL;"
  if ([int]$encryptedArtifacts -lt 2) { throw 'browser artifacts were not durably envelope-encrypted' }
  $artifactEvents = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT count(*) FROM run_events WHERE run_id='$($artifactRun.spec.id)' AND kind='artifact_created' AND payload->>'storage'='object_store_encrypted';"
  if ([int]$artifactEvents -lt 2) { throw 'durable artifact events were not emitted' }
  $artifacts = Invoke-Json 'GET' "/runs/$($artifactRun.spec.id)/artifacts" $null $accessToken
  $jsonArtifact = $artifacts | Where-Object { $_.media_type -eq 'application/json' } | Select-Object -First 1
  if (-not $jsonArtifact) { throw 'artifact list did not include the browser event log' }
  $download = Invoke-WebRequest -UseBasicParsing -Method GET `
    -Uri "http://127.0.0.1:18080/api/v1/runs/$($artifactRun.spec.id)/artifacts/$($jsonArtifact.id)" `
    -Headers @{ authorization = "Bearer $accessToken" }
  $downloadBytes = [Text.Encoding]::UTF8.GetBytes([string]$download.Content)
  $sha256 = [Security.Cryptography.SHA256]::Create()
  try {
    $downloadDigest = ([BitConverter]::ToString($sha256.ComputeHash($downloadBytes))).Replace('-', '').ToLowerInvariant()
  } finally {
    $sha256.Dispose()
  }
  if ($downloadDigest -ne $jsonArtifact.digest -or $downloadBytes.Length -ne $jsonArtifact.size_bytes) {
    throw 'downloaded artifact does not match its authenticated metadata'
  }

  Write-Output "run_id=$($run.spec.id)"
  Write-Output "desktop_session_id=$($session.id)"
  Write-Output "rfb_handshake=$($handshake.Trim())"
  Write-Output "concurrent_view_handshake=$($viewHandshake.Trim())"
  Write-Output "audit_actions=$joinedActions"
  Write-Output "executor_snapshot_id=$($snapshot.id)"
  Write-Output "executor_snapshot_bytes=$($agentChunkBytes.Length)"
  Write-Output "executor_artifact_id=$($agentArtifact.id)"
  Write-Output "artifact_run_id=$($artifactRun.spec.id)"
  Write-Output "encrypted_artifacts=$encryptedArtifacts"
  Write-Output "artifact_events=$artifactEvents"
  Write-Output "downloaded_artifact=$($jsonArtifact.name)"
  Write-Output "downloaded_artifact_bytes=$($downloadBytes.Length)"
} catch {
  if (Test-Path (Join-Path $testRoot 'runner.stderr.log')) { Get-Content (Join-Path $testRoot 'runner.stderr.log') }
  if (Test-Path (Join-Path $testRoot 'server.stderr.log')) { Get-Content (Join-Path $testRoot 'server.stderr.log') }
  if (Test-Path (Join-Path $testRoot 'worker.stderr.log')) { Get-Content (Join-Path $testRoot 'worker.stderr.log') }
  if (Test-Path (Join-Path $testRoot 'model.stderr.log')) { Get-Content (Join-Path $testRoot 'model.stderr.log') }
  throw
} finally {
  foreach ($process in @($workerProcess, $fakeModelProcess, $serverProcess, $runnerProcess)) {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force }
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-server-desktop-e2e-')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
