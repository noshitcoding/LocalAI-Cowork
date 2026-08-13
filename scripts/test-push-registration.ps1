$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_push_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function ConvertTo-Base64Url([byte[]]$bytes) {
  return [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

function New-WebPushKeys {
  $curve = [Security.Cryptography.ECCurve]::CreateFromFriendlyName('nistP256')
  $key = [Security.Cryptography.ECDiffieHellman]::Create($curve)
  try {
    $parameters = $key.ExportParameters($false)
    $public = [byte[]]::new(65)
    $public[0] = 4
    [Array]::Copy($parameters.Q.X, 0, $public, 1, 32)
    [Array]::Copy($parameters.Q.Y, 0, $public, 33, 32)
  } finally {
    $key.Dispose()
  }
  $auth = [byte[]]::new(16)
  $random = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $random.GetBytes($auth) } finally { $random.Dispose() }
  return @{ p256dh = ConvertTo-Base64Url $public; auth = ConvertTo-Base64Url $auth }
}

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
    Uri = "http://127.0.0.1:18084/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 20)
  }
  return Invoke-RestMethod @parameters
}

try {
  foreach ($name in @(
    'bootstrap_token.txt', 'postgres_password.txt', 'minio_root_user.txt',
    'minio_root_password.txt', 'storage_master_key.txt'
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $secretRoot $name))) {
      throw "missing deployment secret $name; run deploy/init-secrets.ps1 first"
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

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18084'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $env:COWORK_WEB_PUSH_ENABLED = 'true'
  $env:COWORK_WEB_PUSH_SUBJECT = 'mailto:push-e2e@opencowork.invalid'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18084/readyz' 30

  $deviceId = [guid]::NewGuid().ToString()
  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $tokens = Invoke-Json 'POST' '/auth/bootstrap' @{
    email = 'push-e2e@opencowork.invalid'
    display_name = 'Push E2E'
    password = 'Push-Registration-E2E-Password-42!'
    device_id = $deviceId
  } $bootstrapToken
  $configuration = Invoke-Json 'GET' '/push/config' $null $tokens.access_token
  if (-not $configuration.web_push_public_key -or $configuration.web_push_public_key.Length -lt 80) {
    throw 'server did not expose a valid VAPID public key'
  }

  $keys = New-WebPushKeys
  $endpoint = "https://push.example.invalid/subscription/$([guid]::NewGuid().ToString('N'))"
  $subscription = Invoke-Json 'POST' '/push/subscriptions' @{
    device_id = $deviceId
    provider = 'web_push'
    endpoint = $endpoint
    p256dh = $keys.p256dh
    auth = $keys.auth
  } $tokens.access_token
  $listed = @(Invoke-Json 'GET' '/push/subscriptions' $null $tokens.access_token)
  if ($listed.Count -ne 1 -or $listed[0].id -ne $subscription.id) {
    throw 'registered WebPush subscription was not listed'
  }
  $databaseEvidence = docker exec open-cowork-postgres-1 psql -At -U cowork -d $databaseName `
    -c "SELECT provider || ':' || octet_length(ciphertext) || ':' || octet_length(encrypted_data_key) FROM push_subscriptions WHERE id = '$($subscription.id)'"
  if ($databaseEvidence -notmatch '^web_push:[1-9][0-9]+:[1-9][0-9]+$') {
    throw "push subscription was not stored as ciphertext: $databaseEvidence"
  }
  if (($subscription | ConvertTo-Json -Compress) -match [regex]::Escape($endpoint)) {
    throw 'subscription API leaked its endpoint'
  }

  Invoke-Json 'DELETE' "/push/subscriptions/$($subscription.id)" $null $tokens.access_token | Out-Null
  $afterRevocation = Invoke-Json 'GET' '/push/subscriptions' $null $tokens.access_token
  if ($null -ne $afterRevocation -and $afterRevocation.Count -ne 0) {
    throw "revoked push subscription remains active: $($afterRevocation | ConvertTo-Json -Compress -Depth 10)"
  }
  Write-Output "subscription_id=$($subscription.id)"
  Write-Output "encrypted_record=$databaseEvidence"
  Write-Output 'endpoint_redaction=ok'
  Write-Output 'revocation=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) {
    Remove-Item -LiteralPath $testRoot -Recurse -Force
  }
}
