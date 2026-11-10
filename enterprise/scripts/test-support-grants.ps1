$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_support_$([guid]::NewGuid().ToString('N'))"
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
  $parameters = @{ Method = $method; Uri = "http://127.0.0.1:18085/api/v1$path"; Headers = $headers }
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

function Assert-Unprocessable([scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null; throw "$description unexpectedly succeeded" }
  catch { if ($_.Exception.Response.StatusCode.value__ -ne 422) { throw } }
}

try {
  if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch 'open-cowork-postgres-1')) {
    throw 'open-cowork-postgres-1 is not running'
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18085'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_SERVER_CAPABILITIES = 'model.external,files'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18085/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $admin = Invoke-Json 'POST' '/auth/bootstrap' @{
    email = 'support-admin@opencowork.invalid'; display_name = 'Support Admin'
    password = 'Support-Admin-Password-42!'; device_id = [guid]::NewGuid().ToString()
  } $bootstrapToken
  $invitation = Invoke-Json 'POST' '/auth/invitations' @{
    email = 'project-owner@opencowork.invalid'; expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o')
  } $admin.access_token
  $owner = Invoke-Json 'POST' '/auth/invitations/accept' @{
    token = $invitation.token; display_name = 'Project Owner'; password = 'Project-Owner-Password-42!'
    device_id = [guid]::NewGuid().ToString()
  }
  $project = Invoke-Json 'POST' '/projects' @{
    name = 'Private support scope'; description = 'must not be visible globally'
    privacy = 'private_local'; team_id = $null; preferred_executor_target = $null; policy = @{}
  } $owner.access_token
  $thread = Invoke-Json 'POST' '/threads' @{
    project_id = $project.id; title = 'Scoped support thread'
    forked_from_thread_id = $null; forked_from_message_id = $null
  } $owner.access_token
  $run = Invoke-Json 'POST' '/runs' @{
    thread_id = $thread.id; project_id = $project.id; project_revision = 1
    project_privacy = 'private_local'; task = $null
    executor_target = @{ kind = 'server_linux'; pool_id = $null }
    required_capabilities = @(); input = @{ prompt = 'support test' }
    model_profile_id = $null; snapshot_id = $null; idempotency_key = "support-$([guid]::NewGuid())"
  } $owner.access_token

  $adminProjects = Invoke-Json 'GET' '/projects' $null $admin.access_token
  if (@($adminProjects).Count -ne 0) { throw 'platform admin received project content without a support grant' }
  Assert-Unauthorized { Invoke-Json 'GET' "/projects/$($project.id)" $null $admin.access_token } 'admin project access without grant'
  Assert-Unauthorized { Invoke-Json 'GET' "/runs/$($run.spec.id)" $null $admin.access_token } 'admin run access without grant'

  $threadGrant = Invoke-Json 'POST' '/support-grants' @{
    support_user_id = $admin.user_id; project_id = $null; thread_id = $thread.id
    reason = 'Investigate a user-reported run issue'; expires_at = (Get-Date).ToUniversalTime().AddHours(1).ToString('o')
  } $owner.access_token
  $supportedRun = Invoke-Json 'GET' "/runs/$($run.spec.id)" $null $admin.access_token
  if ($supportedRun.spec.id -ne $run.spec.id) { throw 'thread-scoped support could not read its run' }
  Assert-Unauthorized { Invoke-Json 'GET' "/projects/$($project.id)" $null $admin.access_token } 'thread grant project-wide access'
  Invoke-Json 'DELETE' "/support-grants/$($threadGrant.id)" $null $admin.access_token | Out-Null
  Assert-Unauthorized { Invoke-Json 'GET' "/runs/$($run.spec.id)" $null $admin.access_token } 'revoked thread support grant'

  $projectGrant = Invoke-Json 'POST' '/support-grants' @{
    support_user_id = $admin.user_id; project_id = $project.id; thread_id = $null
    reason = 'Project-wide support approved by owner'; expires_at = (Get-Date).ToUniversalTime().AddHours(1).ToString('o')
  } $owner.access_token
  $supportedProject = Invoke-Json 'GET' "/projects/$($project.id)" $null $admin.access_token
  if ($supportedProject.id -ne $project.id) { throw 'project support grant did not allow viewer access' }
  Assert-Unprocessable {
    Invoke-Json 'POST' '/support-grants' @{
      support_user_id = $admin.user_id; project_id = $project.id; thread_id = $null
      reason = 'Too long'; expires_at = (Get-Date).ToUniversalTime().AddHours(25).ToString('o')
    } $owner.access_token
  } 'support grant longer than 24 hours'
  Invoke-Json 'DELETE' "/support-grants/$($projectGrant.id)" $null $owner.access_token | Out-Null

  $auditCount = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc `
    "SELECT count(*) FROM audit_events WHERE action IN ('support_grant.create','support_grant.access','support_grant.revoke');"
  if ([int](($auditCount -join '').Trim()) -lt 6) { throw 'support grant audit trail is incomplete' }

  Write-Output 'admin_content_default_deny=ok'
  Write-Output 'thread_support_scope=ok'
  Write-Output 'support_revoke=ok'
  Write-Output 'support_24h_limit=ok'
  Write-Output 'support_audit=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
