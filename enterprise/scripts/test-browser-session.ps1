$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_browser_session_$([guid]::NewGuid().ToString('N'))"
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

try {
  foreach ($name in @('bootstrap_token.txt', 'postgres_password.txt')) {
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
  try {
    cargo build -p cowork-server | Out-Host
    Push-Location (Join-Path $workspace 'app')
    try { npm run build:web | Out-Host } finally { Pop-Location }
  } finally { Pop-Location }

  $certificate = Join-Path $testRoot 'localhost.crt'
  $privateKey = Join-Path $testRoot 'localhost.key'
  $savedErrorPreference = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    & openssl req -x509 -newkey rsa:2048 -nodes -days 1 `
      -keyout $privateKey -out $certificate -subj '/CN=127.0.0.1' `
      -addext 'subjectAltName=IP:127.0.0.1,DNS:localhost' -config NUL 2>$null
    $opensslExitCode = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $savedErrorPreference
  }
  if ($opensslExitCode -ne 0) { throw 'failed to create browser-session test certificate' }

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18097'
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN_FILE = (Resolve-Path (Join-Path $secretRoot 'bootstrap_token.txt')).Path
  $env:COWORK_WEB_PUSH_ENABLED = 'false'
  $serverProcess = Start-Process -FilePath (Join-Path $workspace 'target/debug/cowork-server.exe') `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot 'server.stdout.log') `
    -RedirectStandardError (Join-Path $testRoot 'server.stderr.log')
  Wait-Http 'http://127.0.0.1:18097/readyz' 30

  $email = 'browser-session-e2e@opencowork.invalid'
  $password = 'Browser-Session-E2E-Password-42!'
  $bootstrapToken = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $headers = @{ authorization = "Bearer $bootstrapToken" }
  $body = @{
    email = $email
    display_name = 'Browser Session E2E'
    password = $password
    device_id = [guid]::NewGuid().ToString()
  } | ConvertTo-Json -Compress
  Invoke-RestMethod -Method POST -Uri 'http://127.0.0.1:18097/api/v1/auth/bootstrap' `
    -Headers $headers -ContentType 'application/json' -Body $body | Out-Null

  $env:COWORK_TEST_API_URL = 'http://127.0.0.1:18097'
  $env:COWORK_TEST_WEB_ORIGIN = 'https://127.0.0.1:18447'
  $env:COWORK_TEST_WEB_DIST = (Resolve-Path (Join-Path $workspace 'app/dist')).Path
  $env:COWORK_TEST_TLS_CERT = $certificate
  $env:COWORK_TEST_TLS_KEY = $privateKey
  $env:COWORK_TEST_EMAIL = $email
  $env:COWORK_TEST_PASSWORD = $password
  Push-Location (Join-Path $workspace 'app')
  try { & node './scripts/test-browser-session.mjs' | Out-Host } finally { Pop-Location }
  if ($LASTEXITCODE -ne 0) { throw "browser-session E2E failed with exit code $LASTEXITCODE" }
} catch {
  if (Test-Path -LiteralPath (Join-Path $testRoot 'server.stderr.log')) {
    Get-Content -LiteralPath (Join-Path $testRoot 'server.stderr.log') | Out-Host
  }
  throw
} finally {
  foreach ($name in @(
    'COWORK_TEST_API_URL', 'COWORK_TEST_WEB_ORIGIN', 'COWORK_TEST_WEB_DIST',
    'COWORK_TEST_TLS_CERT', 'COWORK_TEST_TLS_KEY', 'COWORK_TEST_EMAIL',
    'COWORK_TEST_PASSWORD'
  )) {
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
  }
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
