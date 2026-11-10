$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.Net.Http

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_oidc_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
$providerProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 200
    try { $result = Invoke-RestMethod -Uri $url } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{
    Method = $method
    Uri = "http://127.0.0.1:18093/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30)
  }
  return Invoke-RestMethod @parameters
}

function Assert-Status([int]$status, [scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null; throw "$description unexpectedly succeeded" }
  catch { if ($_.Exception.Response.StatusCode.value__ -ne $status) { throw } }
}

function ConvertTo-Base64Url([byte[]]$bytes) {
  return [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

function New-Pkce {
  $bytes = New-Object byte[] 64
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
  $verifier = ConvertTo-Base64Url $bytes
  $sha = [Security.Cryptography.SHA256]::Create()
  try { $challenge = ConvertTo-Base64Url ($sha.ComputeHash([Text.Encoding]::ASCII.GetBytes($verifier))) }
  finally { $sha.Dispose() }
  return @{ verifier = $verifier; challenge = $challenge }
}

function Get-Redirect([string]$url) {
  $handler = [System.Net.Http.HttpClientHandler]::new()
  $handler.AllowAutoRedirect = $false
  $client = [System.Net.Http.HttpClient]::new($handler)
  try {
    $response = $client.GetAsync($url).GetAwaiter().GetResult()
    if ([int]$response.StatusCode -notin @(302, 303, 307, 308)) {
      $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
      throw "expected redirect from $url, got $([int]$response.StatusCode): $body"
    }
    return $response.Headers.Location.ToString()
  } finally {
    $client.Dispose()
    $handler.Dispose()
  }
}

function Get-Status([string]$url) {
  $handler = [System.Net.Http.HttpClientHandler]::new()
  $handler.AllowAutoRedirect = $false
  $client = [System.Net.Http.HttpClient]::new($handler)
  try { return [int]($client.GetAsync($url).GetAwaiter().GetResult().StatusCode) }
  finally { $client.Dispose(); $handler.Dispose() }
}

try {
  if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch 'open-cowork-postgres-1')) {
    throw 'PostgreSQL is not running'
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $env:MOCK_OIDC_CLIENT_ID = 'open-cowork-e2e'
  $env:MOCK_OIDC_CLIENT_SECRET = 'open-cowork-e2e-secret'
  $providerProcess = Start-Process node -ArgumentList @('scripts/mock-oidc-provider.mjs', '18092') -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'provider.stdout.log') -RedirectStandardError (Join-Path $testRoot 'provider.stderr.log')
  Wait-Http 'http://127.0.0.1:18092/healthz' 15

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18093'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_PUBLIC_ORIGIN = 'http://127.0.0.1:18093'
  Remove-Item Env:COWORK_WEBAUTHN_RP_ID -ErrorAction SilentlyContinue
  $env:COWORK_OIDC_ISSUER = 'http://127.0.0.1:18092'
  $env:COWORK_OIDC_CLIENT_ID = 'open-cowork-e2e'
  $env:COWORK_OIDC_CLIENT_SECRET = 'open-cowork-e2e-secret'
  $env:COWORK_OIDC_AUTO_PROVISION = 'true'
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $serverProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18093/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'oidc-admin@opencowork.invalid'
    display_name = 'OIDC Admin'
    password = 'Oidc-Admin-Password-42!'
    device_id = [guid]::NewGuid()
  } $bootstrapToken
  $configuration = Invoke-Json GET '/auth/oidc/config' $null
  if (-not $configuration.enabled) { throw 'OIDC configuration is not advertised' }

  $pkce = New-Pkce
  $stateBytes = New-Object byte[] 32
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($stateBytes) } finally { $rng.Dispose() }
  $clientState = ConvertTo-Base64Url $stateBytes
  $deviceId = [guid]::NewGuid()
  $authorization = Invoke-Json POST '/auth/oidc/start' @{
    device_id = $deviceId
    code_challenge = $pkce.challenge
    code_challenge_method = 'S256'
    client_state = $clientState
    redirect_uri = 'http://127.0.0.1:18093/auth/callback'
  }
  Assert-Status 422 { Invoke-Json POST '/auth/oidc/start' @{
      device_id = $deviceId
      code_challenge = $pkce.challenge
      code_challenge_method = 'S256'
      client_state = $clientState
      redirect_uri = 'https://attacker.invalid/callback'
    } } 'unlisted OIDC redirect'

  $providerCallback = Get-Redirect $authorization.authorization_url
  $clientCallback = Get-Redirect $providerCallback
  $clientCallbackUrl = [Uri]$clientCallback
  if ($clientCallbackUrl.GetLeftPart([UriPartial]::Path) -ne 'http://127.0.0.1:18093/auth/callback') {
    throw "OIDC callback escaped the exact client redirect: $clientCallback"
  }
  $query = [System.Web.HttpUtility]::ParseQueryString($clientCallbackUrl.Query)
  if ($query['state'] -ne $clientState) { throw 'OIDC client state was not preserved' }
  $tokens = Invoke-Json POST '/auth/native/token' @{
    code = $query['code']
    code_verifier = $pkce.verifier
    device_id = $deviceId
  }
  if (-not $tokens.access_token -or -not $tokens.refresh_token) { throw 'OIDC did not produce a normal Open Cowork session' }
  if ((Get-Status $providerCallback) -ne 401) { throw 'OIDC provider state was replayable' }
  $null = Invoke-Json GET '/projects' $null $tokens.access_token
  Assert-Status 401 { Invoke-Json POST '/auth/login' @{
      email = 'oidc-user@opencowork.invalid'
      password = 'Not-A-Real-Password-42!'
      second_factor = $null
      device_id = [guid]::NewGuid()
    } } 'password login for OIDC-only account'

  $linkPkce = New-Pkce
  $linkStateBytes = New-Object byte[] 32
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($linkStateBytes) } finally { $rng.Dispose() }
  $linkState = ConvertTo-Base64Url $linkStateBytes
  $linkDevice = [guid]::NewGuid()
  $linkAuthorization = Invoke-Json POST '/auth/oidc/link/start' @{
    device_id = $linkDevice
    code_challenge = $linkPkce.challenge
    code_challenge_method = 'S256'
    client_state = $linkState
    redirect_uri = 'http://127.0.0.1:18093/auth/callback'
  } $admin.access_token
  $linkProviderCallback = Get-Redirect $linkAuthorization.authorization_url
  $linkClientCallback = [Uri](Get-Redirect $linkProviderCallback)
  $linkQuery = [System.Web.HttpUtility]::ParseQueryString($linkClientCallback.Query)
  $linkTokens = Invoke-Json POST '/auth/native/token' @{
    code = $linkQuery['code']
    code_verifier = $linkPkce.verifier
    device_id = $linkDevice
  }
  if ($linkTokens.user_id -ne $admin.user_id) { throw 'OIDC link changed the authenticated account' }

  $identityCount = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*) FROM oidc_identities WHERE issuer='http://127.0.0.1:18092' AND subject='oidc-e2e-user';"
  $auditCount = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*) FROM audit_events WHERE action='auth.oidc.login';"
  $linkAuditCount = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*) FROM audit_events WHERE action='auth.oidc.link';"
  if ([int](($identityCount -join '').Trim()) -ne 1 -or [int](($auditCount -join '').Trim()) -ne 1 -or [int](($linkAuditCount -join '').Trim()) -ne 1) {
    throw 'OIDC identity or audit record is missing'
  }
  Write-Output 'oidc_discovery_and_jwks=ok'
  Write-Output 'oidc_pkce_nonce_id_token=ok'
  Write-Output 'oidc_redirect_allowlist=ok'
  Write-Output 'oidc_state_replay=ok'
  Write-Output 'oidc_auto_provision=ok'
  Write-Output 'oidc_session_exchange=ok'
  Write-Output 'oidc_authenticated_link=ok'
} catch {
  if (Test-Path (Join-Path $testRoot 'provider.stderr.log')) { Get-Content (Join-Path $testRoot 'provider.stderr.log') }
  if (Test-Path (Join-Path $testRoot 'server.stderr.log')) { Get-Content (Join-Path $testRoot 'server.stderr.log') }
  throw
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  if ($providerProcess -and -not $providerProcess.HasExited) {
    Stop-Process -Id $providerProcess.Id -Force -ErrorAction SilentlyContinue
    $providerProcess.WaitForExit()
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork_oidc_')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
