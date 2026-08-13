$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$migrationRoot = Join-Path $workspace 'server/cowork-server/migrations'
$migrations = @(Get-ChildItem -LiteralPath $migrationRoot -Filter '*.sql' | Sort-Object Name)
$currentMigrationVersion = [int]$migrations[-1].BaseName.Substring(0, 4)
$previousMigrationVersion = $currentMigrationVersion - 1
$secretRoot = Join-Path $workspace 'deploy/secrets'
$databaseName = "cowork_upgrade_$([guid]::NewGuid().ToString('N'))"
$dumpName = "$databaseName.dump"
$containerDump = "/tmp/$dumpName"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) $databaseName
$hostDump = Join-Path $testRoot $dumpName
$serverProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Invoke-Psql([string]$sql, [string]$database = $databaseName) {
  $result = docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d $database -tAc $sql
  if ($LASTEXITCODE -ne 0) { throw "psql failed for database $database" }
  return (($result -join "`n").Trim())
}

function Wait-Http([string]$url, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  do {
    Start-Sleep -Milliseconds 200
    try { $result = Invoke-RestMethod -Uri $url -Method GET } catch { $result = $null }
  } while (-not $result -and (Get-Date) -lt $deadline)
  if (-not $result) { throw "$url did not become ready" }
}

