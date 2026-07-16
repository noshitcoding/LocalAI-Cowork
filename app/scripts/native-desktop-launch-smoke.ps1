param(
  [string]$ExecutablePath = ""
)

$ErrorActionPreference = "Stop"
$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $appRoot "src-tauri\target\release"))
$candidate = if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
  Join-Path $expectedRoot "app.exe"
} else {
  $ExecutablePath
}
$resolvedExecutable = (Resolve-Path -LiteralPath $candidate).Path
$expectedPrefix = $expectedRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar

if (-not $resolvedExecutable.StartsWith($expectedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Native smoke executable must stay inside $expectedRoot"
}

$process = $null
try {
  $process = Start-Process -FilePath $resolvedExecutable -WorkingDirectory (Split-Path $resolvedExecutable) -PassThru -WindowStyle Hidden
  $deadline = [DateTime]::UtcNow.AddSeconds(20)
  $windowReady = $false

  while ([DateTime]::UtcNow -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $process.Refresh()
    if ($process.HasExited) {
      throw "Open Cowork exited during startup with code $($process.ExitCode)."
    }
    if ($process.MainWindowHandle -ne 0 -and $process.MainWindowTitle -eq "Open Cowork") {
      $windowReady = $true
      break
    }
  }

  if (-not $windowReady) {
    throw "Open Cowork did not expose its expected desktop window within 20 seconds."
  }

  Write-Host "[OK] Native process stayed alive and exposed window '$($process.MainWindowTitle)' (PID $($process.Id))."
} finally {
  if ($null -ne $process) {
    try {
      $process.Refresh()
      if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        Wait-Process -Id $process.Id -Timeout 10 -ErrorAction SilentlyContinue
      }
    } catch {
      Write-Warning "Native smoke cleanup could not confirm process shutdown: $($_.Exception.Message)"
    }
  }
}
