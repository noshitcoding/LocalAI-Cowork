$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_pkce_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function ConvertTo-Base64Url([byte[]]$bytes) {
  return [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

function New-PkcePair {
  $bytes = [byte[]]::new(64)
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
  $verifier = ConvertTo-Base64Url $bytes
  $sha = [Security.Cryptography.SHA256]::Create()
  try { $digest = $sha.ComputeHash([Text.Encoding]::ASCII.GetBytes($verifier)) } finally { $sha.Dispose() }
  return @{ verifier = $verifier; challenge = (ConvertTo-Base64Url $digest) }
}

function ConvertFrom-Base32([string]$value) {
  $alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
  $output = [Collections.Generic.List[byte]]::new()
  $buffer = 0
  $bits = 0
  foreach ($character in $value.TrimEnd('=').ToUpperInvariant().ToCharArray()) {
    $index = $alphabet.IndexOf($character)
    if ($index -lt 0) { throw 'invalid base32 secret' }
    $buffer = ($buffer -shl 5) -bor $index
    $bits += 5
    while ($bits -ge 8) {
      $bits -= 8
      $output.Add([byte](($buffer -shr $bits) -band 0xff))
      if ($bits -eq 0) { $buffer = 0 } else { $buffer = $buffer -band ((1 -shl $bits) - 1) }
    }
  }
  return $output.ToArray()
}

function New-TotpCode([string]$secret) {
  $counter = [uint64][Math]::Floor([DateTimeOffset]::UtcNow.ToUnixTimeSeconds() / 30)
  $counterBytes = [BitConverter]::GetBytes($counter)
  if ([BitConverter]::IsLittleEndian) { [Array]::Reverse($counterBytes) }
  $hmac = [Security.Cryptography.HMACSHA1]::new((ConvertFrom-Base32 $secret))
  try { $digest = $hmac.ComputeHash($counterBytes) } finally { $hmac.Dispose() }
  $offset = $digest[19] -band 0x0f
  $binary = ([int64]($digest[$offset] -band 0x7f) * 16777216) + `
    ([int64]$digest[$offset + 1] * 65536) + `
    ([int64]$digest[$offset + 2] * 256) + [int64]$digest[$offset + 3]
  return ($binary % 1000000).ToString('000000')
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
    Uri = "http://127.0.0.1:18083/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 20)
  }
  return Invoke-RestMethod @parameters
}

function Assert-Unauthorized([scriptblock]$operation, [string]$description) {
  try {
    & $operation | Out-Null
    throw "$description unexpectedly succeeded"
  } catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw }
  }
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
  if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch 'open-cowork-postgres-1')) {
    throw 'open-cowork-postgres-1 is not running'
  }

  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host

  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18083'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $env:COWORK_WEBAUTHN_RP_ID = 'localhost'
  $env:COWORK_PUBLIC_ORIGIN = 'http://localhost:18083'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18083/readyz' 30

  $email = 'pkce-e2e@opencowork.invalid'
  $password = 'Native-PKCE-E2E-Password-42!'
  $deviceId = [guid]::NewGuid().ToString()
  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  Invoke-Json 'POST' '/auth/bootstrap' @{
    email = $email
    display_name = 'PKCE E2E'
    password = $password
    device_id = $deviceId
  } $bootstrapToken | Out-Null

  $pair = New-PkcePair
  $authorization = Invoke-Json 'POST' '/auth/native/authorize' @{
    email = $email
    password = $password
    device_id = $deviceId
    code_challenge = $pair.challenge
    code_challenge_method = 'S256'
  }
  $tokens = Invoke-Json 'POST' '/auth/native/token' @{
    code = $authorization.code
    code_verifier = $pair.verifier
    device_id = $deviceId
  }
  if ($tokens.access_token.Length -lt 32 -or $tokens.refresh_token.Length -lt 32) {
    throw 'PKCE exchange did not return valid tokens'
  }

  $env:COWORK_TEST_BASE_URL = 'http://localhost:18083'
  $env:COWORK_TEST_ACCESS_TOKEN = $tokens.access_token
  $env:COWORK_TEST_EMAIL = $email
  $env:COWORK_TEST_DEVICE_ID = $deviceId
  & node (Join-Path $workspace 'app/scripts/test-passkeys.mjs') | Out-Host
  if ($LASTEXITCODE -ne 0) { throw "passkey browser E2E failed with exit code $LASTEXITCODE" }
  Assert-Unauthorized {
    Invoke-Json 'POST' '/auth/native/token' @{
      code = $authorization.code
      code_verifier = $pair.verifier
      device_id = $deviceId
    }
  } 'authorization-code replay'

  $pair = New-PkcePair
  $authorization = Invoke-Json 'POST' '/auth/native/authorize' @{
    email = $email
    password = $password
    device_id = $deviceId
    code_challenge = $pair.challenge
    code_challenge_method = 'S256'
  }
  Assert-Unauthorized {
    Invoke-Json 'POST' '/auth/native/token' @{
      code = $authorization.code
      code_verifier = $pair.verifier
      device_id = [guid]::NewGuid().ToString()
    }
  } 'cross-device code exchange'
  Assert-Unauthorized {
    Invoke-Json 'POST' '/auth/native/token' @{
      code = $authorization.code
      code_verifier = $pair.verifier
      device_id = $deviceId
    }
  } 'device-mismatch code reuse'

  $setup = Invoke-Json 'POST' '/auth/totp/setup' @{} $tokens.access_token
  if ($setup.secret.Length -ne 32 -or -not $setup.otpauth_uri.StartsWith('otpauth://totp/')) {
    throw 'TOTP setup response is invalid'
  }
  $totpCode = New-TotpCode $setup.secret
  $recovery = Invoke-Json 'POST' '/auth/totp/enable' @{ code = $totpCode } $tokens.access_token
  if ($recovery.recovery_codes.Count -ne 10) { throw 'TOTP did not create ten recovery codes' }
  $status = Invoke-Json 'GET' '/auth/totp' $null $tokens.access_token
  if (-not $status.enabled -or $status.unused_recovery_codes -ne 10) { throw 'TOTP status mismatch' }
  $encrypted = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT octet_length(ciphertext) > 20 AND octet_length(encrypted_data_key) > 20 FROM user_totp WHERE user_id='$($tokens.user_id)';"
  if (($encrypted -join '').Trim() -ne 't') { throw 'TOTP seed was not envelope-encrypted' }

  $pair = New-PkcePair
  Assert-Unauthorized {
    Invoke-Json 'POST' '/auth/native/authorize' @{
      email = $email; password = $password; device_id = $deviceId
      code_challenge = $pair.challenge; code_challenge_method = 'S256'
    }
  } 'login without second factor'
  $usedRecoveryCode = $recovery.recovery_codes[0]
  $authorization = Invoke-Json 'POST' '/auth/native/authorize' @{
    email = $email; password = $password; device_id = $deviceId
    code_challenge = $pair.challenge; code_challenge_method = 'S256'
    second_factor = $usedRecoveryCode
  }
  $totpTokens = Invoke-Json 'POST' '/auth/native/token' @{
    code = $authorization.code; code_verifier = $pair.verifier; device_id = $deviceId
  }
  if ($totpTokens.access_token.Length -lt 32) { throw 'second-factor PKCE exchange failed' }
  $pair = New-PkcePair
  Assert-Unauthorized {
    Invoke-Json 'POST' '/auth/native/authorize' @{
      email = $email; password = $password; device_id = $deviceId
      code_challenge = $pair.challenge; code_challenge_method = 'S256'
      second_factor = $usedRecoveryCode
    }
  } 'recovery-code replay'

  Write-Output "pkce_user_id=$($tokens.user_id)"
  Write-Output 's256_exchange=ok'
  Write-Output 'single_use=ok'
  Write-Output 'device_binding=ok'
  Write-Output 'totp_enrollment=ok'
  Write-Output 'totp_encryption=ok'
  Write-Output 'recovery_code_single_use=ok'
} finally {
  Remove-Item Env:COWORK_TEST_ACCESS_TOKEN -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_TEST_BASE_URL -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_TEST_EMAIL -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_TEST_DEVICE_ID -ErrorAction SilentlyContinue
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
