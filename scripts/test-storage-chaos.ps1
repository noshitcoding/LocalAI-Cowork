$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_storage_chaos_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$apiBase = 'http://127.0.0.1:18101/api/v1'
$serverProcess = $null
$workerProcess = $null
$forwarderProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 150
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Wait-Tcp([int]$port, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    $client = [Net.Sockets.TcpClient]::new()
    try {
      $task = $client.ConnectAsync('127.0.0.1', $port)
      if ($task.Wait(200) -and $client.Connected) { return }
    } catch { } finally { $client.Dispose() }
    Start-Sleep -Milliseconds 100
  } while ((Get-Date) -lt $deadline)
  throw "TCP port $port did not become ready"
}

function Start-S3Forwarder {
  $env:COWORK_FORWARD_LISTEN_HOST = '127.0.0.1'
  $env:COWORK_FORWARD_LISTEN_PORT = '19101'
  $env:COWORK_FORWARD_TARGET_HOST = '127.0.0.1'
  $env:COWORK_FORWARD_TARGET_PORT = '19000'
  $process = Start-Process -FilePath node -ArgumentList '.\scripts\tcp-forwarder.mjs' `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'forwarder.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'forwarder.stderr.log')
  Wait-Tcp 19101 10
  return $process
}

function Stop-Child($process) {
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    $process.WaitForExit()
  }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{ Method = $method; Uri = "$apiBase$path"; Headers = $headers }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = $body | ConvertTo-Json -Compress -Depth 20
  }
  return Invoke-RestMethod @parameters
}

function Invoke-Chunk([string]$manifestId, [string]$digest, [byte[]]$bytes, [string]$token) {
  return Invoke-RestMethod -Method PUT -Uri "$apiBase/snapshots/$manifestId/chunks/$digest" `
    -Headers @{ authorization = "Bearer $token" } -ContentType 'application/octet-stream' -Body $bytes
}

function Assert-Status([int]$expected, [scriptblock]$operation, [string]$description) {
  try {
    & $operation | Out-Null
    throw "$description unexpectedly succeeded"
  } catch {
    if (-not $_.Exception.Response -or $_.Exception.Response.StatusCode.value__ -ne $expected) { throw }
  }
}

function Digest-Hex([byte[]]$bytes) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try { return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant() }
  finally { $sha.Dispose() }
}

function Begin-Snapshot([string]$path, [string]$digest, [int]$size, [string]$projectId, [string]$token) {
  return Invoke-Json POST '/snapshots' @{
    project_id = $projectId
    total_bytes = $size
    files = @(@{
      path = $path
      size = $size
      mode = 420
      modified_at = (Get-Date).ToUniversalTime().ToString('o')
      chunks = @(@{ digest = $digest; plaintext_size = $size })
    })
    expires_at = (Get-Date).ToUniversalTime().AddDays(1).ToString('o')
  } $token
}

function Query-Scalar([string]$sql) {
  return ((docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc $sql) -join '').Trim()
}

