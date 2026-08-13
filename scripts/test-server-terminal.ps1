$ErrorActionPreference = 'Stop'

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-server-terminal-e2e-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$runnerProcess = $null
$serverProcess = $null
$projectId = $null
$runId = $null
$volumeName = $null
$password = 'Terminal-E2E-Password-42!'
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
    Uri = "http://127.0.0.1:18180/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30)
  }
  return Invoke-RestMethod @parameters
}

try {
  foreach ($name in @('bootstrap_token.txt', 'postgres_password.txt', 'runner_signing_key.txt')) {
    if (-not (Test-Path -LiteralPath (Join-Path $secretRoot $name))) {
      throw "missing deployment secret $name; run deploy/init-secrets.ps1 first"
    }
  }
  docker image inspect open-cowork-sandbox-core:0.3.0 | Out-Null

  Push-Location $workspace
  try { cargo build -p cowork-runner -p cowork-server | Out-Host } finally { Pop-Location }

  $runnerKeyPath = (Resolve-Path (Join-Path $secretRoot 'runner_signing_key.txt')).Path
  $env:COWORK_RUNNER_SIGNING_KEY_FILE = $runnerKeyPath
  $env:COWORK_RUNNER_LISTEN_ADDR = '127.0.0.1:18190'
  $env:COWORK_RUNNER_CORE_IMAGE = 'open-cowork-sandbox-core:0.3.0'
  $env:COWORK_RUNNER_GUI_IMAGE = 'open-cowork-sandbox-gui:0.3.0'
  $runnerProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-runner.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'runner.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'runner.stderr.log')
  Wait-Http 'http://127.0.0.1:18190/healthz' 20

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18180'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/cowork"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_RUNNER_URL = 'http://127.0.0.1:18190'
  $env:COWORK_RUNNER_SIGNING_KEY_FILE = $runnerKeyPath
  $env:COWORK_SERVER_CAPABILITIES = 'model.external,files,shell,git,web.fetch,browser.headless,browser.visible,desktop.linux,office.ooxml,office.libreoffice'
  Remove-Item Env:COWORK_S3_ENDPOINT -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_STORAGE_MASTER_KEY_FILE -ErrorAction SilentlyContinue
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18180/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $credentials = @{
    email = 'terminal-e2e@opencowork.invalid'
    display_name = 'Terminal E2E'
    password = $password
    device_id = $deviceId.ToString()
  }
  try {
    $tokens = Invoke-Json 'POST' '/auth/bootstrap' $credentials $bootstrapToken
  } catch {
    try {
      $tokens = Invoke-Json 'POST' '/auth/login' @{
        email = $credentials.email
        password = $password
        device_id = $deviceId.ToString()
      } $null
    } catch {
      # The shared development database may already have been bootstrapped by
      # the desktop E2E suite. Its deterministic disposable admin is safe to reuse.
      $tokens = Invoke-Json 'POST' '/auth/login' @{
        email = 'desktop-e2e@opencowork.invalid'
        password = 'Desktop-E2E-Password-42!'
        device_id = $deviceId.ToString()
      } $null
    }
  }
  $accessToken = $tokens.access_token

  $project = Invoke-Json 'POST' '/projects' @{
    name = "Terminal E2E $([guid]::NewGuid().ToString('N').Substring(0, 8))"
    description = 'Disposable terminal integration project'
    privacy = 'private_local'
    team_id = $null
    preferred_executor_target = @{ kind = 'server_linux' }
    policy = @{ tool_policy = 'autonomous' }
  } $accessToken
  $projectId = $project.id
  $thread = Invoke-Json 'POST' '/threads' @{
    project_id = $project.id
    title = 'Terminal integration'
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
    required_capabilities = @('shell')
    input = @{ prompt = 'terminal integration test' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = [guid]::NewGuid().ToString()
  } $accessToken
  $runId = $run.spec.id
  $updated = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "UPDATE runs SET state='running', revision=revision+1, started_at=now(), updated_at=now() WHERE id='$runId' RETURNING id;"
  if (-not ($updated -match [regex]::Escape($runId))) { throw 'test run could not be activated' }

  $volumeName = "cowork-run-$($runId.Replace('-', ''))"
  docker volume create $volumeName | Out-Null
  $ticket = Invoke-Json 'POST' "/runs/$runId/terminal-sessions" @{ columns = 100; rows = 30 } $accessToken
  if ($ticket.protocol -ne 'terminal.binary.v1') { throw 'unexpected terminal protocol' }

  $webSocket = [Net.WebSockets.ClientWebSocket]::new()
  try {
    $ticketValue = [Uri]::EscapeDataString($ticket.token)
    $uri = [Uri]::new("ws://127.0.0.1:18180/api/v1/terminal-sessions/$($ticket.session_id)/stream?ticket=$ticketValue")
    $null = $webSocket.ConnectAsync($uri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $command = [Text.Encoding]::UTF8.GetBytes("printf 'COWORK_TERMINAL_E2E_OK\n'; printf 'persisted' > terminal-result.txt`n")
    $null = $webSocket.SendAsync([ArraySegment[byte]]::new($command), [Net.WebSockets.WebSocketMessageType]::Binary, $true, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $output = [Text.StringBuilder]::new()
    $deadline = (Get-Date).AddSeconds(15)
    while ($output.ToString() -notmatch 'COWORK_TERMINAL_E2E_OK' -and (Get-Date) -lt $deadline) {
      $buffer = [byte[]]::new(16384)
      $timeout = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(2))
      try {
        $received = $webSocket.ReceiveAsync([ArraySegment[byte]]::new($buffer), $timeout.Token).GetAwaiter().GetResult()
        if ($received.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Close) { break }
        $null = $output.Append([Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count))
      } catch [OperationCanceledException] {
        # Continue until the overall deadline so slow CI hosts remain supported.
      } finally {
        $timeout.Dispose()
      }
    }
    if ($output.ToString() -notmatch 'COWORK_TERMINAL_E2E_OK') {
      throw "terminal output marker was not received: $output"
    }

    $ticketWasSingleUse = $false
    $secondSocket = [Net.WebSockets.ClientWebSocket]::new()
    try {
      try {
        $null = $secondSocket.ConnectAsync($uri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
      } catch {
        $ticketWasSingleUse = $true
      }
    } finally {
      $secondSocket.Dispose()
    }
    if (-not $ticketWasSingleUse) { throw 'terminal stream ticket was reusable' }
    $null = $webSocket.CloseOutputAsync([Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'e2e complete', [Threading.CancellationToken]::None).GetAwaiter().GetResult()
  } finally {
    $webSocket.Dispose()
  }
  Start-Sleep -Seconds 2

  $persisted = docker run --rm --volume "${volumeName}:/workspace:ro" open-cowork-sandbox-core:0.3.0 /bin/sh -lc 'cat /workspace/terminal-result.txt'
  if (($persisted -join '').Trim() -ne 'persisted') { throw 'terminal did not mutate its run workspace' }
  $sessionRow = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT state || ':' || input_bytes || ':' || output_bytes FROM terminal_sessions WHERE id='$($ticket.session_id)';"
  if ($sessionRow -notmatch '^ended:[1-9][0-9]*:[1-9][0-9]*$') { throw "unexpected terminal session accounting: $sessionRow" }
  $actions = docker exec open-cowork-postgres-1 psql -U cowork -d cowork -tAc `
    "SELECT action FROM audit_events WHERE target_id='$($ticket.session_id)' ORDER BY created_at;"
  $joinedActions = ($actions -join ',')
  foreach ($expected in @('terminal_session.create', 'terminal_session.connect', 'terminal_session.end')) {
    if ($joinedActions -notmatch [regex]::Escape($expected)) { throw "missing audit action $expected" }
  }

  Write-Output "run_id=$runId"
  Write-Output "terminal_session_id=$($ticket.session_id)"
  Write-Output 'terminal_output=ok'
  Write-Output 'single_use_ticket=ok'
  Write-Output "session_accounting=$sessionRow"
  Write-Output "audit_actions=$joinedActions"
} catch {
  if (Test-Path (Join-Path $testRoot 'runner.stdout.log')) { Get-Content (Join-Path $testRoot 'runner.stdout.log') }
  if (Test-Path (Join-Path $testRoot 'runner.stderr.log')) { Get-Content (Join-Path $testRoot 'runner.stderr.log') }
  if (Test-Path (Join-Path $testRoot 'server.stdout.log')) { Get-Content (Join-Path $testRoot 'server.stdout.log') }
  if (Test-Path (Join-Path $testRoot 'server.stderr.log')) { Get-Content (Join-Path $testRoot 'server.stderr.log') }
  throw
} finally {
  foreach ($process in @($serverProcess, $runnerProcess)) {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force }
  }
  if ($runId) {
    docker ps -aq --filter "label=dev.opencowork.run_id=$runId" | ForEach-Object { docker rm --force $_ | Out-Null }
  }
  if ($volumeName -and $volumeName -match '^cowork-run-[0-9a-f]{32}$') {
    docker volume rm --force $volumeName | Out-Null
  }
  if ($projectId) {
    docker exec open-cowork-postgres-1 psql -U cowork -d cowork -v ON_ERROR_STOP=1 -c "DELETE FROM runs WHERE project_id='$projectId'; DELETE FROM threads WHERE project_id='$projectId'; DELETE FROM projects WHERE id='$projectId';" | Out-Null
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-server-terminal-e2e-')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