try {
  Invoke-Psql "CREATE DATABASE $databaseName" 'postgres' | Out-Null
  Invoke-Psql @'
CREATE TABLE _sqlx_migrations (
  version BIGINT PRIMARY KEY,
  description TEXT NOT NULL,
  installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
  success BOOLEAN NOT NULL,
  checksum BYTEA NOT NULL,
  execution_time BIGINT NOT NULL
)
'@ | Out-Null

  foreach ($migration in $migrations | Where-Object { [int]$_.BaseName.Substring(0, 4) -le $previousMigrationVersion }) {
    Get-Content -LiteralPath $migration.FullName -Raw | docker exec -i open-cowork-postgres-1 `
      psql -v ON_ERROR_STOP=1 -U cowork -d $databaseName | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "failed to apply $($migration.Name)" }
    $version = [int]$migration.BaseName.Substring(0, 4)
    $description = $migration.BaseName.Substring(5).Replace('_', ' ')
    $checksum = (Get-FileHash -LiteralPath $migration.FullName -Algorithm SHA384).Hash.ToLowerInvariant()
    Invoke-Psql "INSERT INTO _sqlx_migrations(version, description, success, checksum, execution_time) VALUES ($version, '$description', TRUE, decode('$checksum', 'hex'), 0)" | Out-Null
  }
  if ([int](Invoke-Psql 'SELECT max(version) FROM _sqlx_migrations') -ne $previousMigrationVersion) {
    throw 'failed to construct the N-1 migration state'
  }

  $userId = [guid]::NewGuid().ToString()
  $identityId = [guid]::NewGuid().ToString()
  $sessionId = [guid]::NewGuid().ToString()
  $refreshFamilyId = [guid]::NewGuid().ToString()
  $runId = [guid]::NewGuid().ToString()
  Invoke-Psql "INSERT INTO users(id, etag, email, display_name, platform_admin) VALUES ('$userId', 'W/`"$userId`:1`"', 'upgrade-marker@opencowork.invalid', 'Before Upgrade', TRUE)" | Out-Null
  Invoke-Psql "INSERT INTO oidc_identities(id, user_id, issuer, subject) VALUES ('$identityId', '$userId', 'https://upgrade.invalid', 'persistent-subject')" | Out-Null
  Invoke-Psql "INSERT INTO auth_sessions(id, user_id, device_id, refresh_token_hash, refresh_family_id, previous_token_hash, expires_at) VALUES ('$sessionId', '$userId', '$([guid]::NewGuid())', decode(repeat('aa', 32), 'hex'), '$refreshFamilyId', decode(repeat('bb', 32), 'hex'), now()+interval '1 day')" | Out-Null
  Invoke-Psql "INSERT INTO runs(id, thread_id, project_id, creator_user_id, idempotency_key, target_kind, state, spec, created_at, updated_at, finished_at) VALUES ('$runId', '$([guid]::NewGuid())', '$([guid]::NewGuid())', '$userId', 'upgrade-event-cursor', 'server_linux', 'completed', '{}', now()-interval '100 days', now()-interval '100 days', now()-interval '100 days'); INSERT INTO run_events(run_id, sequence, event_id, kind, payload, created_at) VALUES ('$runId', 2, '$([guid]::NewGuid())', 'state_changed', '{}', now()-interval '100 days'), ('$runId', 7, '$([guid]::NewGuid())', 'completed', '{}', now()-interval '100 days');" | Out-Null

  docker exec open-cowork-postgres-1 pg_dump -U cowork -d $databaseName --format=custom --file=$containerDump
  if ($LASTEXITCODE -ne 0) { throw 'N-1 pg_dump failed' }
  docker cp "open-cowork-postgres-1:$containerDump" $hostDump | Out-Host
  if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $hostDump)) { throw 'failed to copy the N-1 backup' }

  $postgresPassword = [IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim()
  $env:DATABASE_URL = "postgres://cowork:$postgresPassword@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN = [IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim()
  $env:COWORK_MODE = 'api'
  $env:COWORK_LISTEN_ADDR = '127.0.0.1:18092'
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
  Wait-Http 'http://127.0.0.1:18092/readyz' 30

  if ([int](Invoke-Psql 'SELECT max(version) FROM _sqlx_migrations') -ne $currentMigrationVersion) {
    throw 'the current server did not migrate N-1 to the current schema'
  }
  if ((Invoke-Psql "SELECT display_name FROM users WHERE id = '$userId'") -ne 'Before Upgrade') {
    throw 'the forward migration did not preserve the existing user'
  }
  if ((Invoke-Psql "SELECT subject FROM oidc_identities WHERE id = '$identityId'") -ne 'persistent-subject') {
    throw 'the forward migration did not preserve the existing OIDC identity'
  }
  if ((Invoke-Psql "SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='runs' AND column_name='next_event_sequence')") -ne 't') {
    throw 'the current migration did not create the durable event sequence cursor'
  }
  if ([int](Invoke-Psql "SELECT next_event_sequence FROM runs WHERE id='$runId'") -ne 8) {
    throw 'the event cursor migration did not preserve the prior maximum sequence'
  }
  if ([int](Invoke-Psql "SELECT count(*) FROM auth_refresh_token_history WHERE session_id='$sessionId' AND token_hash=decode(repeat('bb', 32), 'hex')") -ne 1) {
    throw 'the refresh history migration did not preserve the previous token replay marker'
  }

  Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
  $serverProcess.WaitForExit()
  $serverProcess = $null
  Invoke-Psql "UPDATE users SET display_name = 'After Upgrade Mutation' WHERE id = '$userId'" | Out-Null

  Invoke-Psql "DROP DATABASE $databaseName WITH (FORCE)" 'postgres' | Out-Null
  Invoke-Psql "CREATE DATABASE $databaseName" 'postgres' | Out-Null
  docker cp $hostDump "open-cowork-postgres-1:$containerDump" | Out-Host
  if ($LASTEXITCODE -ne 0) { throw 'failed to stage the rollback dump' }
  docker exec open-cowork-postgres-1 pg_restore --exit-on-error -U cowork -d $databaseName $containerDump
  if ($LASTEXITCODE -ne 0) { throw 'rollback pg_restore failed' }

  if ([int](Invoke-Psql 'SELECT max(version) FROM _sqlx_migrations') -ne $previousMigrationVersion) {
    throw 'rollback did not restore the N-1 migration ledger'
  }
  if ((Invoke-Psql "SELECT display_name FROM users WHERE id = '$userId'") -ne 'Before Upgrade') {
    throw 'rollback did not restore the pre-upgrade data'
  }
  if ((Invoke-Psql "SELECT to_regclass('public.auth_refresh_token_history') IS NULL") -ne 't') {
    throw 'rollback retained the current refresh-history schema that did not exist in the backup'
  }
  if ([int](Invoke-Psql "SELECT max(sequence) FROM run_events WHERE run_id='$runId'") -ne 7) {
    throw 'rollback did not restore the original event sequence history'
  }

  Write-Output 'n_minus_one_schema_fixture=ok'
  Write-Output 'forward_migration_preserves_data=ok'
  Write-Output 'binary_backup_restore=ok'
  Write-Output 'rollback_restores_schema_and_data=ok'
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
    $serverProcess.WaitForExit()
  }
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres `
    -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)" | Out-Host
  docker exec open-cowork-postgres-1 rm -f $containerDump 2>$null
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
