param()
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$appRoot = Join-Path $repo 'app'
$executable = Join-Path $appRoot 'src-tauri\target\debug\app.exe'
$private = Join-Path $repo '.internal\enterprise-local'
if (!(Test-Path -LiteralPath $executable)) { throw 'Build the desktop shell first: cd app; npm run tauri dev' }
New-Item -ItemType Directory -Force -Path $private | Out-Null
$listener = Get-NetTCPConnection -LocalPort 5173 -State Listen -ErrorAction SilentlyContinue
if ($listener) {
    $response = Invoke-WebRequest -UseBasicParsing 'http://127.0.0.1:5173/'
    if ($response.Content -notmatch 'Local AI Cowork') { throw 'Port 5173 is occupied by another application' }
} else {
    $node = (Get-Command node.exe).Source
    $vite = Join-Path $appRoot 'node_modules\vite\bin\vite.js'
    if (!(Test-Path -LiteralPath $vite)) { throw 'Install frontend dependencies first: cd app; npm ci' }
    $null = Start-Process -FilePath $node -ArgumentList @(('"' + $vite + '"'), '--host', '127.0.0.1', '--port', '5173', '--strictPort') -WorkingDirectory $appRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $private 'desktop-vite.log') -RedirectStandardError (Join-Path $private 'desktop-vite.err.log') -PassThru
    $ready = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        try { $null = Invoke-WebRequest -UseBasicParsing 'http://127.0.0.1:5173/' -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 500 }
    }
    if (!$ready) { throw 'The desktop frontend did not start; inspect .internal/enterprise-local/desktop-vite.err.log' }
}
$existing = Get-Process app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $executable }
if (!$existing) { Start-Process -FilePath $executable -WorkingDirectory (Join-Path $appRoot 'src-tauri') }
Write-Host 'Development desktop ready. Open Server from the app menu (Ctrl+8).'
