param([switch]$Force)

$ErrorActionPreference = 'Stop'
$secretDirectory = Join-Path $PSScriptRoot 'secrets'
New-Item -ItemType Directory -Path $secretDirectory -Force | Out-Null

function New-RandomHex([int]$bytes) {
  $buffer = [byte[]]::new($bytes)
  $random = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $random.GetBytes($buffer) } finally { $random.Dispose() }
  return ([BitConverter]::ToString($buffer)).Replace('-', '').ToLowerInvariant()
}

function New-RandomBase64([int]$bytes) {
  $buffer = [byte[]]::new($bytes)
  $random = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $random.GetBytes($buffer) } finally { $random.Dispose() }
  return [Convert]::ToBase64String($buffer)
}

function Write-Secret([string]$name, [string]$value) {
  $path = Join-Path $secretDirectory $name
  if ((Test-Path -LiteralPath $path) -and -not $Force) {
    throw "$path already exists; rerun with -Force only when rotating a stopped deployment"
  }
  [IO.File]::WriteAllText($path, $value, [Text.UTF8Encoding]::new($false))
}

$postgresPassword = New-RandomHex 32
Write-Secret 'bootstrap_token.txt' (New-RandomHex 32)
Write-Secret 'postgres_password.txt' $postgresPassword
Write-Secret 'database_url.txt' "postgres://cowork:$postgresPassword@postgres:5432/cowork"
Write-Secret 'minio_root_user.txt' "cowork$(New-RandomHex 8)"
Write-Secret 'minio_root_password.txt' (New-RandomHex 32)
Write-Secret 'runner_signing_key.txt' (New-RandomHex 32)
Write-Secret 'storage_master_key.txt' (New-RandomBase64 32)

Write-Output "Created Open Cowork deployment secrets in $secretDirectory"
