$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_quota_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do { Start-Sleep -Milliseconds 200; try { $result = Invoke-RestMethod -Uri $url } catch { $result = $null } }
  while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}
function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}; if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{ Method = $method; Uri = "http://127.0.0.1:18086/api/v1$path"; Headers = $headers }
  if ($null -ne $body) { $parameters.ContentType = 'application/json'; $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30) }
  return Invoke-RestMethod @parameters
}
function Assert-Status([int]$status, [scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null; throw "$description unexpectedly succeeded" }
  catch { if ($_.Exception.Response.StatusCode.value__ -ne $status) { throw } }
}
function New-RunBody($project, $thread, [string]$key) {
  return @{
    thread_id = $thread.id; project_id = $project.id; project_revision = 1
    project_privacy = $project.privacy; task = $null
    executor_target = @{ kind = 'server_linux'; pool_id = $null }
    required_capabilities = @(); input = @{ prompt = 'quota test' }
    model_profile_id = $null; snapshot_id = $null; idempotency_key = $key
  }
}

try {
  if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch 'open-cowork-postgres-1')) { throw 'PostgreSQL is not running' }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }
  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'; $env:COWORK_LISTEN_ADDR = '127.0.0.1:18086'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'; $env:COWORK_S3_REGION = 'us-east-1'; $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $serverProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18086/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'quota-admin@opencowork.invalid'; display_name = 'Quota Admin'; password = 'Quota-Admin-Password-42!'; device_id = [guid]::NewGuid()
  } $bootstrapToken
  $invite = Invoke-Json POST '/auth/invitations' @{ email = 'quota-user@opencowork.invalid'; expires_at = (Get-Date).ToUniversalTime().AddHours(2).ToString('o') } $admin.access_token
  $user = Invoke-Json POST '/auth/invitations/accept' @{ token = $invite.token; display_name = 'Quota User'; password = 'Quota-User-Password-42!'; device_id = [guid]::NewGuid() }
  $project = Invoke-Json POST '/projects' @{ name = 'Quota private'; description = ''; privacy = 'private_local'; team_id = $null; preferred_executor_target = $null; policy = @{} } $user.access_token
  $thread = Invoke-Json POST '/threads' @{ project_id = $project.id; title = 'Quota thread'; forked_from_thread_id = $null; forked_from_message_id = $null } $user.access_token

  $quota = Invoke-Json PUT "/quotas/user/$($user.user_id)" @{
    storage_bytes = 100; concurrent_runs = 1; monthly_tokens = 10; monthly_cost_micros = 100; hard_cost_limit = $true
  } $admin.access_token
  if ($quota.limits.concurrent_runs -ne 1 -or $quota.limits.storage_bytes -ne 100) { throw 'user quota limits were not stored' }
  Assert-Status 401 { Invoke-Json PUT "/quotas/user/$($user.user_id)" @{ storage_bytes = 100; concurrent_runs = 2; monthly_tokens = 10; monthly_cost_micros = 100; hard_cost_limit = $true } $user.access_token } 'user self quota update'
  Assert-Status 422 { Invoke-Json PUT "/quotas/user/$($user.user_id)" @{ storage_bytes = 100; concurrent_runs = 1; monthly_tokens = $null; monthly_cost_micros = 100; hard_cost_limit = $true } $admin.access_token } 'cost quota without token fallback'

  $idempotency = "quota-idempotent-$([guid]::NewGuid())"
  $run1 = Invoke-Json POST '/runs' (New-RunBody $project $thread $idempotency) $user.access_token
  $retry = Invoke-Json POST '/runs' (New-RunBody $project $thread $idempotency) $user.access_token
  if ($retry.spec.id -ne $run1.spec.id) { throw 'idempotent retry created a second run' }
  Assert-Status 429 { Invoke-Json POST '/runs' (New-RunBody $project $thread "quota-two-$([guid]::NewGuid())") $user.access_token } 'concurrent run quota'
  Invoke-Json POST "/runs/$($run1.spec.id)/cancel" $null $user.access_token | Out-Null
  $run2 = Invoke-Json POST '/runs' (New-RunBody $project $thread "quota-after-cancel-$([guid]::NewGuid())") $user.access_token
  Invoke-Json POST "/runs/$($run2.spec.id)/cancel" $null $user.access_token | Out-Null

  $modified = (Get-Date).ToUniversalTime().ToString('o')
  Assert-Status 429 { Invoke-Json POST '/snapshots' @{
      project_id = $project.id; total_bytes = 101; expires_at = (Get-Date).ToUniversalTime().AddDays(1).ToString('o')
      files = @(@{ path = 'too-large.bin'; size = 101; mode = 420; modified_at = $modified; chunks = @(@{ digest = ('a' * 64); plaintext_size = 101 }) })
    } $user.access_token } 'storage quota'
  $snapshot = Invoke-Json POST '/snapshots' @{
    project_id = $project.id; total_bytes = 100; expires_at = (Get-Date).ToUniversalTime().AddDays(1).ToString('o')
    files = @(@{ path = 'allowed.bin'; size = 100; mode = 420; modified_at = $modified; chunks = @(@{ digest = ('b' * 64); plaintext_size = 100 }) })
  } $user.access_token
  if (-not $snapshot.manifest_id) { throw 'allowed snapshot was not reserved' }
  $usage = Invoke-Json GET "/quotas/user/$($user.user_id)" $null $user.access_token
  if ($usage.usage.storage_bytes -ne 100 -or $usage.usage.running_runs -ne 0) { throw 'live quota usage is incorrect' }

  $team = Invoke-Json POST '/teams' @{ name = 'Quota team' } $user.access_token
  $teamProject = Invoke-Json POST '/projects' @{ name = 'Team quota project'; description = ''; privacy = 'team_managed'; team_id = $team.id; preferred_executor_target = $null; policy = @{} } $user.access_token
  $teamThread = Invoke-Json POST '/threads' @{ project_id = $teamProject.id; title = 'Team quota'; forked_from_thread_id = $null; forked_from_message_id = $null } $user.access_token
  Invoke-Json PUT "/quotas/team/$($team.id)" @{ storage_bytes = $null; concurrent_runs = 0; monthly_tokens = $null; monthly_cost_micros = $null; hard_cost_limit = $true } $user.access_token | Out-Null
  Assert-Status 429 { Invoke-Json POST '/runs' (New-RunBody $teamProject $teamThread "team-quota-$([guid]::NewGuid())") $user.access_token } 'team concurrent run quota'

  $audit = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*) FROM audit_events WHERE action='quota.update';"
  if ([int](($audit -join '').Trim()) -lt 2) { throw 'quota updates were not audited' }
  Write-Output 'quota_idempotency=ok'
  Write-Output 'user_concurrent_runs=ok'
  Write-Output 'team_concurrent_runs=ok'
  Write-Output 'logical_storage_quota=ok'
  Write-Output 'quota_rbac_and_audit=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) { Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue; $serverProcess.WaitForExit() }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
