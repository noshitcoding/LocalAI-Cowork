param(
  [int]$WorkerCount = 4
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($WorkerCount -lt 2 -or $WorkerCount -gt 16) {
  throw 'WorkerCount must be between 2 and 16.'
}

$workspace = Split-Path -Parent $PSScriptRoot
$serverBinary = Join-Path $workspace 'target\debug\cowork-server.exe'
if (-not (Test-Path -LiteralPath $serverBinary -PathType Leaf)) {
  throw "The worker soak requires $serverBinary. Build cowork-server first."
}

$node = (Get-Command node -ErrorAction Stop).Source
if (-not $env:DATABASE_URL) {
  if (-not $env:COWORK_TEST_POSTGRES_PASSWORD) {
    throw 'DATABASE_URL or COWORK_TEST_POSTGRES_PASSWORD is required.'
  }
  $databaseUri = [UriBuilder]::new('postgres', '127.0.0.1', 5432, 'cowork_worker_soak_ci')
  $databaseUri.UserName = 'cowork'
  $databaseUri.Password = $env:COWORK_TEST_POSTGRES_PASSWORD
  $env:DATABASE_URL = $databaseUri.Uri.AbsoluteUri
}
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-worker-soak-$([guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($testRoot) | Out-Null
$processes = [Collections.Generic.List[Diagnostics.Process]]::new()
$failed = $false

function Start-SoakProcess {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$ArgumentList = @(),
    [Parameter(Mandatory = $true)][string]$LogName
  )

  $stdout = Join-Path $testRoot "$LogName.stdout.log"
  $stderr = Join-Path $testRoot "$LogName.stderr.log"
  $process = Start-Process -FilePath $FilePath `
    -ArgumentList $ArgumentList `
    -WorkingDirectory $workspace `
    -WindowStyle Hidden `
    -RedirectStandardOutput $stdout `
    -RedirectStandardError $stderr `
    -PassThru
  $processes.Add($process)
  return $process
}

function Test-Endpoint {
  param([Parameter(Mandatory = $true)][string]$Uri)

  try {
    $response = Invoke-WebRequest -Uri $Uri -Method Get -UseBasicParsing -TimeoutSec 2
    return $response.StatusCode -eq 200
  } catch {
    return $false
  }
}

function Show-SoakLogs {
  Get-ChildItem -LiteralPath $testRoot -Filter '*.log' | Sort-Object Name | ForEach-Object {
    Write-Output "==> $($_.Name)"
    Get-Content -LiteralPath $_.FullName
  }
}

try {
  $env:COWORK_MODE = 'api'
  Start-SoakProcess -FilePath $node -ArgumentList @('scripts/fake-soak-runner.mjs') -LogName 'runner' | Out-Null
  Start-SoakProcess -FilePath $serverBinary -LogName 'api' | Out-Null

  $ready = $false
  for ($attempt = 0; $attempt -lt 150; $attempt += 1) {
    if ((Test-Endpoint 'http://127.0.0.1:18098/healthz') -and (Test-Endpoint 'http://127.0.0.1:18099/readyz')) {
      $ready = $true
      break
    }
    Start-Sleep -Milliseconds 200
  }
  if (-not $ready) {
    throw 'The API or fake runner did not become ready.'
  }

  for ($index = 0; $index -lt $WorkerCount; $index += 1) {
    $env:COWORK_MODE = 'worker'
    $env:COWORK_WORKER_ID = "00000000-0000-4000-8000-$('{0:D12}' -f (71 + $index))"
    Start-SoakProcess -FilePath $serverBinary -LogName "worker-$index" | Out-Null
  }

  Start-Sleep -Seconds 1
  foreach ($process in $processes) {
    $process.Refresh()
    if ($process.HasExited) {
      throw "A soak process exited early with code $($process.ExitCode)."
    }
  }

  $env:COWORK_TEST_WORKER_COUNT = $WorkerCount.ToString([Globalization.CultureInfo]::InvariantCulture)
  & $node 'scripts/test-worker-soak.mjs'
  if ($LASTEXITCODE -ne 0) {
    throw "The worker soak acceptance exited with code $LASTEXITCODE."
  }
} catch {
  $failed = $true
  [Console]::Error.WriteLine($_.Exception.ToString())
  Show-SoakLogs
} finally {
  foreach ($process in $processes) {
    try {
      $process.Refresh()
      if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit(5000) | Out-Null
      }
    } catch {
      Write-Warning "Could not stop soak process $($process.Id): $_"
    }
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $resolvedTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($resolvedTemp, [StringComparison]::OrdinalIgnoreCase) -and
      (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-worker-soak-', [StringComparison]::Ordinal)) {
    [IO.Directory]::Delete($resolvedRoot, $true)
  }
}

if ($failed) {
  exit 1
}
