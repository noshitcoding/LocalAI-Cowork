param([switch]$SkipBuild, [switch]$NoOpen, [switch]$PrepareOnly)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$private = Join-Path $repo '.internal\enterprise-local'
$compose = Join-Path $repo 'deploy\enterprise\docker-compose.local.yml'
$controlImages = Join-Path $private 'control-plane-images.json'
$startCompose = @('compose', '--project-name', 'cowork-enterprise-local', '-f', $compose)
$provisionerImage = 'cowork-provisioner:local-review'
$managementImage = 'cowork-server:local-review'
if (Test-Path -LiteralPath $controlImages) {
    $pins = Get-Content -LiteralPath $controlImages -Raw | ConvertFrom-Json
    $services = @('management', 'management-web', 'operator-control', 'provisioner')
    if (@($pins.PSObject.Properties.Name).Count -ne 1 -or $null -eq $pins.services -or
        (Compare-Object @($pins.services.PSObject.Properties.Name | Sort-Object) @($services | Sort-Object))) {
        throw 'Invalid installed control-plane image manifest'
    }
    foreach ($service in $services) {
        $entry = $pins.services.$service
        $repository = if ($service -eq 'management') { 'cowork-server' } elseif ($service -eq 'provisioner') { 'cowork-provisioner' } else { 'cowork-web' }
        if (@($entry.PSObject.Properties.Name).Count -ne 1 -or $entry.image -notmatch ('^' + $repository + '@sha256:[0-9a-f]{64}$')) {
            throw "Invalid installed image for $service"
        }
    }
    $startCompose += @('-f', $controlImages)
    $provisionerImage = $pins.services.provisioner.image
    $managementImage = $pins.services.management.image
}
# Persist only the paired, reviewed release-selection switches. No arbitrary env-file keys.
$controlFeatures = Join-Path $private 'control-plane-features.env'
if (Test-Path -LiteralPath $controlFeatures) {
    $entries = @(Get-Content -LiteralPath $controlFeatures | Where-Object { $_.Trim().Length -gt 0 })
    $flags = @{}
    foreach ($entry in $entries) {
        if ($entry -notmatch '^(COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED|COWORK_RELEASE_SELECTION_ENABLED)=(true|false)$' -or $flags.ContainsKey($Matches[1])) {
            throw 'Invalid installed control-plane feature configuration'
        }
        $flags[$Matches[1]] = $Matches[2]
    }
    if ($flags.Count -ne 2 -or $flags['COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED'] -ne $flags['COWORK_RELEASE_SELECTION_ENABLED']) {
        throw 'Invalid installed control-plane feature configuration'
    }
    # Compose prefers the caller's environment over --env-file. Reject drift
    # before startup so the installed management/operator contract stays paired.
    foreach ($featureName in $flags.Keys) {
        $callerValue = [Environment]::GetEnvironmentVariable($featureName, 'Process')
        if (![string]::IsNullOrEmpty($callerValue) -and $callerValue -cne $flags[$featureName]) {
            throw "Invalid installed control-plane feature override: $featureName"
        }
    }
    $startCompose += @('--env-file', $controlFeatures)
}
New-Item -ItemType Directory -Force -Path $private | Out-Null
# Private local test credentials are never put into tracked config or console output.
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
& icacls $private /inheritance:r /grant:r "${identity}:(OI)(CI)F" 'SYSTEM:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Cannot protect the local secrets directory' }
function Get-LocalSecret([string]$name) {
    $path = Join-Path $private $name
    if (!(Test-Path -LiteralPath $path)) {
        $bytes = New-Object byte[] 32
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
        [IO.File]::WriteAllText($path, ([BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant()))
    }
    return [IO.File]::ReadAllText($path).Trim()
}
foreach ($kind in @('management', 'workspace')) {
    $dbPassword = Get-LocalSecret "${kind}_db_password"
    $databaseUrlPath = Join-Path $private "${kind}_database_url"
    if (!(Test-Path -LiteralPath $databaseUrlPath)) {
        [IO.File]::WriteAllText($databaseUrlPath, "postgres://cowork_${kind}:${dbPassword}@${kind}-db:5432/cowork_${kind}")
    }
    $null = Get-LocalSecret "${kind}_bootstrap"
    $null = Get-LocalSecret "${kind}_admin_password"
}
$null = Get-LocalSecret 'audit_key'
$null = Get-LocalSecret 'auth_master_key'
$null = Get-LocalSecret 'provisioner_token'
$null = Get-LocalSecret 'fleet_s3_user'
$null = Get-LocalSecret 'fleet_s3_password'
$fleetPassword = Get-LocalSecret 'fleet_db_password'
$fleetUrlPath = Join-Path $private 'fleet_database_url'
if (!(Test-Path -LiteralPath $fleetUrlPath)) {
    [IO.File]::WriteAllText($fleetUrlPath, "postgres://cowork_fleet:${fleetPassword}@fleet-db:5432/cowork_fleet")
}
if ($PrepareOnly) { Write-Host 'Local secrets prepared'; exit 0 }
& docker info --format '{{.ServerVersion}}'
if ($LASTEXITCODE -ne 0) { throw 'Start Docker Desktop, then rerun this script' }
if (!$SkipBuild) {
    # Preserve the previous release before replacing development tags. A digest
    # alone does not retain an image in Docker Desktop's containerd image store.
    foreach ($imageName in @('cowork-server', 'cowork-runner', 'cowork-sandbox-core', 'cowork-sandbox-gui', 'cowork-sandbox-crew', 'cowork-egress', 'cowork-provisioner', 'cowork-web')) {
        $existingImage = & docker image ls --quiet "${imageName}:local-review"
        if ($LASTEXITCODE -ne 0) { throw 'Image inventory failed' }
        if ($existingImage) {
            $references = (& docker image inspect "${imageName}:local-review" --format '{{json .RepoDigests}}') | ConvertFrom-Json
            if ($LASTEXITCODE -ne 0) { throw 'Image digest inspection failed' }
            foreach ($reference in $references) {
                if ($reference -match '^(.+)@sha256:([0-9a-f]{64})$') {
                    & docker image tag $reference "$($Matches[1]):retained-sha256-$($Matches[2])"
                    if ($LASTEXITCODE -ne 0) { throw 'Previous release retention failed' }
                }
            }
        }
    }
    foreach ($image in @(
        @{Name='cowork-runner'; File='server/cowork-runner/Dockerfile'},
        @{Name='cowork-sandbox-core'; File='deploy/sandboxes/core/Dockerfile'},
        @{Name='cowork-sandbox-gui'; File='deploy/sandboxes/gui/Dockerfile'},
        @{Name='cowork-sandbox-crew'; File='deploy/sandboxes/crew/Dockerfile'},
        @{Name='cowork-egress'; File='deploy/enterprise/egress.Dockerfile'}
    )) {
        & docker build -f (Join-Path $repo $image.File) -t "$($image.Name):local-review" $repo
        if ($LASTEXITCODE -ne 0) { throw "Build failed: $($image.Name)" }
    }
    & docker compose --project-name cowork-enterprise-local -f $compose build
    if ($LASTEXITCODE -ne 0) { throw 'Docker build failed' }
}
$provisionerNetworkContract = & docker run --rm --network none --entrypoint python3 $provisionerImage /opt/cowork-provisioner/local_entrypoint.py --network-contract
if ($LASTEXITCODE -ne 0) { throw 'The provisioner image is too old for this Compose network profile. Build the updated image before starting services.' }
$provisionerNetworkContract = $provisionerNetworkContract | ConvertFrom-Json
if ($provisionerNetworkContract.version -ne 1 -or $provisionerNetworkContract.independent_namespace -ne $true -or $provisionerNetworkContract.management_url -ne 'https://operator-control:8443') { throw 'The provisioner image does not support the independent management network profile.' }
& (Join-Path $PSScriptRoot 'Initialize-EnterpriseControlTls.ps1') -Directory $private -Image $managementImage
$backupIdentity = Join-Path $private 'fleet_backup_identity'
if (!(Test-Path -LiteralPath $backupIdentity)) {
    $keygen = [Diagnostics.Process]::new()
    $keygen.StartInfo = [Diagnostics.ProcessStartInfo]::new('docker', "run --rm --network none --entrypoint age-keygen $provisionerImage")
    $keygen.StartInfo.UseShellExecute = $false
    $keygen.StartInfo.CreateNoWindow = $true
    $keygen.StartInfo.RedirectStandardOutput = $true
    $keygen.StartInfo.RedirectStandardError = $true
    try {
        $null = $keygen.Start()
        $generatedIdentity = $keygen.StandardOutput.ReadToEnd()
        $keygenLog = $keygen.StandardError.ReadToEnd()
        $keygen.WaitForExit()
        [IO.File]::WriteAllText((Join-Path $private 'backup-keygen.log'), $keygenLog)
        if ($keygen.ExitCode -ne 0 -or !($generatedIdentity -match 'AGE-SECRET-KEY-')) { throw 'Backup identity generation failed' }
        [IO.File]::WriteAllText($backupIdentity, $generatedIdentity, [Text.UTF8Encoding]::new($false))
    } finally { $keygen.Dispose() }
}
& docker @startCompose up -d --wait --wait-timeout 180
if ($LASTEXITCODE -ne 0) { throw 'Docker services did not become healthy' }
$access = @('# Lokale Cowork-Testinstallation', '', 'Nur lokale Testkonten. Keine Produktivdaten.', '')
foreach ($entry in @(@{Kind='management'; Port=18082; Route='management'; Hostname='localhost'}, @{Kind='workspace'; Port=18083; Route='server'; Hostname='127.0.0.1'})) {
    $kind = $entry.Kind
    $origin = "http://$($entry.Hostname):$($entry.Port)"
    $password = Get-LocalSecret "${kind}_admin_password"
    $email = 'admin@cowork.local'
    $body = @{email=$email; display_name='Lokaler Testadministrator'; password=$password; device_id=[guid]::NewGuid().ToString()}
    $session = $null
    try {
        $session = Invoke-RestMethod -Method Post -Uri "$origin/api/v1/auth/bootstrap" -ContentType 'application/json' -Headers @{Authorization="Bearer $(Get-LocalSecret "${kind}_bootstrap")"} -Body ($body | ConvertTo-Json)
    } catch {
        if ([int]$_.Exception.Response.StatusCode -notin @(401, 409)) { throw }
        $body.Remove('display_name')
        if ($kind -eq 'management' -and $env:COWORK_TEST_ADMIN_SECOND_FACTOR) { $body.second_factor = $env:COWORK_TEST_ADMIN_SECOND_FACTOR }
        try { $session = Invoke-RestMethod -Method Post -Uri "$origin/api/v1/auth/login" -ContentType 'application/json' -Body ($body | ConvertTo-Json) }
        catch { if ($kind -ne 'management' -or [int]$_.Exception.Response.StatusCode -ne 401) { throw }; Write-Host 'Die Verwaltung ist bereit. Bitte im Browser mit zweitem Faktor anmelden.' }
    }
    $headers = @{Authorization="Bearer $($session.access_token)"}
    $canManage = $false
    if ($kind -eq 'management' -and $session) {
        $assurance = Invoke-RestMethod -Uri "$origin/api/v1/auth/assurance" -Headers $headers
        $canManage = !$assurance.required -or $assurance.session_verified
        if (!$canManage) { Write-Host 'Bitte zuerst den Authenticator in der Verwaltungsoberfläche einrichten.' }
    }
    if ($canManage) {
        $existing = Invoke-RestMethod -Uri "$origin/management/v1/organisations" -Headers $headers
        foreach ($name in @('Testorganisation Alpha', 'Testorganisation Beta')) {
            if ($existing.items.name -notcontains $name) {
                $null = Invoke-RestMethod -Method Post -Uri "$origin/management/v1/organisations" -Headers $headers -ContentType 'application/json' -Body (@{name=$name} | ConvertTo-Json)
            }
        }
    }
    if ($session) { $null = Invoke-RestMethod -Method Post -Uri "$origin/api/v1/auth/logout" -Headers $headers }
    $access += @("## $kind", '', "Adresse: $origin/$($entry.Route)", "Serveradresse in der Desktop-App: $origin", "E-Mail: $email", "Passwort: $password", '')
    if ($kind -eq 'management') { $access += @('Plattformadministrator: Bei der ersten Anmeldung den Authenticator einrichten und Wiederherstellungscodes sichern. Nach einem Update bei entsprechender Anzeige zuerst ab- und erneut anmelden. Spätere Anmeldungen benötigen zusätzlich einen aktuellen Authenticator- oder unbenutzten Wiederherstellungscode.', '') }
}
$access += @('Die Verwaltung erstellt separate Compose-Instanzen mit API, Worker, Sandbox-Runner, eigener Datenbank und verschlüsseltem Dateispeicher. Mit Instanz öffnen erfolgt die Anmeldung ohne weiteres Passwort.', 'Docker Desktop verwendet das ausdrücklich gewählte seccomp-Profil. Linux-Produktion verlangt weiterhin AppArmor. Ein Modellprofil muss innerhalb der Instanz eingerichtet werden.', 'Der zusätzliche Arbeitsserver auf Port 18083 dient nur dem Verbindungstest. VMs, Kubernetes-Ausfälle und die 40-Aufgaben-Abnahme sind hier nicht getestet.', '', 'Instanzen zuerst in der Verwaltung stoppen, danach: docker compose -f deploy/enterprise/docker-compose.local.yml stop', 'Erneut starten: powershell -File scripts/Start-EnterpriseLocal.ps1 -SkipBuild')
$accessPath = Join-Path $private 'TESTZUGANG.md'
[IO.File]::WriteAllLines($accessPath, $access, [Text.UTF8Encoding]::new($false))
Write-Host "Bereit: http://localhost:18082/management und http://127.0.0.1:18083/server"
Write-Host "Private Testzugänge: $accessPath"
if (!$NoOpen) { Start-Process 'http://localhost:18082/management' }
