$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_model_quota_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$requestLog = Join-Path $testRoot 'model-requests.log'
$serverProcess = $null
$modelJob = $null
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
    Uri = "http://127.0.0.1:18089/api/v1$path"
    Headers = $headers
  }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = ($body | ConvertTo-Json -Compress -Depth 30)
  }
  return Invoke-RestMethod @parameters
}

function Wait-Run([string]$runId, [string]$token) {
  $deadline = (Get-Date).AddSeconds(30)
  do {
    Start-Sleep -Milliseconds 150
    $run = Invoke-Json GET "/runs/$runId" $null $token
  } while ($run.state -notin @('completed', 'failed', 'interrupted', 'canceled', 'expired') -and (Get-Date) -lt $deadline)
  if ($run.state -notin @('completed', 'failed', 'interrupted', 'canceled', 'expired')) {
    throw "run $runId did not reach a terminal state"
  }
  return $run
}

function New-RunBody($project, $thread, [string]$key) {
  return @{
    thread_id = $thread.id
    project_id = $project.id
    project_revision = 1
    project_privacy = $project.privacy
    task = $null
    executor_target = @{ kind = 'server_linux'; pool_id = $null }
    required_capabilities = @('model.external')
    input = @{ prompt = 'Return a short deterministic response without tools.' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = $key
  }
}

try {
  if (-not (docker ps --format '{{.Names}}' | Select-String -SimpleMatch 'open-cowork-postgres-1')) {
    throw 'PostgreSQL is not running'
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "CREATE DATABASE $databaseName" | Out-Host
  Push-Location $workspace
  try { cargo build -p cowork-server | Out-Host } finally { Pop-Location }

  $modelJob = Start-Job -ArgumentList $requestLog -ScriptBlock {
    param($requestLog)
    $listener = [Net.HttpListener]::new()
    $listener.Prefixes.Add('http://127.0.0.1:18090/')
    $listener.Start()
    try {
      while ($listener.IsListening) {
        $context = $listener.GetContext()
        if ($context.Request.Url.AbsolutePath -eq '/healthz') {
          $payload = @{ status = 'ok' }
        } elseif ($context.Request.HttpMethod -eq 'POST' -and $context.Request.Url.AbsolutePath -eq '/v1/chat/completions') {
          $reader = [IO.StreamReader]::new($context.Request.InputStream)
          $null = $reader.ReadToEnd()
          $reader.Dispose()
          [IO.File]::AppendAllText($requestLog, "request`n")
          $payload = @{
            choices = @(@{
              message = @{ content = 'Quota accounting response.'; tool_calls = @() }
              finish_reason = 'stop'
            })
            usage = @{ prompt_tokens = 7; completion_tokens = 3 }
          }
        } else {
          $context.Response.StatusCode = 404
          $payload = @{ error = 'not_found' }
        }
        $bytes = [Text.Encoding]::UTF8.GetBytes(($payload | ConvertTo-Json -Compress -Depth 20))
        $context.Response.ContentType = 'application/json'
        $context.Response.ContentLength64 = $bytes.Length
        $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
        $context.Response.Close()
      }
    } finally {
      $listener.Stop()
    }
  }
  Wait-Http 'http://127.0.0.1:18090/healthz' 15

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'all'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18089'
  $env:COWORK_WORKER_POLL_MS = '50'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_MODEL_BASE_URL = 'http://127.0.0.1:18090/v1'
  $env:COWORK_MODEL_API_KEY = 'model-quota-e2e-key'
  $env:COWORK_MODEL_NAME = 'quota-e2e-model'
  $env:COWORK_MODEL_INPUT_COST_MICROS_PER_MILLION = '1000000'
  $env:COWORK_MODEL_OUTPUT_COST_MICROS_PER_MILLION = '2000000'
  $env:COWORK_SERVER_CAPABILITIES = 'model.external'
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_user.txt')).Path
  $env:COWORK_S3_SECRET_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'minio_root_password.txt')).Path
  $env:COWORK_STORAGE_MASTER_KEY_FILE = (Resolve-Path (Join-Path $secretRoot 'storage_master_key.txt')).Path
  $serverProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18089/readyz' 30

  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'model-quota-admin@opencowork.invalid'
    display_name = 'Model Quota Admin'
    password = 'Model-Quota-Admin-Password-42!'
    device_id = [guid]::NewGuid()
  } $bootstrapToken
  $team = Invoke-Json POST '/teams' @{ name = 'Model quota team' } $admin.access_token
  $project = Invoke-Json POST '/projects' @{
    name = 'Model quota project'
    description = ''
    privacy = 'team_managed'
    team_id = $team.id
    preferred_executor_target = $null
    policy = @{}
  } $admin.access_token
  $thread = Invoke-Json POST '/threads' @{
    project_id = $project.id
    title = 'Model quota thread'
    forked_from_thread_id = $null
    forked_from_message_id = $null
  } $admin.access_token

  Invoke-Json PUT "/quotas/team/$($team.id)" @{
    storage_bytes = $null
    concurrent_runs = 1
    monthly_tokens = 5
    monthly_cost_micros = 100
    hard_cost_limit = $true
  } $admin.access_token | Out-Null
  $first = Invoke-Json POST '/runs' (New-RunBody $project $thread "model-quota-first-$([guid]::NewGuid())") $admin.access_token
  $first = Wait-Run $first.spec.id $admin.access_token
  if ($first.state -ne 'completed') { throw "first model run ended in $($first.state): $($first.error.message)" }
  $usage = Invoke-Json GET "/quotas/team/$($team.id)" $null $admin.access_token
  if ($usage.usage.tokens -ne 10 -or $usage.usage.cost_micros -ne 13) {
    throw "model usage was not recorded exactly: tokens=$($usage.usage.tokens), cost=$($usage.usage.cost_micros)"
  }

  $second = Invoke-Json POST '/runs' (New-RunBody $project $thread "model-quota-token-stop-$([guid]::NewGuid())") $admin.access_token
  $second = Wait-Run $second.spec.id $admin.access_token
  if ($second.state -ne 'failed' -or $second.error.message -notmatch 'monthly token quota') {
    throw 'the exhausted token quota did not stop the model before its next request'
  }

  Invoke-Json PUT "/quotas/team/$($team.id)" @{
    storage_bytes = $null
    concurrent_runs = 1
    monthly_tokens = 100
    monthly_cost_micros = 0
    hard_cost_limit = $false
  } $admin.access_token | Out-Null
  $third = Invoke-Json POST '/runs' (New-RunBody $project $thread "model-quota-soft-cost-$([guid]::NewGuid())") $admin.access_token
  $third = Wait-Run $third.spec.id $admin.access_token
  if ($third.state -ne 'completed') { throw 'soft cost quota unexpectedly stopped a model request' }

  Invoke-Json PUT "/quotas/team/$($team.id)" @{
    storage_bytes = $null
    concurrent_runs = 1
    monthly_tokens = 100
    monthly_cost_micros = 26
    hard_cost_limit = $true
  } $admin.access_token | Out-Null
  $fourth = Invoke-Json POST '/runs' (New-RunBody $project $thread "model-quota-hard-cost-$([guid]::NewGuid())") $admin.access_token
  $fourth = Wait-Run $fourth.spec.id $admin.access_token
  if ($fourth.state -ne 'failed' -or $fourth.error.message -notmatch 'monthly cost quota') {
    throw 'the hard cost quota did not stop the model before its next request'
  }

  $requests = if (Test-Path $requestLog) { @(Get-Content $requestLog).Count } else { 0 }
  if ($requests -ne 2) { throw "expected exactly two model requests, observed $requests" }
  Write-Output 'model_usage_accounting=ok'
  Write-Output 'token_quota_preflight=ok'
  Write-Output 'soft_cost_limit=ok'
  Write-Output 'hard_cost_limit=ok'
  Write-Output 'blocked_model_requests=ok'
} catch {
  if (Test-Path (Join-Path $testRoot 'server.stderr.log')) { Get-Content (Join-Path $testRoot 'server.stderr.log') }
  throw
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  if ($modelJob) {
    Stop-Job $modelJob -ErrorAction SilentlyContinue
    Remove-Job $modelJob -Force -ErrorAction SilentlyContinue
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork_model_quota_')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
