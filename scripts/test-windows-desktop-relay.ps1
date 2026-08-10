$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-windows-desktop-relay-e2e-$([guid]::NewGuid().ToString('N'))"
$serverProcess = $null
$executorSocket = $null
$clientSocket = $null
$reverseSocket = $null
$password = 'Desktop-E2E-Password-42!'
$deviceId = [guid]::NewGuid()
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 200
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token) {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{
    Method = $method
    Uri = "http://127.0.0.1:18082/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30)
  }
  try {
    return Invoke-RestMethod @parameters
  } catch {
    throw "$method /api/v1$path failed: $($_.Exception.Message)"
  }
}

function Connect-WebSocket([string]$uri, [string]$token = '') {
  $socket = [Net.WebSockets.ClientWebSocket]::new()
  if ($token) { $socket.Options.SetRequestHeader('Authorization', "Bearer $token") }
  $timeout = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(15))
  try {
    $null = $socket.ConnectAsync([Uri]::new($uri), $timeout.Token).GetAwaiter().GetResult()
    return $socket
  } catch {
    $socket.Dispose()
    throw
  } finally {
    $timeout.Dispose()
  }
}

function Receive-WebSocketMessage($socket, [int]$maxBytes = 65536) {
  $output = [IO.MemoryStream]::new()
  $buffer = [byte[]]::new(4096)
  $timeout = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(20))
  try {
    do {
      $result = $socket.ReceiveAsync([ArraySegment[byte]]::new($buffer), $timeout.Token).GetAwaiter().GetResult()
      if ($result.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Close) {
        throw 'WebSocket closed before the expected message arrived'
      }
      $output.Write($buffer, 0, $result.Count)
      if ($output.Length -gt $maxBytes) { throw "WebSocket message exceeded $maxBytes bytes" }
    } while (-not $result.EndOfMessage)
    return @{
      Type = $result.MessageType
      Bytes = $output.ToArray()
    }
  } finally {
    $timeout.Dispose()
    $output.Dispose()
  }
}

function Send-WebSocketBinary($socket, [byte[]]$bytes) {
  $null = $socket.SendAsync(
    [ArraySegment[byte]]::new($bytes),
    [Net.WebSockets.WebSocketMessageType]::Binary,
    $true,
    [Threading.CancellationToken]::None
  ).GetAwaiter().GetResult()
}

function Send-WebSocketText($socket, [string]$text) {
  $bytes = [Text.Encoding]::UTF8.GetBytes($text)
  $null = $socket.SendAsync(
    [ArraySegment[byte]]::new($bytes),
    [Net.WebSockets.WebSocketMessageType]::Text,
    $true,
    [Threading.CancellationToken]::None
  ).GetAwaiter().GetResult()
}

