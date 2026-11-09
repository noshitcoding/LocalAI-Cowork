param([Parameter(Mandatory=$true)][string]$Directory, [string]$Image='cowork-server:local-review')
$ErrorActionPreference='Stop'
$certificate=Join-Path $Directory 'control_tls_certificate'
$privateKey=Join-Path $Directory 'control_tls_key'
$hasCertificate=Test-Path -LiteralPath $certificate
$hasPrivateKey=Test-Path -LiteralPath $privateKey
if ($hasCertificate -ne $hasPrivateKey) {throw 'The private management TLS identity is incomplete. Restore the matching certificate and key.'}
if (!(Test-Path -LiteralPath $Directory -PathType Container)) {throw 'The protected destination directory must already exist'}
$temporaryRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/')
$stage=Join-Path $temporaryRoot ('cowork-control-tls-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
function Invoke-ControlOpenSsl([string[]]$OpenSslArguments) {
    $previousPreference=$ErrorActionPreference
    try {
        $ErrorActionPreference='Continue'
        & docker run --rm --network none --mount "type=bind,src=$stage,target=/out" --entrypoint openssl $Image @OpenSslArguments 2>&1 | Out-Null
        $commandExitCode=$LASTEXITCODE
    } finally {$ErrorActionPreference=$previousPreference}
    if ($commandExitCode -ne 0) {throw 'Private management TLS identity validation failed; check its hostname, expiry and matching key before restarting services.'}
}
try {
    if ($hasCertificate) {
        Copy-Item -LiteralPath $certificate -Destination (Join-Path $stage 'control_tls_certificate')
        Copy-Item -LiteralPath $privateKey -Destination (Join-Path $stage 'control_tls_key')
    } else {
        Invoke-ControlOpenSsl @('req','-x509','-newkey','rsa:3072','-nodes','-sha256','-days','397','-subj','/CN=operator-control','-addext','subjectAltName=DNS:operator-control','-addext','extendedKeyUsage=serverAuth','-keyout','/out/control_tls_key','-out','/out/control_tls_certificate')
    }
    Invoke-ControlOpenSsl @('verify','-CAfile','/out/control_tls_certificate','-purpose','sslserver','-verify_hostname','operator-control','/out/control_tls_certificate')
    Invoke-ControlOpenSsl @('x509','-checkend','86400','-noout','-in','/out/control_tls_certificate')
    Invoke-ControlOpenSsl @('x509','-pubkey','-noout','-in','/out/control_tls_certificate','-out','/out/certificate-public.pem')
    Invoke-ControlOpenSsl @('pkey','-pubout','-in','/out/control_tls_key','-out','/out/key-public.pem')
    if ((Get-FileHash -LiteralPath (Join-Path $stage 'certificate-public.pem')).Hash -ne (Get-FileHash -LiteralPath (Join-Path $stage 'key-public.pem')).Hash) {throw 'The private management TLS certificate and key do not match'}
    if ($hasCertificate) {return}
    # The host directory is private. The only container mounting the key is the
    # non-root TLS proxy; Compose bind-backed secrets retain these source modes.
    & docker run --rm --network none --mount "type=bind,src=$stage,target=/out" --entrypoint chmod $Image 0444 /out/control_tls_key /out/control_tls_certificate
    if ($LASTEXITCODE -ne 0) {throw 'Private management TLS secret permissions could not be prepared'}
    Copy-Item -LiteralPath (Join-Path $stage 'control_tls_key') -Destination $privateKey
    Copy-Item -LiteralPath (Join-Path $stage 'control_tls_certificate') -Destination $certificate
} finally {
    $resolved=[IO.Path]::GetFullPath($stage)
    if ([IO.Path]::GetDirectoryName($resolved) -ne $temporaryRoot -or [IO.Path]::GetFileName($resolved) -notmatch '^cowork-control-tls-[a-f0-9]{32}$') {throw 'Refusing cleanup outside the generated TLS staging directory'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
