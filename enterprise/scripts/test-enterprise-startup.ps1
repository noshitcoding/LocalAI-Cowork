$ErrorActionPreference = 'Stop'
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $temporaryRoot ('cowork-startup-test-' + [guid]::NewGuid().ToString('N'))
$shellPath = (Get-Process -Id $PID).Path
$sourceScript = Join-Path $PSScriptRoot 'Start-EnterpriseLocal.ps1'
$checks = @()
try {
    foreach ($case in @('preserve', 'extra-service', 'privileged', 'foreign-image', 'extra-root', 'preserve-features', 'unknown-feature', 'unpaired-feature', 'duplicate-feature', 'matching-feature-environment', 'conflicting-feature-environment')) {
        $caseRoot = Join-Path $testRoot $case
        $private = Join-Path $caseRoot '.internal/enterprise-local'
        $scriptDirectory = Join-Path $caseRoot 'scripts'
        $null = New-Item -ItemType Directory -Force -Path $private, $scriptDirectory
        Copy-Item -LiteralPath $sourceScript -Destination (Join-Path $scriptDirectory 'Start-EnterpriseLocal.ps1')
        $digest = 'a' * 64
        $pins = @{services=@{
            management=@{image="cowork-server@sha256:$digest"}
            provisioner=@{image="cowork-provisioner@sha256:$digest"}
            'management-web'=@{image="cowork-web@sha256:$digest"}
            'operator-control'=@{image="cowork-web@sha256:$digest"}
        }}
        switch ($case) {
            'extra-service' { $pins.services.other=@{image="cowork-server@sha256:$digest"} }
            'privileged' { $pins.services.provisioner.privileged=$true }
            'foreign-image' { $pins.services.management.image="unqualified-server@sha256:$digest" }
            'extra-root' { $pins.volumes=@{unexpected=@{}} }
        }
        $featureContent = $null
        switch ($case) {
            'preserve-features' { $featureContent = "COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED=true`nCOWORK_RELEASE_SELECTION_ENABLED=true`n" }
            'matching-feature-environment' { $featureContent = "COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED=true`nCOWORK_RELEASE_SELECTION_ENABLED=true`n" }
            'conflicting-feature-environment' { $featureContent = "COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED=true`nCOWORK_RELEASE_SELECTION_ENABLED=true`n" }
            'unknown-feature' { $featureContent = "COMPOSE_FILE=unexpected.yml`n" }
            'unpaired-feature' { $featureContent = "COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED=true`nCOWORK_RELEASE_SELECTION_ENABLED=false`n" }
            'duplicate-feature' { $featureContent = "COWORK_RELEASE_SELECTION_ENABLED=true`nCOWORK_RELEASE_SELECTION_ENABLED=true`n" }
        }
        $featuresPath = Join-Path $private 'control-plane-features.env'
        if ($null -ne $featureContent) { [IO.File]::WriteAllText($featuresPath, $featureContent) }
        $pinsPath = Join-Path $private 'control-plane-images.json'
        [IO.File]::WriteAllText($pinsPath, ($pins | ConvertTo-Json -Depth 8))
        $urlPath = Join-Path $private 'management_database_url'
        $selectedUrl = 'postgres://cowork_management:test-only@management-db:5432/cowork_management_release_123456789abc'
        [IO.File]::WriteAllText($urlPath, $selectedUrl)
        $expectedPinsHash = (Get-FileHash -LiteralPath $pinsPath).Hash
        $callerFeatures = @{}
        foreach ($featureName in @('COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED', 'COWORK_RELEASE_SELECTION_ENABLED')) {
            $callerFeatures[$featureName] = [Environment]::GetEnvironmentVariable($featureName, 'Process')
            [Environment]::SetEnvironmentVariable($featureName, $null, 'Process')
        }
        try {
            if ($case -eq 'matching-feature-environment') {
                [Environment]::SetEnvironmentVariable('COWORK_RELEASE_SELECTION_ENABLED', 'true', 'Process')
            } elseif ($case -eq 'conflicting-feature-environment') {
                [Environment]::SetEnvironmentVariable('COWORK_RELEASE_SELECTION_ENABLED', 'false', 'Process')
            }
            $output = & $shellPath -NoProfile -File (Join-Path $scriptDirectory 'Start-EnterpriseLocal.ps1') -PrepareOnly 2>&1
            $exitCode = $LASTEXITCODE
        } finally {
            foreach ($featureName in $callerFeatures.Keys) {
                [Environment]::SetEnvironmentVariable($featureName, $callerFeatures[$featureName], 'Process')
            }
        }
        if ($case -in @('preserve', 'preserve-features', 'matching-feature-environment')) {
            if ($exitCode -ne 0) { throw 'Valid pinned startup preparation failed' }
            if ([IO.File]::ReadAllText($urlPath) -ne $selectedUrl) { throw 'Startup overwrote the selected database' }
            if ((Get-FileHash -LiteralPath $pinsPath).Hash -ne $expectedPinsHash) { throw 'Startup changed immutable image selection' }
            if ($null -ne $featureContent -and [IO.File]::ReadAllText($featuresPath) -ne $featureContent) { throw 'Startup overwrote installed features' }
            if (!(Test-Path -LiteralPath (Join-Path $private 'workspace_database_url'))) { throw 'Fresh connection defaults were not created' }
        } else {
            if ($exitCode -eq 0) { throw "Unsafe image manifest accepted: $case" }
            if (($output | Out-String) -notmatch 'Invalid installed') { throw "Unexpected rejection reason: $case" }
            if (Test-Path -LiteralPath (Join-Path $private 'management_bootstrap')) { throw 'Invalid manifest reached secret initialization' }
        }
        $checks += $case
    }
    @{checks=$checks; live_modified=$false; docker_started=$false} | ConvertTo-Json -Compress
} finally {
    $resolved = [IO.Path]::GetFullPath($testRoot)
    if ([IO.Path]::GetDirectoryName($resolved) -ne $temporaryRoot -or
        [IO.Path]::GetFileName($resolved) -notmatch '^cowork-startup-test-[a-f0-9]{32}$') {
        throw 'Refusing cleanup outside the generated startup fixture'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
