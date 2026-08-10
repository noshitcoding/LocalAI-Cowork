$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$signingKey = 'sandbox-security-e2e-signing-key-000000000'
$testId = [guid]::NewGuid().ToString('N')
$runA = [guid]::NewGuid(); $runB = [guid]::NewGuid(); $runReplay = [guid]::NewGuid()
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-sandbox-security-$testId"
$runnerProcess = $null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function New-SignedHeaders([string]$method, [string]$path, [byte[]]$body) {
  $timestamp = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds().ToString(); $nonce = [guid]::NewGuid().ToString()
  $prefix = [Text.Encoding]::UTF8.GetBytes("$timestamp`n$nonce`n$method`n$path`n")
  $payload = [byte[]]::new($prefix.Length + $body.Length); [Array]::Copy($prefix, $payload, $prefix.Length); [Array]::Copy($body, 0, $payload, $prefix.Length, $body.Length)
  $hmac = [Security.Cryptography.HMACSHA256]::new([Text.Encoding]::UTF8.GetBytes($signingKey))
  try { $signature = ([BitConverter]::ToString($hmac.ComputeHash($payload))).Replace('-', '').ToLowerInvariant() } finally { $hmac.Dispose() }
  @{ 'x-cowork-timestamp'=$timestamp; 'x-cowork-nonce'=$nonce; 'x-cowork-signature'=$signature }
}
function Invoke-SignedJson([string]$method, [string]$path, $value) {
  $body = [Text.Encoding]::UTF8.GetBytes(($value | ConvertTo-Json -Compress -Depth 30))
  Invoke-RestMethod -Method $method -Uri "http://127.0.0.1:18094$path" -Headers (New-SignedHeaders $method $path $body) -ContentType application/json -Body $body
}
function Assert-HttpStatus([int]$expected, [scriptblock]$operation, [string]$description) {
  try { & $operation | Out-Null }
  catch {
    $responseProperty = $_.Exception.PSObject.Properties['Response']
    if ($null -eq $responseProperty -or $responseProperty.Value.StatusCode.value__ -ne $expected) { throw }
    return
  }
  throw "$description unexpectedly succeeded"
}
function New-Job([guid]$runId, [string]$script, [string]$network = 'none', $environment = @{}) {
  Invoke-SignedJson POST '/v1/jobs' @{ schema_version=1; run_id=$runId; image='core'; argv=@('/bin/bash','-lc',$script); environment=$environment; stdin_base64=$null; network=$network; limits=@{ memory_bytes=268435456; cpu_nanos=500000000; pids=64; timeout_seconds=30; tmpfs_bytes=67108864; output_bytes=1048576 } }
}

try {
  $env:COWORK_RUNNER_SIGNING_KEY = $signingKey; $env:COWORK_RUNNER_LISTEN_ADDR = '127.0.0.1:18094'
  $env:COWORK_RUNNER_CORE_IMAGE = 'open-cowork-sandbox-core:0.3.0'; $env:COWORK_RUNNER_GUI_IMAGE = 'open-cowork-sandbox-gui:0.3.0'
  $env:COWORK_SANDBOX_EGRESS_NETWORK = 'open-cowork-sandbox-egress'; $env:COWORK_SANDBOX_HTTP_PROXY = 'http://egress-proxy:3128'
  cargo build -p cowork-runner | Out-Host
  $runnerProcess = Start-Process (Resolve-Path 'target/debug/cowork-runner.exe') -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $testRoot stdout.log) -RedirectStandardError (Join-Path $testRoot stderr.log)
  $deadline = (Get-Date).AddSeconds(30)
  do { Start-Sleep -Milliseconds 200; try { $health = Invoke-RestMethod 'http://127.0.0.1:18094/healthz' } catch { $health = $null } } while (-not $health -and (Get-Date) -lt $deadline)
  if (-not $health) { throw 'runner did not become ready' }
  if ($health.sandbox_security.seccomp -ne 'builtin' -or $health.sandbox_security.apparmor_required -ne $false) {
    throw 'runner did not expose its explicit development-host security policy'
  }

  $hardening = New-Job $runA @'
