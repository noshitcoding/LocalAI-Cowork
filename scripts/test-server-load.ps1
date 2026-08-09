param(
  [ValidateRange(2, 500)][int]$UserCount = 100,
  [ValidateRange(1, 100)][int]$ParallelRuns = 25
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.Net.Http
if ($ParallelRuns -gt $UserCount) { throw 'ParallelRuns cannot exceed UserCount' }

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_load_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do { Start-Sleep -Milliseconds 200; try { $result = Invoke-RestMethod $url } catch { $result = $null } }
  while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}
function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}; if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{ Method = $method; Uri = "http://127.0.0.1:18093/api/v1$path"; Headers = $headers }
  if ($null -ne $body) { $parameters.ContentType = 'application/json'; $parameters.Body = $body | ConvertTo-Json -Compress -Depth 20 }
  Invoke-RestMethod @parameters
}
function Start-JsonRequest([System.Net.Http.HttpClient]$client, [string]$method, [string]$path, [string]$token, $body = $null) {
  $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::new($method), "http://127.0.0.1:18093/api/v1$path")
  $request.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new('Bearer', $token)
  if ($null -ne $body) {
    $json = $body | ConvertTo-Json -Compress -Depth 20
    $request.Content = [System.Net.Http.StringContent]::new($json, [Text.Encoding]::UTF8, 'application/json')
  }
  @{ Request = $request; Task = $client.SendAsync($request); Started = [Diagnostics.Stopwatch]::StartNew() }
}

try {
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "CREATE DATABASE $databaseName" | Out-Host
  $password = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:DATABASE_URL = "postgres://cowork:$password@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_MODE = 'api'; $env:COWORK_LISTEN_ADDR = '127.0.0.1:18093'; $env:COWORK_DATABASE_MAX_CONNECTIONS = '40'
  $env:COWORK_SERVER_CAPABILITIES = 'model.external'; $env:COWORK_WEB_PUSH_ENABLED = 'false'
  Remove-Item Env:COWORK_RUNNER_URL,Env:COWORK_RUNNER_SIGNING_KEY,Env:COWORK_S3_ENDPOINT,Env:COWORK_PUBLIC_ORIGIN,Env:COWORK_WEBAUTHN_RP_ID,Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue
  cargo build -p cowork-server | Out-Host
  $serverProcess = Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18093/readyz' 30

  $admin = Invoke-Json POST '/auth/bootstrap' @{ email='load-0@opencowork.invalid'; display_name='Load User 0'; password='Load-Test-Password-42!'; device_id=[guid]::NewGuid() } $env:COWORK_BOOTSTRAP_TOKEN
  $tokens = [Collections.Generic.List[string]]::new(); $tokens.Add($admin.access_token)
  $users = [Collections.Generic.List[string]]::new(); $users.Add($admin.user_id)
  $team = Invoke-Json POST '/teams' @{ name='100-user load team' } $admin.access_token
  for ($index = 1; $index -lt $UserCount; $index++) {
    $invite = Invoke-Json POST '/auth/invitations' @{ email="load-$index@opencowork.invalid"; expires_at=(Get-Date).ToUniversalTime().AddHours(2).ToString('o') } $admin.access_token
    $session = Invoke-Json POST '/auth/invitations/accept' @{ token=$invite.token; display_name="Load User $index"; password='Load-Test-Password-42!'; device_id=[guid]::NewGuid() }
    $tokens.Add($session.access_token); $users.Add($session.user_id)
    Invoke-Json POST "/teams/$($team.id)/members" @{ user_id=$session.user_id; role='member' } $admin.access_token | Out-Null
  }
  $project = Invoke-Json POST '/projects' @{ name='Load project'; description=''; privacy='team_managed'; team_id=$team.id; preferred_executor_target=$null; policy=@{} } $admin.access_token
  for ($index = 1; $index -lt $UserCount; $index++) {
    Invoke-Json POST "/projects/$($project.id)/members" @{ user_id=$users[$index]; role='runner' } $admin.access_token | Out-Null
  }
  $threads = [Collections.Generic.List[object]]::new()
  for ($index = 0; $index -lt $ParallelRuns; $index++) {
    $threads.Add((Invoke-Json POST '/threads' @{ project_id=$project.id; title="Parallel $index"; forked_from_thread_id=$null; forked_from_message_id=$null } $tokens[$index]))
  }

  $client = [System.Net.Http.HttpClient]::new(); $client.Timeout = [TimeSpan]::FromSeconds(30)
  $active = @()
  for ($index = 0; $index -lt $UserCount; $index++) { $active += Start-JsonRequest $client GET '/projects' $tokens[$index] }
  [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($active.Task))
  $latencies = @()
  foreach ($entry in $active) {
    $entry.Started.Stop(); $latencies += $entry.Started.Elapsed.TotalMilliseconds
    if (-not $entry.Task.Result.IsSuccessStatusCode) { throw "active-user request failed with $($entry.Task.Result.StatusCode)" }
    $entry.Task.Result.Dispose(); $entry.Request.Dispose()
  }
  $sorted = @($latencies | Sort-Object); $p95 = $sorted[[Math]::Min($sorted.Count - 1, [Math]::Ceiling($sorted.Count * 0.95) - 1)]
  if ($p95 -gt 10000) { throw "active-user request p95 exceeded 10 seconds: $p95 ms" }

  $requests = @()
  for ($index = 0; $index -lt $ParallelRuns; $index++) {
    $body = @{ thread_id=$threads[$index].id; project_id=$project.id; project_revision=1; project_privacy='team_managed'; task=$null; executor_target=@{ kind='server_linux'; pool_id=$null }; required_capabilities=@(); input=@{ prompt="parallel load $index" }; model_profile_id=$null; snapshot_id=$null; idempotency_key="load-$([guid]::NewGuid())" }
    $requests += Start-JsonRequest $client POST '/runs' $tokens[$index] $body
  }
  [Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($requests.Task))
  foreach ($entry in $requests) {
    if (-not $entry.Task.Result.IsSuccessStatusCode) { throw "parallel run creation failed with $($entry.Task.Result.StatusCode): $($entry.Task.Result.Content.ReadAsStringAsync().Result)" }
    $entry.Task.Result.Dispose(); $entry.Request.Dispose()
  }
  $client.Dispose()
  $queued = docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*) FROM runs WHERE state='queued';"
  if ([int](($queued -join '').Trim()) -ne $ParallelRuns) { throw 'not every parallel thread produced an independently runnable run' }

  Write-Output "active_users=$UserCount"
  Write-Output "active_user_p95_ms=$([Math]::Round($p95, 1))"
  Write-Output "parallel_runs=$ParallelRuns"
  Write-Output 'load_acceptance=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) { Stop-Process $serverProcess.Id -Force -ErrorAction SilentlyContinue; $serverProcess.WaitForExit() }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