try {
  foreach ($name in @('bootstrap_token.txt', 'postgres_password.txt')) {
    if (-not (Test-Path -LiteralPath (Join-Path $secretRoot $name))) {
      throw "missing deployment secret $name; run deploy/init-secrets.ps1 first"
    }
  }

  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18082'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/cowork"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_SERVER_CAPABILITIES = 'model.external,files,shell,git,web.fetch,browser.headless,browser.visible,desktop.linux,office.ooxml,office.libreoffice'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18082/readyz' 30

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

  $team = Invoke-Json 'POST' '/teams' @{
    name = "Windows Relay E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
  } $accessToken
  $project = Invoke-Json 'POST' '/projects' @{
    name = "Windows Relay E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
    description = 'Disposable reverse desktop relay project'
    privacy = 'team_managed'
    team_id = $team.id
    preferred_executor_target = $null
    policy = @{ tool_policy = 'autonomous' }
  } $accessToken
  $thread = Invoke-Json 'POST' '/threads' @{
    project_id = $project.id
    title = 'Windows reverse desktop relay'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $accessToken
  $pool = Invoke-Json 'POST' '/executor-pools' @{
    name = "Windows Relay E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
    kind = 'managed_windows'
    team_id = $team.id
    policy = @{}
  } $accessToken
  Invoke-Json 'POST' "/executor-pools/$($pool.id)/projects" @{
    project_id = $project.id
  } $accessToken | Out-Null

  $executorId = [guid]::NewGuid()
  $executor = Invoke-Json 'POST' '/executors' @{
    schema_version = 1
    executor_id = $executorId.ToString()
    kind = 'managed_windows'
    pool_id = $pool.id
    owner_user_id = $null
    display_name = 'Windows Reverse Desktop E2E'
    protocol_version = 1
    capabilities = @(
      @{ schema_version = 1; name = 'desktop.windows'; version = 'e2e'; attributes = @{} },
      @{ schema_version = 1; name = 'office.microsoft'; version = 'e2e'; attributes = @{} }
    )
    labels = @{ os = 'windows'; isolation = 'disposable-e2e' }
    max_concurrent_runs = 1
  } $accessToken
  $credential = Invoke-Json 'POST' "/executors/$executorId/credentials" @{
    label = 'Disposable reverse desktop E2E credential'
    expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $accessToken

  $executorSocket = Connect-WebSocket `
    "ws://127.0.0.1:18082/api/v1/agent/executors/$executorId/connect" `
    $credential.token
  $helloMessage = Receive-WebSocketMessage $executorSocket
  $hello = [Text.Encoding]::UTF8.GetString($helloMessage.Bytes) | ConvertFrom-Json
  if ($hello.type -ne 'hello' -or $hello.executor_id -ne $executorId.ToString()) {
    throw 'executor WebSocket did not return the expected authenticated hello'
  }
  Start-Sleep -Milliseconds 250

  $run = Invoke-Json 'POST' '/runs' @{
    thread_id = $thread.id
    project_id = $project.id
    project_revision = $project.revision
    project_privacy = 'team_managed'
    task = $null
    executor_target = @{ kind = 'managed_windows_pool'; pool_id = $pool.id }
    required_capabilities = @('desktop.windows')
    input = @{ prompt = 'Keep the interactive desktop available for relay validation' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $accessToken

  $lease = $null
  $leaseDeadline = (Get-Date).AddSeconds(15)
  do {
    $executorMessage = Receive-WebSocketMessage $executorSocket
    $decoded = [Text.Encoding]::UTF8.GetString($executorMessage.Bytes) | ConvertFrom-Json
    if ($decoded.type -eq 'lease') { $lease = $decoded.lease }
  } while (-not $lease -and (Get-Date) -lt $leaseDeadline)
  if (-not $lease -or $lease.run.spec.id -ne $run.spec.id) {
    throw 'managed Windows executor did not receive the expected lease'
  }

  $session = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions" @{
    width = 1280
    height = 720
  } $accessToken
  if ($session.executor_id -ne $executorId.ToString() -or $session.stream_protocol -ne 'rfb.binary.v1') {
    throw 'managed Windows desktop session was not bound to its leased executor'
  }
  $grant = Invoke-Json 'POST' '/auth/reauthenticate' @{
    password = $password
    purpose = 'desktop_control'
  } $accessToken
  $ticket = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions/$($session.id)/tickets" @{
    control = $true
    reauthentication_token = $grant.token
  } $accessToken

  $ticketValue = [Uri]::EscapeDataString($ticket.token)
  $clientSocket = Connect-WebSocket `
    "ws://127.0.0.1:18082/api/v1/desktop-sessions/$($session.id)/stream?ticket=$ticketValue"
  $streamRequestMessage = Receive-WebSocketMessage $executorSocket
  $streamRequest = [Text.Encoding]::UTF8.GetString($streamRequestMessage.Bytes) | ConvertFrom-Json
  if (
    $streamRequest.type -ne 'desktop_stream_requested' -or
    $streamRequest.run_id -ne $run.spec.id -or
    $streamRequest.session_id -ne $session.id -or
    -not $streamRequest.control
  ) {
    throw 'executor did not receive the expected lease-bound desktop stream command'
  }

  $reverseSocket = Connect-WebSocket `
    "ws://127.0.0.1:18082/api/v1/agent/executors/$executorId/desktop-streams/$($streamRequest.stream_id)" `
    $credential.token
  $rfbVersion = [Text.Encoding]::ASCII.GetBytes("RFB 003.008`n")
  Send-WebSocketBinary $reverseSocket $rfbVersion
  $relayedVersion = Receive-WebSocketMessage $clientSocket
  if ([Text.Encoding]::ASCII.GetString($relayedVersion.Bytes) -ne "RFB 003.008`n") {
    throw 'RFB server handshake did not traverse the reverse executor relay'
  }

  $inputBytes = [byte[]](4, 1, 0, 0, 0, 0, 0, 65)
  Send-WebSocketBinary $clientSocket $inputBytes
  $relayedInput = Receive-WebSocketMessage $reverseSocket
  if ([BitConverter]::ToString($relayedInput.Bytes) -ne [BitConverter]::ToString($inputBytes)) {
    throw 'RFB control input did not traverse the reverse executor relay'
  }

  $null = $clientSocket.CloseOutputAsync(
    [Net.WebSockets.WebSocketCloseStatus]::NormalClosure,
    'relay e2e complete',
    [Threading.CancellationToken]::None
  ).GetAwaiter().GetResult()
  Start-Sleep -Seconds 1
  Invoke-Json 'DELETE' "/runs/$($run.spec.id)/desktop-sessions/$($session.id)" $null $accessToken | Out-Null

  $actions = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT action FROM audit_events WHERE target_id='$($session.id)' ORDER BY created_at;"
  $joinedActions = ($actions -join ',')
  foreach ($expected in @(
    'desktop_session.takeover_start',
    'desktop_session.input_summary',
    'desktop_session.takeover_end',
    'desktop_session.end'
  )) {
    if ($joinedActions -notmatch [regex]::Escape($expected)) {
      throw "missing managed Windows desktop audit action $expected"
    }
  }

  # A terminal executor result must close an otherwise active Windows desktop
  # without requiring a user to press the explicit End Desktop button.
  $automaticSession = Invoke-Json 'POST' "/runs/$($run.spec.id)/desktop-sessions" @{
    width = 1280
    height = 720
  } $accessToken
  Send-WebSocketText $executorSocket (@{
    type = 'complete'
    run_id = $run.spec.id
    request = @{
      lease_token = $lease.lease_token
      result = @{ relay_e2e = 'completed' }
    }
  } | ConvertTo-Json -Compress -Depth 20)
  $completionAckMessage = Receive-WebSocketMessage $executorSocket
  $completionAck = [Text.Encoding]::UTF8.GetString($completionAckMessage.Bytes) | ConvertFrom-Json
  if ($completionAck.type -ne 'ack' -or $completionAck.operation -ne 'complete') {
    throw 'executor completion was not acknowledged'
  }
  $automaticState = (docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT state FROM desktop_sessions WHERE id='$($automaticSession.id)';").Trim()
  if ($automaticState -ne 'ended') {
    throw "terminal run completion left its Windows desktop session in state '$automaticState'"
  }

  Write-Output "run_id=$($run.spec.id)"
  Write-Output "executor_id=$executorId"
  Write-Output "desktop_session_id=$($session.id)"
  Write-Output "reverse_stream_id=$($streamRequest.stream_id)"
  Write-Output "rfb_handshake=$([Text.Encoding]::ASCII.GetString($relayedVersion.Bytes).Trim())"
  Write-Output "relayed_input_bytes=$($relayedInput.Bytes.Length)"
  Write-Output "audit_actions=$joinedActions"
  Write-Output "automatic_desktop_state=$automaticState"
} catch {
  if (Test-Path -LiteralPath (Join-Path $testRoot 'server.stdout.log')) {
    Get-Content -LiteralPath (Join-Path $testRoot 'server.stdout.log')
  }
  if (Test-Path -LiteralPath (Join-Path $testRoot 'server.stderr.log')) {
    Get-Content -LiteralPath (Join-Path $testRoot 'server.stderr.log')
  }
  throw
} finally {
  foreach ($socket in @($clientSocket, $reverseSocket, $executorSocket)) {
    if ($null -ne $socket) { $socket.Dispose() }
  }
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if (
    $resolvedRoot.StartsWith($tempRoot) -and
    (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-windows-desktop-relay-e2e-')
  ) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
