[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$signingScript = Join-Path $PSScriptRoot 'sign-windows-installer.ps1'
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "open-cowork-authenticode-smoke-$([guid]::NewGuid().ToString('N'))"
$temporaryInstaller = Join-Path $temporaryRoot 'Open-Cowork-Setup-x64.exe'
$temporaryPfx = Join-Path $temporaryRoot 'ephemeral-code-signing.pfx'
$passwordText = "OpenCowork-Smoke-$([guid]::NewGuid().ToString('N'))"
$password = ConvertTo-SecureString -String $passwordText -AsPlainText -Force
$subject = "CN=Open Cowork Ephemeral Authenticode Smoke $([guid]::NewGuid().ToString('N'))"
$thumbprint = $null
$previousEnvironment = @{}

function Remove-SmokeCertificate {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return }
    if (Test-Path -LiteralPath "Cert:\CurrentUser\My\$Value") {
        & (Join-Path $env:SystemRoot 'System32\certutil.exe') -user -delstore My $Value | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Ephemeral smoke certificate cleanup failed with exit code $LASTEXITCODE."
        }
    }
}

try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    Copy-Item -LiteralPath $resolvedInstaller -Destination $temporaryInstaller

    $certificate = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
        -CertStoreLocation 'Cert:\CurrentUser\My' -HashAlgorithm SHA256 -NotAfter (Get-Date).AddDays(1)
    $thumbprint = $certificate.Thumbprint
    Export-PfxCertificate -Cert $certificate -FilePath $temporaryPfx -Password $password | Out-Null
    $certificate.Dispose()
    Remove-SmokeCertificate $thumbprint
    Start-Sleep -Milliseconds 500

    foreach ($name in @(
        'OPEN_COWORK_CODESIGN_PFX_BASE64',
        'OPEN_COWORK_CODESIGN_PASSWORD',
        'OPEN_COWORK_CODESIGN_THUMBPRINT',
        'OPEN_COWORK_AUTHENTICODE_TEST_MODE'
    )) {
        $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    $env:OPEN_COWORK_CODESIGN_PFX_BASE64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($temporaryPfx))
    $env:OPEN_COWORK_CODESIGN_PASSWORD = $passwordText
    $env:OPEN_COWORK_CODESIGN_THUMBPRINT = $thumbprint
    $env:OPEN_COWORK_AUTHENTICODE_TEST_MODE = '1'

    $signResult = & $signingScript -InstallerPath $temporaryInstaller `
        -TestAllowUntrustedCertificate -TestSkipTimestamp -SignToolTimeoutSeconds 30 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "Authenticode smoke signing exited with $LASTEXITCODE." }
    $verifyResult = & $signingScript -InstallerPath $temporaryInstaller -VerifyOnly `
        -TestAllowUntrustedCertificate -TestSkipTimestamp -SignToolTimeoutSeconds 30 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "Authenticode smoke verification exited with $LASTEXITCODE." }

    if ($signResult.signerThumbprint -ne $thumbprint -or $verifyResult.signerThumbprint -ne $thumbprint) {
        throw 'Authenticode smoke signer thumbprint mismatch.'
    }
    if ($verifyResult.status -in @('NotSigned', 'HashMismatch')) {
        throw "Authenticode smoke verification returned $($verifyResult.status)."
    }

    [pscustomobject]@{
        status = $verifyResult.status
        signerThumbprint = $verifyResult.signerThumbprint
        sourceBytes = (Get-Item -LiteralPath $resolvedInstaller).Length
        signedBytes = (Get-Item -LiteralPath $temporaryInstaller).Length
        trustedReleaseCertificate = $false
        timestamped = $false
    } | ConvertTo-Json -Compress
}
finally {
    foreach ($name in $previousEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
    Remove-SmokeCertificate $thumbprint
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
    Start-Sleep -Milliseconds 500
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
