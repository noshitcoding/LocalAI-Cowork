$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$source=Join-Path $PSScriptRoot 'nsis_smoke.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($source,[ref]$tokens,[ref]$errors)
if(@($errors).Count){throw ('PowerShell syntax errors: '+@($errors).Count)}
$definitions=$ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)
. ([scriptblock]::Create(($definitions|ForEach-Object {$_.Extent.Text}) -join "`n"))
$passed=0
function Check($value,[string]$message){if(-not $value){throw $message};$script:passed++}
function Refuses([scriptblock]$body){$rejected=$false;try{& $body}catch{$rejected=$true};Check $rejected 'Unsafe fixture input was accepted'}
$directory=Join-Path ([IO.Path]::GetTempPath()) ('nsis-helper-unit-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $directory
$path=Join-Path $directory 'synthetic.bin'
try {
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::UTF8.GetBytes('synthetic nonexecutable payload'))
    $pin=New-Pin $path
    Check ((Assert-Pin $path $pin).sha256 -ceq $pin.sha256) 'Matching synthetic bytes were rejected'
    Refuses {Assert-Pin $path @{size=$pin.size;sha256=('0'*64)}}
    Refuses {Assert-Pin $path @{size=($pin.size+1);sha256=$pin.sha256}}
    Refuses {Assert-Pin $path @{size=$true;sha256=$pin.sha256}}
    Refuses {Absolute-Ordinary (Join-Path $directory '..\synthetic.bin')}
    Refuses {Absolute-Ordinary ($path+':stream')}
    Refuses {Package-Path $directory '../package.exe'}
    Refuses {Package-Path $directory 'package.exe:stream'}
    Check ((Package-Path $directory 'LocalAI_2.0.0-alpha.8_x64-setup.exe') -eq (Join-Path $directory 'LocalAI_2.0.0-alpha.8_x64-setup.exe')) 'Valid package basename was rejected'
    foreach($state in @('queued','running')) {
        Check ((Assert-LiveRunBeforeGuiExit ([pscustomobject]@{run=[pscustomobject]@{id='own-run';state=$state}}) 'own-run') -ceq $state) 'An active exact synthetic run was rejected'
    }
    foreach($state in @('completed','failed','interrupted','canceled','expired','unknown')) {
        Refuses {Assert-LiveRunBeforeGuiExit ([pscustomobject]@{run=[pscustomobject]@{id='own-run';state=$state}}) 'own-run'}
    }
    Refuses {Assert-LiveRunBeforeGuiExit ([pscustomobject]@{run=[pscustomobject]@{id='foreign-run';state='running'}}) 'own-run'}
    # Synthetic bytes only: one real marker plus an unchanged NSS constant.
    $cargo='prefix __TAURI_BUNDLE_TYPE_VAR_UNK suffix __TAURI_BUNDLE_TYPE_VAR_NSS constant'
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($cargo))
    $desktopPin=New-Pin $path;$desktopPin.marker_policy='tauri-cli-2.11.4-single-bundle-marker/1';$desktopPin.allowed_marker='NSS'
    $exact=Assert-DesktopPayload $path $desktopPin
    Check ($exact.comparison -ceq 'exact_cargo' -and $exact.pin.sha256 -ceq $desktopPin.sha256) 'Exact Cargo desktop was rejected'
    $patched=$cargo.Replace('__TAURI_BUNDLE_TYPE_VAR_UNK','__TAURI_BUNDLE_TYPE_VAR_NSS')
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($patched))
    $normalized=Assert-DesktopPayload $path $desktopPin
    Check ($normalized.comparison -ceq 'single_nss_to_unk' -and $normalized.candidates_checked -eq 2 -and $normalized.normalized_marker_offset -eq 7) 'One candidate must reproduce the full Cargo SHA while leaving the other constant unchanged'
    Check ($normalized.pin.sha256 -ceq (New-Pin $path).sha256 -and $normalized.pin.sha256 -cne $desktopPin.sha256) 'Installed raw identity was replaced with the normalized reference'
    Check ((Assert-Pin $path $normalized.pin).sha256 -ceq $normalized.pin.sha256) 'Raw installed process pin was rejected'
    Refuses {Assert-Pin $path $desktopPin}
    $invalidPolicy=$desktopPin.Clone();$invalidPolicy.marker_policy='unreviewed/1'
    Refuses {Assert-DesktopPayload $path $invalidPolicy}
    $invalidKind=$desktopPin.Clone();$invalidKind.allowed_marker='DEB'
    Refuses {Assert-DesktopPayload $path $invalidKind}
    $invalidSha=$desktopPin.Clone();$invalidSha.sha256='0'*64
    Refuses {Assert-DesktopPayload $path $invalidSha}
    foreach($wrong in @($cargo.Replace('__TAURI_BUNDLE_TYPE_VAR_UNK','__TAURI_BUNDLE_TYPE_VAR_DEB'),$cargo.Replace('__TAURI_BUNDLE_TYPE_VAR_UNK','__TAURI_BUNDLE_TYPE_VAR_APP'),$patched.Replace('prefix','Prefix'),($patched+'x'),$patched.Substring(1))) {
        [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($wrong))
        Refuses {Assert-DesktopPayload $path $desktopPin}
    }
    $twoCargo='__TAURI_BUNDLE_TYPE_VAR_UNK __TAURI_BUNDLE_TYPE_VAR_UNK'
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($twoCargo))
    $twoPin=New-Pin $path;$twoPin.marker_policy=$desktopPin.marker_policy;$twoPin.allowed_marker='NSS'
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($twoCargo.Replace('UNK','NSS')))
    Refuses {Assert-DesktopPayload $path $twoPin}
    $manyCargo='__TAURI_BUNDLE_TYPE_VAR_UNK'+(' __TAURI_BUNDLE_TYPE_VAR_NSS'*64)
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($manyCargo))
    $manyPin=New-Pin $path;$manyPin.marker_policy=$desktopPin.marker_policy;$manyPin.allowed_marker='NSS'
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($manyCargo.Replace('UNK','NSS')))
    Refuses {Assert-DesktopPayload $path $manyPin}
    # Model a file swap after the initial raw pin, without native execution.
    [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($patched.Replace('prefix','Prefix')))
    $script:desktopMutationBytes=[Text.Encoding]::ASCII.GetBytes($patched)
    $script:originalDesktopNewPin=${function:New-Pin}
    try {
        function New-Pin([string]$Path) {
            $snapshot=& $script:originalDesktopNewPin $Path
            [IO.File]::WriteAllBytes($Path,$script:desktopMutationBytes)
            return $snapshot
        }
        Refuses {Assert-DesktopPayload $path $desktopPin}
    } finally {${function:New-Pin}=$script:originalDesktopNewPin}
    # Model a later swap immediately before the final independent rehash.
    $script:originalDesktopAssertPin=${function:Assert-Pin}
    $script:desktopMutationBytes=[Text.Encoding]::ASCII.GetBytes($patched.Replace('prefix','Prefix'))
    try {
        function Assert-Pin([string]$Path,$Pin) {
            [IO.File]::WriteAllBytes($Path,$script:desktopMutationBytes)
            return & $script:originalDesktopAssertPin $Path $Pin
        }
        [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($patched))
        Refuses {Assert-DesktopPayload $path $desktopPin}
        [IO.File]::WriteAllBytes($path,[Text.Encoding]::ASCII.GetBytes($cargo))
        Refuses {Assert-DesktopPayload $path $desktopPin}
    } finally {${function:Assert-Pin}=$script:originalDesktopAssertPin}
    $text=[IO.File]::ReadAllText($source)
    Check ($text.Contains('$null=Assert-Pin $desktop.Image $script:desktopRawPin') -and $text.Contains('$script:desktopRawPin=$payloads.desktop.pin')) 'Retained GUI must be rehashed against the installed raw desktop pin'
    Check (-not ($ast.FindAll({param($node) $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -in @('Stop-Process','taskkill','Remove-Item','Set-Item')},$true)).Count) 'Unexpected PID/name cleanup or deletion command'
    Check ($text.Contains('$request.Proxy=$null') -and $text.Contains('$request.AllowAutoRedirect=$false')) 'Loopback client must disable proxies and redirects'
    Check ($text.Contains('psi.EnvironmentVariables.Clear()') -and $text.Contains('ProcessWindowStyle.Hidden') -and $text.Contains('CreateNoWindow=true')) 'Child environment/window contract is absent'
    Check ($text.Contains('TerminateProcess(handle,1)') -and $text.Contains('DuplicateHandle(GetCurrentProcess(),p.Handle') -and $text.Contains('WaitForSingleObject(handle')) 'Exact retained process-handle contract is absent'
    Check ($text.Contains("'/S /D='") -and $text.Contains("'/S _?='") -and $text.Contains('installer_execution_performed -eq $false')) 'Original NSIS binding/mode guard is absent'
    $native=$ast.FindAll({param($node) $node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value.Contains('namespace CoworkSmoke')},$true)
    Check ($native.Count -eq 1) 'Expected one bounded native identity helper'
    # Compile only; none of the native methods or processes are executed.
    Add-Type -TypeDefinition $native[0].Value
    Check ($null -ne ('CoworkSmoke.Lease' -as [type])) 'Native helper did not compile'
    # This host guard runs before reading the deliberately nonexistent input.
    $guardRejected=$false
    try {& $source -Binding (Join-Path $directory 'absent-binding.json') -OriginalDir $directory -WorkRoot 'C:\cw-never-installed' -Output (Join-Path $directory 'absent-output')}
    catch {$guardRejected=$_.Exception.Message -like '*manually dispatched reviewed-main GitHub-hosted*'}
    Check $guardRejected 'Local host did not fail before package IO/execution'
    Check (-not (Test-Path -LiteralPath 'C:\cw-never-installed') -and -not (Test-Path -LiteralPath (Join-Path $directory 'absent-output'))) 'Negative guard produced an installation/evidence root'
    Write-Output ('PASS: '+$passed+' synthetic/static checks; installer_execution_performed=false; native_process_execution_performed=false')
} finally {
    # Only the exact two files/directories created by this test are removed.
    if(Test-Path -LiteralPath $path){[IO.File]::Delete($path)}
    [IO.Directory]::Delete($directory,$false)
}
