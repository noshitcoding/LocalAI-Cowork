$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_storage_pressure_$([guid]::NewGuid().ToString('N'))"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$apiBase = 'http://127.0.0.1:18103/api/v1'
$serverProcess = $null
$workerProcess = $null
$loadProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Stop-Child($process) {
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    $null = $process.WaitForExit(10000)
  }
}

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 150
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

function Invoke-Json([string]$method, [string]$path, $body, [string]$token = '') {
  $headers = @{}
  if ($token) { $headers.authorization = "Bearer $token" }
  $parameters = @{ Method = $method; Uri = "$apiBase$path"; Headers = $headers }
  if ($null -ne $body) {
    $parameters.ContentType = 'application/json'
    $parameters.Body = $body | ConvertTo-Json -Compress -Depth 20
  }
  Invoke-RestMethod @parameters
}

function Query-Scalar([string]$sql) {
  ((docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc $sql) -join '').Trim()
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
  cargo build --manifest-path (Join-Path $workspace 'Cargo.toml') -p cowork-server | Out-Host

  $env:DATABASE_URL = "postgres://cowork:$([IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim())@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_S3_ENDPOINT = 'http://127.0.0.1:19000'
  $env:COWORK_S3_REGION = 'us-east-1'
  $env:COWORK_S3_BUCKET = 'cowork-blobs'
  $env:COWORK_S3_ADDRESSING_STYLE = 'path'
  $env:COWORK_S3_ACCESS_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_user.txt')).Trim()
  $env:COWORK_S3_SECRET_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_password.txt')).Trim()
  $env:COWORK_STORAGE_MASTER_KEY = [IO.File]::ReadAllText((Join-Path $secretRoot 'storage_master_key.txt')).Trim()
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  Remove-Item Env:COWORK_S3_SESSION_TOKEN, Env:COWORK_S3_SESSION_TOKEN_FILE, `
    Env:COWORK_RUNNER_URL, Env:COWORK_RUNNER_SIGNING_KEY, Env:COWORK_PUBLIC_ORIGIN, `
    Env:COWORK_WEBAUTHN_RP_ID, Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue

  $binary = Join-Path $workspace 'target/debug/cowork-server.exe'
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18103'
  $serverProcess = Start-Process -FilePath $binary -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18103/readyz' 30

  $admin = Invoke-Json POST '/auth/bootstrap' @{
    email = 'storage-pressure@opencowork.invalid'
    display_name = 'Storage Pressure'
    password = 'Storage-Pressure-Password-42!'
    device_id = [guid]::NewGuid().ToString()
  } $env:COWORK_BOOTSTRAP_TOKEN
  $project = Invoke-Json POST '/projects' @{
    name = 'Storage pressure project'
    description = ''
    privacy = 'private_local'
    team_id = $null
    preferred_executor_target = $null
    policy = @{}
  } $admin.access_token

  $env:COWORK_MODE = 'worker'
  $env:COWORK_WORKER_POLL_MS = '10'
  $workerProcess = Start-Process -FilePath $binary -WorkingDirectory $workspace -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'worker.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'worker.stderr.log')

  $env:COWORK_STORAGE_PRESSURE_API = $apiBase
  $env:COWORK_STORAGE_PRESSURE_TOKEN = $admin.access_token
  $env:COWORK_STORAGE_PRESSURE_PROJECT_ID = $project.id
  if (-not $env:COWORK_STORAGE_PRESSURE_SECONDS) { $env:COWORK_STORAGE_PRESSURE_SECONDS = '90' }
  if (-not $env:COWORK_STORAGE_PRESSURE_CONCURRENCY) { $env:COWORK_STORAGE_PRESSURE_CONCURRENCY = '8' }
  $pressureOutput = & node (Join-Path $workspace 'scripts/test-storage-pressure.mjs') 2>&1
  $loadExitCode = $LASTEXITCODE
  if ($loadExitCode -ne 0) {
    $pressureOutput | Out-Host
    throw "storage pressure driver failed with code $loadExitCode"
  }
  $pressureOutput | Out-Host
  $serverProcess.Refresh(); $workerProcess.Refresh()
  if ($serverProcess.HasExited) { throw "API exited under storage pressure with code $($serverProcess.ExitCode)" }
  if ($workerProcess.HasExited) { throw "worker exited under storage pressure with code $($workerProcess.ExitCode)" }
  $maximumApiBytes = $serverProcess.PeakWorkingSet64
  $maximumWorkerBytes = $workerProcess.PeakWorkingSet64

  $deadline = (Get-Date).AddSeconds(45)
  do {
    Start-Sleep -Milliseconds 250
    $chunkCount = Query-Scalar 'SELECT count(*) FROM snapshot_chunks;'
  } while ($chunkCount -ne '0' -and (Get-Date) -lt $deadline)
  if ($chunkCount -ne '0') { throw "garbage collection left $chunkCount chunks after pressure stopped" }
  if ((Query-Scalar 'SELECT count(*) FROM snapshot_chunks WHERE ref_count < 0;') -ne '0') {
    throw 'storage pressure produced a negative chunk reference count'
  }
  if ((Query-Scalar "SELECT count(*) FROM snapshot_manifests WHERE status NOT IN ('expired','failed');") -ne '0') {
    throw 'storage pressure left active snapshot manifests'
  }
  if ($maximumApiBytes -gt 1GB -or $maximumWorkerBytes -gt 1GB) {
    throw "storage pressure exceeded the one-GiB process envelope: api=$maximumApiBytes worker=$maximumWorkerBytes"
  }

  Write-Output "sustained_seconds=$env:COWORK_STORAGE_PRESSURE_SECONDS"
  Write-Output "api_peak_bytes=$maximumApiBytes"
  Write-Output "worker_peak_bytes=$maximumWorkerBytes"
  Write-Output 'post_pressure_gc=ok'
  Write-Output 'nonnegative_refcounts=ok'
  Write-Output 'bounded_process_memory=ok'
} catch {
  foreach ($name in @('server.stderr.log', 'worker.stderr.log')) {
    $path = Join-Path $testRoot $name
    if (Test-Path -LiteralPath $path) { Get-Content -LiteralPath $path | Out-Host }
  }
  throw
} finally {
  Stop-Child $loadProcess
  Stop-Child $workerProcess
  Stop-Child $serverProcess
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  if (Test-Path -LiteralPath $testRoot) {
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if (-not $resolvedRoot.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) {
      throw "refusing to remove test directory outside temporary root: $resolvedRoot"
    }
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
  }
}
