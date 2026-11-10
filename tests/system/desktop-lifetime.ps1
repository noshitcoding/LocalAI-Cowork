$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$desktopExe = Join-Path $root 'apps/desktop/src-tauri/target/debug/localai-cowork-v2-desktop.exe'
$taskHost = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ','').Trim()
$daemonExe = Join-Path $root ('apps/desktop/src-tauri/binaries/cowork-daemon-'+$taskHost+'.exe')
if (Get-NetTCPConnection -LocalPort 43821 -State Listen -ErrorAction SilentlyContinue) { throw 'Port 43821 is occupied. This test never stops an existing daemon.' }
if (-not (Test-Path -LiteralPath $desktopExe)) { throw 'Build the desktop executable first' }
$fixture = Join-Path $root ('.local/desktop-smoke-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
# The desktop must create its own private authority root atomically. Logs live outside it.
$data = Join-Path $fixture 'data'
$env:COWORK_DATA_DIR = $data
$env:COWORK_ENABLE_TEST_PROVIDER = '1'
Remove-Item Env:COWORK_DATABASE_URL -ErrorAction SilentlyContinue
Remove-Item Env:COWORK_WEB_DIR -ErrorAction SilentlyContinue
$desktopProcess = $null
$daemonProcess = $null
function Api($method, $path, $body) {
    $args = @{ Method=$method; Uri=('http://127.0.0.1:43821/api/v2'+$path); Headers=$script:headers; TimeoutSec=5 }
    if ($null -ne $body) { $args.Body=($body | ConvertTo-Json -Depth 15 -Compress); $args.ContentType='application/json' }
    Invoke-RestMethod @args
}
try {
    $desktopProcess = Start-Process -FilePath $desktopExe -WorkingDirectory $root -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'desktop.out') -RedirectStandardError (Join-Path $fixture 'desktop.err')
    $ready = $false
    for ($i=0; $i -lt 80; $i++) {
        if ($desktopProcess.HasExited) { throw ('Desktop exited: '+(Get-Content (Join-Path $fixture 'desktop.err') -Raw)) }
        try {
            $token = [IO.File]::ReadAllText((Join-Path $data 'daemon.token')).Trim()
            $script:headers = @{Authorization=('Bearer '+$token)}
            $null = Api GET '/health' $null
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 250 }
    }
    if (-not $ready) { throw 'The desktop did not start and connect its daemon' }
    $listener = Get-NetTCPConnection -LocalPort 43821 -State Listen
    $daemonProcess = Get-Process -Id $listener.OwningProcess
    if ($daemonProcess.Path -ne $daemonExe) { throw 'Unexpected daemon executable; no external process will be stopped' }
    $project = Api POST '/projects' @{name='Disposable desktop lifecycle test'}
    $order = Api POST '/orders' @{project_id=$project.id; title='Desktop process lifetime'}
    $provider = Api POST '/providers' @{name='Slow test provider';kind='test';endpoint='';model='slow-test';context_tokens=32768}
    $request = @{order_id=$order.id;base_revision=1;idempotency_key=[Guid]::NewGuid().ToString();prompt='Create a deterministic test artifact.';provider_id=$provider.id;provider_revision=1;access_mode='autonomous'}
    $prepared = Api POST '/execution-plans' $request
    if ($prepared.retired -or $prepared.plan.order_id -ne $order.id -or $prepared.plan.idempotency_key -ne $request.idempotency_key) { throw 'The prepared execution plan differs from this test request' }
    $planPath = '/orders/'+[Uri]::EscapeDataString($order.id)+'/execution-plans/'+[Uri]::EscapeDataString($prepared.plan.id)+'/commit'
    $run = Api POST $planPath @{sha256=$prepared.sha256}
    Stop-Process -Id $desktopProcess.Id
    $desktopProcess = $null
    $completed = $false
    for ($i=0; $i -lt 120; $i++) {
        $detail = Api GET ('/runs/'+$run.id) $null
        if ($detail.run.state -eq 'completed') { $completed=$true; break }
        if ($detail.run.state -in @('failed','interrupted','canceled','expired')) { throw ('Run stopped: '+$detail.run.state) }
        Start-Sleep -Milliseconds 250
    }
    if (-not $completed) { throw 'The run did not finish after terminating its desktop process' }
    if (@($detail.artifacts).Count -ne 1) { throw 'Expected exactly one durable artifact' }
    Write-Output 'PASS: Native Windows executable booted its daemon; an accepted run completed after the desktop process was terminated.'
    Write-Output ('Evidence data directory: '+$data)
} finally {
    if ($desktopProcess -and -not $desktopProcess.HasExited) { Stop-Process -Id $desktopProcess.Id }
    if ($daemonProcess -and -not $daemonProcess.HasExited -and $daemonProcess.Path -eq $daemonExe) { Stop-Process -Id $daemonProcess.Id }
}
