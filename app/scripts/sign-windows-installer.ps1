[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [string[]]$TimestampUrl = @(
        "http://timestamp.sectigo.com",
        "https://timestamp.digicert.com",
        "https://rfc3161timestamp.globalsign.com/advanced"
    ),

    [ValidateRange(10, 300)]
    [int]$SignToolTimeoutSeconds = 45,

    [switch]$VerifyOnly,

    [switch]$TestAllowUntrustedCertificate,

    [switch]$TestSkipTimestamp
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Normalize-Thumbprint {
    param([Parameter(Mandatory = $true)][string]$Value)
    return ($Value -replace '[^A-Fa-f0-9]', '').ToUpperInvariant()
}

function Resolve-SignTool {
    $command = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (-not (Test-Path -LiteralPath $kitsRoot -PathType Container)) {
        throw "signtool.exe was not found. Install the Windows SDK signing tools."
    }

    $candidate = Get-ChildItem -LiteralPath $kitsRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object { try { [version]$_.Name } catch { [version]'0.0' } } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1

    if (-not $candidate) {
        throw "The x64 Windows SDK signtool.exe was not found under $kitsRoot."
    }
    return $candidate
}

function Test-CodeSigningEku {
    param([Parameter(Mandatory = $true)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate)
    $codeSigningOid = '1.3.6.1.5.5.7.3.3'
    foreach ($extension in $Certificate.Extensions) {
        if ($extension -is [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]) {
            foreach ($oid in $extension.EnhancedKeyUsages) {
                if ($oid.Value -eq $codeSigningOid) {
                    return $true
                }
            }
        }
    }
    return $false
}

function ConvertTo-ProcessArgument {
    param([Parameter(Mandatory = $true)][string]$Value)
    if ($Value -notmatch '[\s"]') {
        return $Value
    }
    return '"' + ($Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Invoke-BoundedProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Path
    $quotedArguments = @($Arguments | ForEach-Object { ConvertTo-ProcessArgument -Value ([string]$_) })
    $startInfo.Arguments = $quotedArguments -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "$([System.IO.Path]::GetFileName($Path)) could not be started."
    }
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try { $process.Kill($true) } catch { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        [void]$process.WaitForExit(5000)
        throw "$([System.IO.Path]::GetFileName($Path)) exceeded the ${TimeoutSeconds}s timeout."
    }
    [pscustomobject]@{
        ExitCode = $process.ExitCode
    }
}

function Assert-TimestampUrl {
    param([Parameter(Mandatory = $true)][string]$Value)
    $uri = $null
    if (-not [Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$uri)) {
        throw "Invalid RFC 3161 timestamp URL."
    }
    $allowedHosts = @('timestamp.sectigo.com', 'timestamp.digicert.com', 'rfc3161timestamp.globalsign.com')
    if ($uri.Scheme -notin @('http', 'https') -or $uri.Host -notin $allowedHosts -or $uri.UserInfo) {
        throw "RFC 3161 timestamp URL is outside the approved TSA allowlist."
    }
}

function Assert-CodeSigningCertificate {
    param(
        [Parameter(Mandatory = $true)]
        [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedThumbprint
    )

    $actualThumbprint = Normalize-Thumbprint $Certificate.Thumbprint
    if ($actualThumbprint -ne $ExpectedThumbprint) {
        throw "The imported certificate thumbprint does not match OPEN_COWORK_CODESIGN_THUMBPRINT."
    }
    if (-not $Certificate.HasPrivateKey) {
        throw "The selected code-signing certificate has no private key."
    }
    if (-not (Test-CodeSigningEku $Certificate)) {
        throw "The selected certificate is not valid for code signing."
    }
    $now = [DateTime]::UtcNow
    if ($Certificate.NotBefore.ToUniversalTime() -gt $now -or $Certificate.NotAfter.ToUniversalTime() -le $now) {
        throw "The selected code-signing certificate is outside its validity period."
    }
}

function Assert-InstallerSignature {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedThumbprint,
        [Parameter(Mandatory = $true)][string]$SignTool,
        [bool]$AllowUntrustedCertificate = $false,
        [bool]$RequireTimestamp = $true
    )

    $verification = Invoke-BoundedProcess -Path $SignTool -Arguments @('verify', '/pa', '/all', '/v', $Path) -TimeoutSeconds $SignToolTimeoutSeconds
    if ($verification.ExitCode -ne 0 -and -not $AllowUntrustedCertificate) {
        throw "Authenticode verification failed with exit code $($verification.ExitCode)."
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    try {
        if (-not $AllowUntrustedCertificate -and $signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "Authenticode status is $($signature.Status), expected Valid."
        }
        if ($AllowUntrustedCertificate -and $signature.Status -in @(
            [System.Management.Automation.SignatureStatus]::NotSigned,
            [System.Management.Automation.SignatureStatus]::HashMismatch
        )) {
            throw "The test signature is missing or has a hash mismatch."
        }
        if (-not $signature.SignerCertificate) {
            throw "The installer has no Authenticode signer certificate."
        }
        if ((Normalize-Thumbprint $signature.SignerCertificate.Thumbprint) -ne $ExpectedThumbprint) {
            throw "The installer signer does not match OPEN_COWORK_CODESIGN_THUMBPRINT."
        }
        if ($RequireTimestamp -and -not $signature.TimeStamperCertificate) {
            throw "The installer signature has no trusted RFC 3161 timestamp."
        }

        [pscustomobject]@{
            path = [System.IO.Path]::GetFileName($Path)
            status = $signature.Status.ToString()
            signerSubject = $signature.SignerCertificate.Subject
            signerThumbprint = Normalize-Thumbprint $signature.SignerCertificate.Thumbprint
            signerValidUntil = $signature.SignerCertificate.NotAfter.ToUniversalTime().ToString('o')
            timestampSubject = if ($signature.TimeStamperCertificate) { $signature.TimeStamperCertificate.Subject } else { $null }
        }
    }
    finally {
        if ($signature.SignerCertificate) { $signature.SignerCertificate.Dispose() }
        if ($signature.TimeStamperCertificate) { $signature.TimeStamperCertificate.Dispose() }
    }
}

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
if ([System.IO.Path]::GetExtension($resolvedInstaller) -ine '.exe') {
    throw "InstallerPath must point to an .exe file."
}
if ((Get-Item -LiteralPath $resolvedInstaller).Length -lt 2) {
    throw "InstallerPath is empty or truncated."
}

$expectedThumbprintValue = [Environment]::GetEnvironmentVariable('OPEN_COWORK_CODESIGN_THUMBPRINT', 'Process')
if ([string]::IsNullOrWhiteSpace($expectedThumbprintValue)) {
    throw "OPEN_COWORK_CODESIGN_THUMBPRINT is required."
}
$expectedThumbprint = Normalize-Thumbprint $expectedThumbprintValue
if ($expectedThumbprint.Length -ne 40 -and $expectedThumbprint.Length -ne 64) {
    throw "OPEN_COWORK_CODESIGN_THUMBPRINT must be a SHA-1 or SHA-256 certificate thumbprint."
}

$testModeRequested = $TestAllowUntrustedCertificate -or $TestSkipTimestamp
if ($testModeRequested -and $env:OPEN_COWORK_AUTHENTICODE_TEST_MODE -ne '1') {
    throw "Authenticode test switches require OPEN_COWORK_AUTHENTICODE_TEST_MODE=1."
}

$signTool = Resolve-SignTool
if ($VerifyOnly) {
    Assert-InstallerSignature -Path $resolvedInstaller -ExpectedThumbprint $expectedThumbprint -SignTool $signTool `
        -AllowUntrustedCertificate $TestAllowUntrustedCertificate -RequireTimestamp (-not $TestSkipTimestamp) |
        ConvertTo-Json -Compress
    exit 0
}

$pfxBase64 = [Environment]::GetEnvironmentVariable('OPEN_COWORK_CODESIGN_PFX_BASE64', 'Process')
$pfxPassword = [Environment]::GetEnvironmentVariable('OPEN_COWORK_CODESIGN_PASSWORD', 'Process')
if ([string]::IsNullOrWhiteSpace($pfxBase64)) {
    throw "OPEN_COWORK_CODESIGN_PFX_BASE64 is required."
}
if ([string]::IsNullOrEmpty($pfxPassword)) {
    throw "OPEN_COWORK_CODESIGN_PASSWORD is required."
}

$temporaryPfx = Join-Path ([System.IO.Path]::GetTempPath()) "open-cowork-codesign-$([guid]::NewGuid().ToString('N')).pfx"
$existingThumbprints = @(
    Get-ChildItem -Path 'Cert:\CurrentUser\My' |
        ForEach-Object { Normalize-Thumbprint $_.Thumbprint }
)
$importedCertificates = @()

try {
    try {
        [System.IO.File]::WriteAllBytes($temporaryPfx, [Convert]::FromBase64String($pfxBase64))
    }
    catch {
        throw "OPEN_COWORK_CODESIGN_PFX_BASE64 is not valid base64-encoded PFX data."
    }

    $securePassword = ConvertTo-SecureString -String $pfxPassword -AsPlainText -Force
    $importedCertificates = @(Import-PfxCertificate -FilePath $temporaryPfx -CertStoreLocation 'Cert:\CurrentUser\My' -Password $securePassword)
    $signingCertificates = @($importedCertificates | Where-Object { $_.HasPrivateKey -and (Test-CodeSigningEku $_) })
    if ($signingCertificates.Count -ne 1) {
        throw "The PFX must contain exactly one code-signing certificate with a private key."
    }
    $certificate = $signingCertificates[0]
    Assert-CodeSigningCertificate -Certificate $certificate -ExpectedThumbprint $expectedThumbprint

    if ($TestSkipTimestamp) {
        $signing = Invoke-BoundedProcess -Path $signTool -Arguments @(
            'sign', '/sha1', $expectedThumbprint, '/s', 'My', '/fd', 'SHA256', '/v', $resolvedInstaller
        ) -TimeoutSeconds $SignToolTimeoutSeconds
        $signed = $signing.ExitCode -eq 0
        $timestampFailures = @("timestamp intentionally omitted in test mode: exit $($signing.ExitCode)")
    }
    else {
        $signed = $false
        $timestampFailures = @()
        foreach ($timestampEndpoint in $TimestampUrl) {
            Assert-TimestampUrl $timestampEndpoint
            try {
                $signing = Invoke-BoundedProcess -Path $signTool -Arguments @(
                    'sign', '/sha1', $expectedThumbprint, '/s', 'My', '/fd', 'SHA256',
                    '/tr', $timestampEndpoint, '/td', 'SHA256', '/v', $resolvedInstaller
                ) -TimeoutSeconds $SignToolTimeoutSeconds
                if ($signing.ExitCode -eq 0) {
                    $signed = $true
                    break
                }
                $timestampFailures += "$(([Uri]$timestampEndpoint).Host): exit $($signing.ExitCode)"
            }
            catch {
                $timestampFailures += "$(([Uri]$timestampEndpoint).Host): $($_.Exception.Message)"
            }
        }
    }
    if (-not $signed) {
        throw "Authenticode signing failed for every approved timestamp service: $($timestampFailures -join '; ')"
    }

    Assert-InstallerSignature -Path $resolvedInstaller -ExpectedThumbprint $expectedThumbprint -SignTool $signTool `
        -AllowUntrustedCertificate $TestAllowUntrustedCertificate -RequireTimestamp (-not $TestSkipTimestamp) |
        ConvertTo-Json -Compress
}
finally {
    if (Test-Path -LiteralPath $temporaryPfx) {
        Remove-Item -LiteralPath $temporaryPfx -Force
    }
    foreach ($certificate in $importedCertificates) {
        $thumbprint = Normalize-Thumbprint $certificate.Thumbprint
        $certificate.Dispose()
        if ($thumbprint -and $existingThumbprints -notcontains $thumbprint) {
            $certUtil = Join-Path $env:SystemRoot 'System32\certutil.exe'
            $cleanup = Invoke-BoundedProcess -Path $certUtil -Arguments @('-user', '-delstore', 'My', $thumbprint) -TimeoutSeconds 10
            if ($cleanup.ExitCode -ne 0) {
                throw "Temporary code-signing certificate cleanup failed with exit code $($cleanup.ExitCode)."
            }
        }
    }
}
