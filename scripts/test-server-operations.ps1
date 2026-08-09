$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_operations_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

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
  $parameters = @{ Method = $method; Uri = "http://127.0.0.1:18091/api/v1$path"; Headers = $headers }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 20)
  }
  return Invoke-RestMethod @parameters
}

function Assert-Unauthorized([scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null; throw "$description unexpectedly succeeded" }
  catch { if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw } }
}

try {
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host
  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18091'
  $env:COWORK_SERVER_CAPABILITIES = 'model.external'
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  Remove-Item Env:COWORK_RUNNER_URL -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_RUNNER_SIGNING_KEY -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_S3_ENDPOINT -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_PUBLIC_ORIGIN -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_WEBAUTHN_RP_ID -ErrorAction SilentlyContinue
  Remove-Item Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue

  cargo build -p cowork-server | Out-Host
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18091/readyz' 30

  $openapi = Invoke-RestMethod 'http://127.0.0.1:18091/api/v1/openapi.json'
  $schemas = Invoke-RestMethod 'http://127.0.0.1:18091/api/v1/schemas/contracts.json'
  if ($openapi.openapi -ne '3.1.0' -or @($openapi.paths.PSObject.Properties).Count -lt 80) {
    throw 'public generated OpenAPI artifact is incomplete'
  }
  if ($schemas.'$schema' -ne 'https://json-schema.org/draft/2020-12/schema' -or @($schemas.'$defs'.PSObject.Properties).Count -lt 40) {
    throw 'public generated JSON Schema artifact is incomplete'
  }

  $admin = Invoke-Json 'POST' '/auth/bootstrap' @{
    email = 'operations-admin@opencowork.invalid'; display_name = 'Operations Admin'
    password = 'Operations-Admin-Password-42!'; device_id = [guid]::NewGuid().ToString()
  } $env:COWORK_BOOTSTRAP_TOKEN
  $invitation = Invoke-Json 'POST' '/auth/invitations' @{
    email = 'operations-member@opencowork.invalid'; expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $admin.access_token
  $member = Invoke-Json 'POST' '/auth/invitations/accept' @{
    token = $invitation.token; display_name = 'Operations Member'; password = 'Operations-Member-Password-42!'
    device_id = [guid]::NewGuid().ToString()
  }

  Assert-Unauthorized { Invoke-Json 'GET' '/operations/metrics' $null $member.access_token } 'member operations access'
  Assert-Unauthorized { Invoke-Json 'GET' '/operations/support-bundle' $null $member.access_token } 'member support bundle export'

  $metrics = Invoke-Json 'GET' '/operations/metrics' $null $admin.access_token
  if ($metrics.application.database_migration_version -ne 19) { throw 'operations metrics returned an unexpected database migration' }
  if ($metrics.database.users -ne 2) { throw 'operations metrics did not return aggregate user counts' }
  if ($metrics.PSObject.Properties.Name -contains 'email') { throw 'operations metrics exposed identities' }

  $bundlePath = Join-Path $testRoot 'support-bundle.json'
  Invoke-WebRequest -Uri 'http://127.0.0.1:18091/api/v1/operations/support-bundle' `
    -Headers @{ authorization = "Bearer $($admin.access_token)" } -OutFile $bundlePath | Out-Null
  $bundleText = [IO.File]::ReadAllText($bundlePath)
  $bundle = $bundleText | ConvertFrom-Json
  if ($bundle.application.database_migration_version -ne 19) { throw 'support bundle is incomplete' }
  foreach ($forbidden in @('operations-admin@', 'Operations Admin', 'password_hash', 'object_key', 'prompt')) {
    if ($bundleText.IndexOf($forbidden, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
      throw "support bundle exposed forbidden content: $forbidden"
    }
  }
  $auditCount = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT count(*) FROM audit_events WHERE action = 'operations.support_bundle.export';"
  if ([int](($auditCount -join '').Trim()) -ne 1) { throw 'support bundle export was not audited exactly once' }

  Write-Output 'operations_admin_rbac=ok'
  Write-Output 'public_openapi_and_json_schema=ok'
  Write-Output 'operations_aggregate_metrics=ok'
  Write-Output 'support_bundle_redaction=ok'
  Write-Output 'support_bundle_audit=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
