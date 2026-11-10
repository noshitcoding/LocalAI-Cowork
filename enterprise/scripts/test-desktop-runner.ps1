$ErrorActionPreference = 'Stop'

$signingKey = 'desktop-runner-e2e-signing-key-000000000000'
$runId = [guid]::NewGuid()
$sessionId = [guid]::NewGuid()
$compactRun = $runId.ToString('N')
$volume = "cowork-run-$compactRun"
$container = "cowork-desktop-$($sessionId.ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-desktop-e2e-$($sessionId.ToString('N'))"
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$stdout = Join-Path $testRoot 'runner.stdout.log'
$stderr = Join-Path $testRoot 'runner.stderr.log'
$runnerProcess = $null

function New-SignedHeaders([string]$method, [string]$path, [byte[]]$body) {
  $timestamp = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds().ToString()
  $nonce = [guid]::NewGuid().ToString()
  $prefix = [Text.Encoding]::UTF8.GetBytes("$timestamp`n$nonce`n$method`n$path`n")
  $payload = [byte[]]::new($prefix.Length + $body.Length)
  [Array]::Copy($prefix, 0, $payload, 0, $prefix.Length)
  [Array]::Copy($body, 0, $payload, $prefix.Length, $body.Length)
  $hmac = [Security.Cryptography.HMACSHA256]::new([Text.Encoding]::UTF8.GetBytes($signingKey))
  try {
    $signature = ([BitConverter]::ToString($hmac.ComputeHash($payload))).Replace('-', '').ToLowerInvariant()
  } finally {
    $hmac.Dispose()
  }
  return @{
    'x-cowork-timestamp' = $timestamp
    'x-cowork-nonce' = $nonce
    'x-cowork-signature' = $signature
  }
}

