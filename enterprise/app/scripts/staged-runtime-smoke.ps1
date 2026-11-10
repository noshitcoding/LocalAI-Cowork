[CmdletBinding()]
param([string]$ExecutablePath = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Read-only production helper test. Never install, register or launch normal GUI.
$tauriRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../src-tauri'))
$releaseExecutable = if ($ExecutablePath) { (Resolve-Path -LiteralPath $ExecutablePath).Path } else { Join-Path $tauriRoot 'target/release/app.exe' }
if (-not (Test-Path -LiteralPath $releaseExecutable -PathType Leaf)) {
    throw 'Build the NEW release app with cargo build --release --bin app before this smoke.'
}
$layoutFile = Join-Path $tauriRoot 'runtime-layout.json'
$layout = Get-Content -LiteralPath $layoutFile -Raw | ConvertFrom-Json
$config = Get-Content -LiteralPath (Join-Path $tauriRoot 'tauri.conf.json') -Raw | ConvertFrom-Json
if ($layout.schemaVersion -ne 1 -or $layout.target -ne 'windows-x64' -or $layout.appVersion -ne $config.version) {
    throw 'Generate a current Windows runtime-layout.json before this smoke.'
}

$testId = [Guid]::NewGuid().ToString('N')
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
$stagingRoot = Join-Path $temporaryRoot "lacowork-production-smoke-$testId"
$foreignCwd = Join-Path $stagingRoot 'foreign-working-directory'
$forbiddenAppData = Join-Path $stagingRoot 'must-not-create-appdata'
$forbiddenLocalAppData = Join-Path $stagingRoot 'must-not-create-localappdata'
$forbiddenDaemonData = Join-Path $stagingRoot 'must-not-create-daemon-data'

function Resolve-StagingPath {
    param([Parameter(Mandatory = $true)][string]$RelativePath)
    if ([IO.Path]::IsPathRooted($RelativePath) -or $RelativePath.Contains(':') -or @($RelativePath -split '[\\/]' | Where-Object { $_ -in @('', '.', '..') }).Count -gt 0) {
        throw 'Unsafe relative staging path.'
    }
    $result = [IO.Path]::GetFullPath((Join-Path $stagingRoot $RelativePath))
    $prefix = [IO.Path]::GetFullPath($stagingRoot).TrimEnd('\') + '\'
    if (-not $result.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Staging path escaped temporary installation.' }
    return $result
}

function Invoke-StagedHelper {
    param([Parameter(Mandatory = $true)][string]$Arguments, [string]$InputText = '', [int]$ExpectedExitCode = 0)
    if ($Arguments -notin @('--lacowork-runtime-smoke --json', '--lacowork-mcp-websearch')) { throw 'Only the reviewed pre-Tauri CLI modes may run in this smoke.' }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Resolve-StagingPath 'app.exe'
    $start.Arguments = $Arguments
    $start.WorkingDirectory = $foreignCwd
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['LACOWORK_RUNTIME_ROOT'] = Join-Path $foreignCwd 'wrong-virtualized-parent-root'
    $start.EnvironmentVariables['APPDATA'] = $forbiddenAppData
    $start.EnvironmentVariables['LOCALAPPDATA'] = $forbiddenLocalAppData
    $start.EnvironmentVariables['COWORK_DAEMON_DATA_DIR'] = $forbiddenDaemonData
    $start.EnvironmentVariables.Remove('COWORK_DAEMON_IPC_TOKEN')
    $start.EnvironmentVariables.Remove('COWORK_DAEMON_IPC_TOKEN_FILE')
    $start.EnvironmentVariables.Remove('COWORK_MODEL_API_KEY')
    $child = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $child.StandardOutput.ReadToEndAsync()
        $stderr = $child.StandardError.ReadToEndAsync()
        $child.StandardInput.Write($InputText)
        $child.StandardInput.Close()
        if (-not $child.WaitForExit(30000)) { $child.Kill(); $child.WaitForExit(); throw 'Staged helper exceeded 30 seconds.' }
        $output = $stdout.GetAwaiter().GetResult()
        $errors = $stderr.GetAwaiter().GetResult()
        if ($child.ExitCode -ne $ExpectedExitCode) { throw "Staged helper exit mismatch: expected $ExpectedExitCode, got $($child.ExitCode); stderrBytes=$($errors.Length)." }
        if ($errors.Length -ne 0) { throw 'Pre-Tauri helper unexpectedly wrote to stderr.' }
        foreach ($forbidden in @($forbiddenAppData, $forbiddenLocalAppData, $forbiddenDaemonData)) {
            if (Test-Path -LiteralPath $forbidden) { throw 'Pre-Tauri helper unexpectedly initialized application data.' }
        }
        return $output
    } finally { $child.Dispose() }
}

try {
    $null = New-Item -ItemType Directory -Path $stagingRoot
    $null = New-Item -ItemType Directory -Path $foreignCwd
    $artifactCount = 0
    foreach ($entry in $layout.artifacts.PSObject.Properties) {
        $relative = [string]$entry.Value.path
        $destination = Resolve-StagingPath $relative
        $mapping = @($config.bundle.resources.PSObject.Properties | Where-Object {
            $mapped = [string]$_.Value
            $mapped -and ($relative -eq $mapped -or $relative.StartsWith($mapped.TrimEnd('/') + '/', [StringComparison]::Ordinal))
        } | Sort-Object { ([string]$_.Value).Length } -Descending | Select-Object -First 1)
        if ($mapping.Count -ne 1) { throw "No exact source mapping for runtime artifact $($entry.Name)." }
        $mappedDestination = [string]$mapping[0].Value
        $suffix = $relative.Substring($mappedDestination.Length).TrimStart('/')
        $sourceRelative = [string]$mapping[0].Name
        if ($suffix) { $sourceRelative = $sourceRelative.TrimEnd('/') + '/' + $suffix }
        $source = Join-Path $tauriRoot $sourceRelative
        if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne [string]$entry.Value.sha256) { throw "Stale source hash for $($entry.Name); regenerate runtime layout." }
        $null = New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destination)) -Force
        Copy-Item -LiteralPath $source -Destination $destination
        if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne [string]$entry.Value.sha256) { throw "Staged hash mismatch for $($entry.Name)." }
        $artifactCount++
    }
    Copy-Item -LiteralPath $layoutFile -Destination (Resolve-StagingPath 'runtime-layout.json')
    Copy-Item -LiteralPath $releaseExecutable -Destination (Resolve-StagingPath 'app.exe')
    $executableHash = (Get-FileHash -LiteralPath $releaseExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
    if ((Get-FileHash -LiteralPath (Resolve-StagingPath 'app.exe') -Algorithm SHA256).Hash -ne $executableHash) { throw 'Staged app executable hash mismatch.' }

    $result = Invoke-StagedHelper '--lacowork-runtime-smoke --json' | ConvertFrom-Json
    if (-not $result.ok -or $result.appVersion -ne $layout.appVersion -or $result.target -ne $layout.target -or $result.artifactCount -ne $artifactCount) { throw 'Production resource resolver returned inconsistent metadata.' }
    Write-Output 'production_module_root_foreign_cwd=ok'
    Write-Output 'production_ignores_debug_root_override=ok'
    Write-Output "validated_staged_artifacts=$artifactCount"

    $requests = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' + "`n" +
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' + "`n" +
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' + "`n"
    $output = Invoke-StagedHelper '--lacowork-mcp-websearch' -InputText $requests
    $responses = @($output -split "`r?`n" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
    if ($responses.Count -ne 2 -or $responses[0].result.protocolVersion -ne '2024-11-05' -or $responses[1].result.tools[0].name -ne 'search_web') { throw 'Built-in MCP offline stdio handshake failed.' }
    Write-Output 'production_builtin_mcp_offline_handshake=ok'

    # Missing files must fail closed; move only a validated staged copy.
    $manifest = Resolve-StagingPath ([string]$layout.artifacts.daemonManifest.path)
    $preserved = Resolve-StagingPath 'withheld-daemon-manifest.json'
    Move-Item -LiteralPath $manifest -Destination $preserved
    $failure = Invoke-StagedHelper '--lacowork-runtime-smoke --json' -ExpectedExitCode 1 | ConvertFrom-Json
    if ($failure.ok -or $failure.error -notlike 'RUNTIME_RESOURCE_UNREADABLE:*') { throw 'Missing manifest did not fail closed.' }
    Write-Output 'production_missing_payload_fails_closed=ok'
    Write-Output 'pre_tauri_no_appdata_side_effects=ok'
    Write-Output "release_app_sha256=$executableHash"
} finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        $resolved = [IO.Path]::GetFullPath($stagingRoot)
        if (-not $resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -ne "lacowork-production-smoke-$testId") { throw 'Refusing cleanup outside the exact temporary staging root.' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