set -eu
test "$(id -u)" = 10001
test ! -e /var/run/docker.sock
! touch /rootfs-write-test 2>/dev/null
test "$(awk '/CapEff/{print $2}' /proc/self/status)" = 0000000000000000
test "$(awk '/^Seccomp:/{print $2}' /proc/self/status)" = 2
test "$(stat -c %a /workspace)" -ge 700
mkdir -p artifacts
printf cross-run-secret > artifacts/secret.txt
'@
  if ($hardening.exit_code -ne 0) { throw "sandbox hardening assertions failed: $($hardening.stderr)" }
  if ($hardening.schema_version -ne 2) { throw 'v1 request was not upgraded to a v2 response' }
  $isolation = New-Job $runB 'test ! -e artifacts/secret.txt && printf isolated'
  if ($isolation.exit_code -ne 0 -or $isolation.stdout -ne 'isolated') { throw 'a run observed another run workspace' }

  $public = New-Job $runB 'curl --fail --silent --show-error --max-time 10 http://example.com >/dev/null' filtered_egress
  if ($public.exit_code -ne 0) { throw "filtered public egress failed: $($public.stderr)" }
  foreach ($target in @('http://169.254.169.254/latest/meta-data/', 'http://postgres:5432/', 'http://host.docker.internal/')) {
    $blocked = New-Job $runB "if curl --fail --silent --max-time 5 '$target' >/dev/null 2>&1; then exit 91; fi" filtered_egress
    if ($blocked.exit_code -ne 0) { throw "private or metadata target was reachable: $target" }
  }

  Assert-HttpStatus 400 { New-Job $runB 'true' filtered_egress @{ HTTP_PROXY='http://attacker.invalid:3128' } } 'proxy override'
  $invalidImage = @{ schema_version=1; run_id=$runB; image='attacker-image'; argv=@('true'); environment=@{}; stdin_base64=$null; network='none'; limits=@{ memory_bytes=268435456; cpu_nanos=500000000; pids=64; timeout_seconds=30; tmpfs_bytes=67108864; output_bytes=1048576 } }
  Assert-HttpStatus 400 { Invoke-SignedJson POST '/v1/jobs' $invalidImage } 'non-allowlisted image'
  Assert-HttpStatus 401 { Invoke-RestMethod -Method POST -Uri 'http://127.0.0.1:18094/v1/jobs' -ContentType application/json -Body '{}' } 'unsigned runner request'

  $replayValue = @{ schema_version=1; run_id=$runReplay; image='core'; argv=@('/bin/bash','-lc','true'); environment=@{}; stdin_base64=$null; network='none'; limits=@{ memory_bytes=268435456; cpu_nanos=500000000; pids=64; timeout_seconds=30; tmpfs_bytes=67108864; output_bytes=1048576 } }
  $replayBody = [Text.Encoding]::UTF8.GetBytes(($replayValue | ConvertTo-Json -Compress -Depth 20)); $replayPath='/v1/jobs'; $replayHeaders=New-SignedHeaders POST $replayPath $replayBody
  Invoke-RestMethod -Method POST -Uri "http://127.0.0.1:18094$replayPath" -Headers $replayHeaders -ContentType application/json -Body $replayBody | Out-Null
  Assert-HttpStatus 401 { Invoke-RestMethod -Method POST -Uri "http://127.0.0.1:18094$replayPath" -Headers $replayHeaders -ContentType application/json -Body $replayBody } 'replayed runner request'

  $exportPath = "/v1/runs/$runB/file?path=artifacts%2Fsecret.txt"; $empty=[byte[]]::new(0)
  Assert-HttpStatus 404 { Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:18094$exportPath" -Headers (New-SignedHeaders GET $exportPath $empty) } 'cross-run artifact export'

  Write-Output 'sandbox_non_root_read_only_caps=ok'
  Write-Output 'sandbox_seccomp_enforced=ok'
  Write-Output 'protocol_v1_request_to_v2_runner=ok'
  Write-Output 'sandbox_docker_socket_absent=ok'
  Write-Output 'sandbox_cross_run_isolation=ok'
  Write-Output 'sandbox_public_egress=ok'
  Write-Output 'sandbox_private_metadata_egress_denied=ok'
  Write-Output 'runner_image_and_proxy_policy=ok'
  Write-Output 'runner_signature_replay_protection=ok'
} catch {
  if (Test-Path (Join-Path $testRoot stderr.log)) { Get-Content (Join-Path $testRoot stderr.log) }
  throw
} finally {
  if ($runnerProcess -and -not $runnerProcess.HasExited) { Stop-Process $runnerProcess.Id -Force -ErrorAction SilentlyContinue; $runnerProcess.WaitForExit() }
  foreach ($runId in @($runA,$runB,$runReplay)) {
    $volume="cowork-run-$($runId.ToString('N'))"; if ((docker volume ls --filter "name=^${volume}$" --format '{{.Name}}') -eq $volume) { docker volume rm $volume | Out-Null }
  }
  if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