function Start-Worker([string]$logName) {
  $env:COWORK_MODE = 'worker'
  $env:COWORK_WORKER_POLL_MS = '50'
  return Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot "$logName.stdout.log") `
    -RedirectStandardError (Join-Path $testRoot "$logName.stderr.log")
}

try {
  foreach ($name in @(
    'bootstrap_token.txt', 'postgres_password.txt', 'minio_root_user.txt',
    'minio_root_password.txt', 'storage_master_key.txt'
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $secretRoot $name))) {
      throw "missing deployment secret $name"
    }
  }
  foreach ($container in @('open-cowork-postgres-1', 'open-cowork-minio-1')) {
    if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch $container)) {
      throw "$container is not running"
    }
  }

  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $env:DATABASE_URL = "postgres://cowork:$([IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim())@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19101'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_user.txt')).Trim()
  $env:COWORK_S3_SECRET_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_password.txt')).Trim()
  $env:COWORK_STORAGE_MASTER_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'storage_master_key.txt')).Trim()
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  Remove-Item Env:COWORK_RUNNER_URL, Env:COWORK_RUNNER_SIGNING_KEY, Env:COWORK_PUBLIC_ORIGIN, `
    Env:COWORK_WEBAUTHN_RP_ID, Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue

  $forwarderProcess = Start-S3Forwarder
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18101'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18101/readyz' 30

  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'storage-chaos@opencowork.invalid'
    display_name = 'Storage Chaos'
    password = 'Storage-Chaos-Password-42!'
    device_id = [guid]::NewGuid().ToString()
  } $env:COWORK_BOOTSTRAP_TOKEN
  $project = Invoke-Json POST '/projects' @{
    name = 'Storage chaos project'
    description = ''
    privacy = 'private_local'
    team_id = $null
    preferred_executor_target = $null
    policy = @{}
  } $admin.access_token

  $chunk = [byte[]]::new(1MB)
  for ($index = 0; $index -lt $chunk.Length; $index++) { $chunk[$index] = [byte](31 + ($index % 197)) }
  $digest = Digest-Hex $chunk
  $upload = Begin-Snapshot 'chaos/resumable.bin' $digest $chunk.Length $project.id $admin.access_token
  if (@($upload.missing_chunks).Count -ne 1) { throw 'new chaos upload did not request its chunk' }

  Stop-Child $forwarderProcess
  $forwarderProcess = $null
  Assert-Status 500 { Invoke-Chunk $upload.manifest_id $digest $chunk $admin.access_token } 'upload during object-store outage'
  if ((Query-Scalar "SELECT count(*) FROM snapshot_chunks WHERE encode(plaintext_digest, 'hex')='$digest';") -ne '0') {
    throw 'failed object-store upload left partial chunk metadata'
  }

  $forwarderProcess = Start-S3Forwarder
  $receipt = Invoke-Chunk $upload.manifest_id $digest $chunk $admin.access_token
  if ($receipt.deduplicated) { throw 'resumed upload was unexpectedly deduplicated' }
  $manifest = Invoke-Json POST "/snapshots/$($upload.manifest_id)/commit" @{} $admin.access_token
  $download = Invoke-WebRequest -UseBasicParsing -Uri "$apiBase/snapshots/$($manifest.id)/chunks/$digest" `
    -Headers @{ authorization = "Bearer $($admin.access_token)" }
  $downloadBytes = if ($download.Content -is [byte[]]) { $download.Content } else { [Text.Encoding]::Latin1.GetBytes([string]$download.Content) }
  if ((Digest-Hex $downloadBytes) -ne $digest) { throw 'resumed encrypted chunk failed integrity roundtrip' }

  $reserved = Begin-Snapshot 'chaos/reserved.bin' $digest $chunk.Length $project.id $admin.access_token
  if (@($reserved.missing_chunks).Count -ne 0) { throw 'existing chunk was not reserved for resumable upload' }
  Invoke-Json DELETE "/snapshots/$($manifest.id)" $null $admin.access_token | Out-Null
  $workerProcess = Start-Worker 'reservation-worker'
  Start-Sleep -Seconds 2
  Stop-Child $workerProcess
  $workerProcess = $null
  if ((Query-Scalar "SELECT count(*) FROM snapshot_chunks WHERE encode(plaintext_digest, 'hex')='$digest' AND ref_count=0 AND status='ready';") -ne '1') {
    throw 'garbage collection removed a chunk reserved by an active upload'
  }

  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d $databaseName `
    -c "UPDATE snapshot_manifests SET upload_expires_at=now()-interval '1 minute' WHERE id='$($reserved.manifest_id)';" | Out-Null
  $workerProcess = Start-Worker 'abandoned-worker'
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 150; $remaining = Query-Scalar "SELECT count(*) FROM snapshot_chunks WHERE encode(plaintext_digest, 'hex')='$digest';" } `
    while ($remaining -ne '0' -and (Get-Date) -lt $deadline)
  Stop-Child $workerProcess
  $workerProcess = $null
  if ($remaining -ne '0') { throw 'abandoned upload reservation was not released and collected' }

  $chunk2 = [byte[]]::new(512KB)
  for ($index = 0; $index -lt $chunk2.Length; $index++) { $chunk2[$index] = [byte](7 + ($index % 223)) }
  $digest2 = Digest-Hex $chunk2
  $upload2 = Begin-Snapshot 'chaos/delete-retry.bin' $digest2 $chunk2.Length $project.id $admin.access_token
  Invoke-Chunk $upload2.manifest_id $digest2 $chunk2 $admin.access_token | Out-Null
  $manifest2 = Invoke-Json POST "/snapshots/$($upload2.manifest_id)/commit" @{} $admin.access_token
  Invoke-Json DELETE "/snapshots/$($manifest2.id)" $null $admin.access_token | Out-Null

  Stop-Child $forwarderProcess
  $forwarderProcess = $null
  $workerProcess = Start-Worker 'outage-gc-worker'
  Start-Sleep -Seconds 2
  Stop-Child $workerProcess
  $workerProcess = $null
  if ((Query-Scalar "SELECT count(*) FROM snapshot_chunks WHERE encode(plaintext_digest, 'hex')='$digest2' AND ref_count=0 AND status='ready';") -ne '1') {
    throw 'failed object deletion did not return chunk metadata to retryable ready state'
  }

  $forwarderProcess = Start-S3Forwarder
  $workerProcess = Start-Worker 'recovered-gc-worker'
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 150; $remaining = Query-Scalar "SELECT count(*) FROM snapshot_chunks WHERE encode(plaintext_digest, 'hex')='$digest2';" } `
    while ($remaining -ne '0' -and (Get-Date) -lt $deadline)
  Stop-Child $workerProcess
  $workerProcess = $null
  if ($remaining -ne '0') { throw 'object deletion did not recover after storage connectivity returned' }

  Write-Output 'outage_upload_has_no_partial_metadata=ok'
  Write-Output 'resumable_upload_after_outage=ok'
  Write-Output 'encrypted_integrity_roundtrip=ok'
  Write-Output 'active_reservation_blocks_gc=ok'
  Write-Output 'abandoned_reservation_gc=ok'
  Write-Output 'object_delete_retry_after_outage=ok'
} catch {
  foreach ($name in @('server.stderr.log', 'forwarder.stderr.log')) {
    $path = Join-Path $testRoot $name
    if (Test-Path -LiteralPath $path) { Get-Content -LiteralPath $path | Out-Host }
  }
  throw
} finally {
  Stop-Child $workerProcess
  Stop-Child $serverProcess
  Stop-Child $forwarderProcess
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