function Invoke-SignedJson([string]$method, [string]$path, $value) {
  $json = $value | ConvertTo-Json -Compress -Depth 30
  $body = [Text.Encoding]::UTF8.GetBytes($json)
  $headers = New-SignedHeaders $method $path $body
  return Invoke-RestMethod -Method $method -Uri "http://127.0.0.1:18090$path" `
    -Headers $headers -ContentType 'application/json' -Body $body
}

function Start-TestRunner([string]$binary, [string]$stdout, [string]$stderr) {
  $process = Start-Process -FilePath $binary -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  $deadline = (Get-Date).AddSeconds(20)
  do {
    Start-Sleep -Milliseconds 250
    try { $health = Invoke-RestMethod 'http://127.0.0.1:18090/healthz' } catch { $health = $null }
  } while (-not $health -and (Get-Date) -lt $deadline)
  if (-not $health) {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    throw 'runner did not become healthy'
  }
  return $process
}

function Receive-RfbHandshake([guid]$sessionId) {
  $empty = [byte[]]::new(0)
  $path = "/v1/desktop-sessions/$sessionId/stream?control=false"
  $headers = New-SignedHeaders 'GET' $path $empty
  $webSocket = [Net.WebSockets.ClientWebSocket]::new()
  foreach ($entry in $headers.GetEnumerator()) {
    $webSocket.Options.SetRequestHeader($entry.Key, $entry.Value)
  }
  try {
    $uri = [Uri]::new("ws://127.0.0.1:18090$path")
    $null = $webSocket.ConnectAsync($uri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $buffer = [byte[]]::new(64)
    $segment = [ArraySegment[byte]]::new($buffer)
    $received = $webSocket.ReceiveAsync($segment, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    return [Text.Encoding]::ASCII.GetString($buffer, 0, $received.Count)
  } finally {
    $webSocket.Abort()
    $webSocket.Dispose()
  }
}

try {
  $env:COWORK_RUNNER_SIGNING_KEY = $signingKey
  $env:COWORK_RUNNER_LISTEN_ADDR = '127.0.0.1:18090'
  $env:COWORK_RUNNER_CORE_IMAGE = 'open-cowork-sandbox-core:0.3.0'
  $env:COWORK_RUNNER_GUI_IMAGE = 'open-cowork-sandbox-gui:0.3.0'
  $env:COWORK_SANDBOX_EGRESS_NETWORK = 'open-cowork-sandbox-egress'
  $env:COWORK_SANDBOX_HTTP_PROXY = 'http://egress-proxy:3128'
  $binary = (Resolve-Path 'target/debug/cowork-runner.exe').Path
  $runnerProcess = Start-TestRunner $binary $stdout $stderr

  $limits = @{
    memory_bytes = 2147483648
    cpu_nanos = 1000000000
    pids = 512
    timeout_seconds = 120
    tmpfs_bytes = 268435456
    output_bytes = 4194304
  }
  $session = Invoke-SignedJson 'POST' '/v1/desktop-sessions' @{
    schema_version = 1
    session_id = $sessionId.ToString()
    run_id = $runId.ToString()
    dimensions = @{ width = 1024; height = 768; scale_factor = 1.0 }
    network = 'filtered_egress'
    limits = $limits
  }
  if ($session.container_name -ne $container) { throw 'runner returned the wrong desktop container' }

  $handshake = Receive-RfbHandshake $sessionId
  if (-not $handshake.StartsWith('RFB 003.')) { throw "unexpected RFB handshake: $handshake" }

  $job = Invoke-SignedJson 'POST' '/v1/jobs' @{
    schema_version = 1
    run_id = $runId.ToString()
    image = 'gui'
    argv = @('/bin/bash', '-lc', 'printf desktop-exec > shared.txt')
    environment = @{}
    stdin_base64 = $null
    network = 'none'
    limits = $limits
  }
  if ($job.exit_code -ne 0) { throw "desktop job failed: $($job.stderr)" }
  if ($job.container_name -ne $container) { throw 'GUI job did not execute in the active desktop' }
  $content = docker run --rm --volume "${volume}:/workspace:ro" open-cowork-sandbox-core:0.3.0 /bin/bash -lc 'cat shared.txt'
  if ($content -ne 'desktop-exec') { throw 'desktop workspace result does not match' }

  $browserInput = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(
    '{"action":"navigate","url":"http://example.com","visible":true,"timeout_ms":30000}'
  ))
  $browserJob = Invoke-SignedJson 'POST' '/v1/jobs' @{
    schema_version = 1
    run_id = $runId.ToString()
    image = 'gui'
    argv = @('node', '/opt/cowork/browser-tool.mjs')
    environment = @{}
    stdin_base64 = $browserInput
    network = 'filtered_egress'
    limits = $limits
  }
  if ($browserJob.exit_code -ne 0) { throw "visible browser job failed: $($browserJob.stderr)" }
  $browserResult = $browserJob.stdout | ConvertFrom-Json
  if ($browserResult.title -ne 'Example Domain') { throw 'visible browser returned the wrong page' }
  $artifactPath = $browserResult.artifacts | Where-Object { $_.EndsWith('-events.json') } | Select-Object -First 1
  if (-not $artifactPath) { throw 'visible browser did not report its event artifact' }
  $artifactRoute = "/v1/runs/$runId/file?path=$([Uri]::EscapeDataString($artifactPath))"
  $artifactHeaders = New-SignedHeaders 'GET' $artifactRoute ([byte[]]::new(0))
  $artifactResponse = Invoke-WebRequest -UseBasicParsing -Method GET `
    -Uri "http://127.0.0.1:18090$artifactRoute" -Headers $artifactHeaders
  if ($artifactResponse.StatusCode -ne 200 -or $artifactResponse.Content.Length -lt 2) {
    throw 'runner did not export the browser artifact'
  }

  $failingBrowserInput = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(
    '{"action":"click","selector":"#cowork-element-that-does-not-exist","visible":true,"timeout_ms":500}'
  ))
  $failingBrowserJob = Invoke-SignedJson 'POST' '/v1/jobs' @{
    schema_version = 1
    run_id = $runId.ToString()
    image = 'gui'
    argv = @('node', '/opt/cowork/browser-tool.mjs')
    environment = @{}
    stdin_base64 = $failingBrowserInput
    network = 'filtered_egress'
    limits = $limits
  }
  if ($failingBrowserJob.exit_code -eq 0) { throw 'browser failure scenario unexpectedly succeeded' }
  $failingBrowserResult = $failingBrowserJob.stdout | ConvertFrom-Json
  if ($failingBrowserResult.ok -ne $false -or -not $failingBrowserResult.error.message) {
    throw 'failed browser action did not return structured diagnostics'
  }
  $failureLogPath = $failingBrowserResult.artifacts | Where-Object { $_.EndsWith('-events.json') } | Select-Object -First 1
  $failureTracePath = $failingBrowserResult.artifacts | Where-Object { $_.EndsWith('-trace.zip') } | Select-Object -First 1
  if (-not $failureLogPath -or -not $failureTracePath) { throw 'failed browser action did not preserve trace and event artifacts' }
  $failureLogRoute = "/v1/runs/$runId/file?path=$([Uri]::EscapeDataString($failureLogPath))"
  $failureLogHeaders = New-SignedHeaders 'GET' $failureLogRoute ([byte[]]::new(0))
  $failureLogResponse = Invoke-WebRequest -UseBasicParsing -Method GET `
    -Uri "http://127.0.0.1:18090$failureLogRoute" -Headers $failureLogHeaders
  $failureLogText = if ($failureLogResponse.Content -is [byte[]]) {
    [Text.Encoding]::UTF8.GetString($failureLogResponse.Content)
  } else {
    [string]$failureLogResponse.Content
  }
  if ($failureLogText -notmatch 'toolerror') { throw 'failed browser event log is missing the tool error' }
  $chromiumProcesses = docker exec $container /bin/bash -lc "pgrep -af 'chrome.*remote-debugging-port=9222' | wc -l"
  if ([int]$chromiumProcesses -lt 1) { throw 'visible Chromium did not remain alive in the desktop session' }

  Stop-Process -Id $runnerProcess.Id -Force
  $runnerProcess.WaitForExit()
  $runnerProcess = Start-TestRunner $binary $stdout $stderr
  $recoveredHandshake = Receive-RfbHandshake $sessionId
  if (-not $recoveredHandshake.StartsWith('RFB 003.')) { throw "recovered RFB handshake failed: $recoveredHandshake" }
  $recoveredJob = Invoke-SignedJson 'POST' '/v1/jobs' @{
    schema_version = 1
    run_id = $runId.ToString()
    image = 'gui'
    argv = @('/bin/bash', '-lc', 'test "$(cat shared.txt)" = desktop-exec')
    environment = @{}
    stdin_base64 = $null
    network = 'none'
    limits = $limits
  }
  if ($recoveredJob.exit_code -ne 0 -or $recoveredJob.container_name -ne $container) {
    throw 'runner did not recover the desktop session and workspace'
  }

  $empty = [byte[]]::new(0)
  $deletePath = "/v1/desktop-sessions/$sessionId"
  $headers = New-SignedHeaders 'DELETE' $deletePath $empty
  Invoke-WebRequest -UseBasicParsing -Method DELETE `
    -Uri "http://127.0.0.1:18090$deletePath" -Headers $headers | Out-Null
  Write-Output "desktop_container=$($session.container_name)"
  Write-Output "rfb_handshake=$($handshake.Trim())"
  Write-Output "job_container=$($job.container_name)"
  Write-Output "workspace_content=$content"
  Write-Output "visible_browser_title=$($browserResult.title)"
  Write-Output "visible_chromium_processes=$chromiumProcesses"
  Write-Output "artifact_path=$artifactPath"
  Write-Output "artifact_bytes=$($artifactResponse.Content.Length)"
  Write-Output "failure_trace_path=$failureTracePath"
  Write-Output "failure_log_path=$failureLogPath"
  Write-Output "recovered_rfb_handshake=$($recoveredHandshake.Trim())"
  Write-Output "recovered_job_container=$($recoveredJob.container_name)"
} catch {
  if (Test-Path $stderr) { Get-Content $stderr }
  throw
} finally {
  if ($runnerProcess -and -not $runnerProcess.HasExited) {
    Stop-Process -Id $runnerProcess.Id -Force
  }
  $existingContainer = docker ps -a --filter "name=^/${container}$" --format '{{.Names}}'
  if ($existingContainer -eq $container) {
    docker rm --force $container | Out-Null
  }
  $existingVolume = docker volume ls --filter "name=^${volume}$" --format '{{.Name}}'
  if ($existingVolume -eq $volume) {
    docker volume rm $volume | Out-Null
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-desktop-e2e-')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
