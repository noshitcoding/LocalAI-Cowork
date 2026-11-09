$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path; $secretRoot=Join-Path $workspace 'deploy/secrets'
$databaseName="cowork_snapshot20_$([guid]::NewGuid().ToString('N'))"; $testRoot=Join-Path ([IO.Path]::GetTempPath()) $databaseName
$serverProcess=$null; New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
function Wait-Http([string]$url,[int]$seconds){$deadline=(Get-Date).AddSeconds($seconds);do{Start-Sleep -Milliseconds 200;try{$r=Invoke-RestMethod $url}catch{$r=$null}}while(-not $r-and(Get-Date)-lt$deadline);if(-not$r){throw "$url did not become ready"}}
function Invoke-Json([string]$method,[string]$path,$body,[string]$token=''){ $h=@{};if($token){$h.authorization="Bearer $token"};$p=@{Method=$method;Uri="http://127.0.0.1:18095/api/v1$path";Headers=$h};if($null-ne$body){$p.ContentType='application/json';$p.Body=$body|ConvertTo-Json -Compress -Depth 20};Invoke-RestMethod @p }
try {
  docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "CREATE DATABASE $databaseName"|Out-Host
  $pg=[IO.File]::ReadAllText((Join-Path $secretRoot 'postgres_password.txt')).Trim();$env:DATABASE_URL="postgres://cowork:$pg@127.0.0.1:15432/$databaseName"
  $env:COWORK_BOOTSTRAP_TOKEN=[IO.File]::ReadAllText((Join-Path $secretRoot 'bootstrap_token.txt')).Trim();$env:COWORK_MODE='api';$env:COWORK_LISTEN_ADDR='127.0.0.1:18095';$env:COWORK_WEB_PUSH_ENABLED='false'
  $env:COWORK_S3_ENDPOINT='http://127.0.0.1:19000';$env:COWORK_S3_REGION='us-east-1';$env:COWORK_S3_BUCKET='cowork-blobs'
  $env:COWORK_S3_ACCESS_KEY=[IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_user.txt')).Trim();$env:COWORK_S3_SECRET_KEY=[IO.File]::ReadAllText((Join-Path $secretRoot 'minio_root_password.txt')).Trim();$env:COWORK_STORAGE_MASTER_KEY=[IO.File]::ReadAllText((Join-Path $secretRoot 'storage_master_key.txt')).Trim()
  Remove-Item Env:COWORK_RUNNER_URL,Env:COWORK_RUNNER_SIGNING_KEY,Env:COWORK_PUBLIC_ORIGIN,Env:COWORK_WEBAUTHN_RP_ID,Env:COWORK_OIDC_ISSUER -ErrorAction SilentlyContinue
  cargo build -p cowork-server|Out-Host;$serverProcess=Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -WorkingDirectory $workspace -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $testRoot stdout.log) -RedirectStandardError (Join-Path $testRoot stderr.log);Wait-Http 'http://127.0.0.1:18095/readyz' 30
  $admin=Invoke-Json POST '/auth/bootstrap' @{email='snapshot20@opencowork.invalid';display_name='Snapshot 20GB';password='Snapshot-20GB-Password-42!';device_id=[guid]::NewGuid()} $env:COWORK_BOOTSTRAP_TOKEN
  $project=Invoke-Json POST '/projects' @{name='20 GiB snapshot';description='';privacy='private_local';team_id=$null;preferred_executor_target=$null;policy=@{}} $admin.access_token
  $chunkSize=16MB;$chunk=[byte[]]::new($chunkSize);for($i=0;$i-lt$chunk.Length;$i++){ $chunk[$i]=[byte](65+($i%23)) }
  $sha=[Security.Cryptography.SHA256]::Create();try{$digest=([BitConverter]::ToString($sha.ComputeHash($chunk))).Replace('-','').ToLowerInvariant()}finally{$sha.Dispose()}
  $chunks=[Collections.Generic.List[object]]::new();for($i=0;$i-lt1280;$i++){$chunks.Add(@{digest=$digest;plaintext_size=$chunkSize})}
  $file=@{path='datasets/logical-20-gib.bin';size=20GB;mode=420;modified_at=(Get-Date).ToUniversalTime().ToString('o');chunks=$chunks}
  $upload=Invoke-Json POST '/snapshots' @{project_id=$project.id;total_bytes=20GB;files=@($file);expires_at=(Get-Date).ToUniversalTime().AddDays(1).ToString('o')} $admin.access_token
  if(@($upload.missing_chunks).Count-ne1){throw 'initial resumable session did not request exactly one unique chunk'}
  $status=Invoke-Json GET "/snapshots/$($upload.manifest_id)/upload" $null $admin.access_token;if(@($status.missing_chunks).Count-ne1){throw 'upload cursor did not preserve the missing chunk'}
  $receipt=Invoke-RestMethod -Method PUT -Uri "http://127.0.0.1:18095/api/v1/snapshots/$($upload.manifest_id)/chunks/$digest" -Headers @{authorization="Bearer $($admin.access_token)"} -ContentType 'application/octet-stream' -Body $chunk
  if($receipt.deduplicated){throw 'first chunk upload was unexpectedly deduplicated'}
  $resumed=Invoke-Json GET "/snapshots/$($upload.manifest_id)/upload" $null $admin.access_token;if(@($resumed.missing_chunks).Count-ne0){throw 'resumed upload did not observe the completed chunk'}
  $manifest=Invoke-Json POST "/snapshots/$($upload.manifest_id)/commit" @{} $admin.access_token;if($manifest.total_bytes-ne20GB-or@($manifest.files[0].chunks).Count-ne1280){throw '20 GiB manifest was not committed exactly'}
  $second=Invoke-Json POST '/snapshots' @{project_id=$project.id;total_bytes=20GB;files=@($file);expires_at=(Get-Date).ToUniversalTime().AddDays(1).ToString('o')} $admin.access_token
  if(@($second.missing_chunks).Count-ne0){throw 'scope-local content deduplication did not reuse the encrypted chunk'}
  $manifest2=Invoke-Json POST "/snapshots/$($second.manifest_id)/commit" @{} $admin.access_token
  $dbState=docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc "SELECT count(*)||':'||min(ref_count)||':'||(min(ciphertext_size)>min(plaintext_size)) FROM snapshot_chunks;"
  if((($dbState-join'').Trim())-ne'1:2560:true'){throw "unexpected encrypted dedupe state: $dbState"}
  $download=Invoke-WebRequest -UseBasicParsing -Method GET -Uri "http://127.0.0.1:18095/api/v1/snapshots/$($manifest.id)/chunks/$digest" -Headers @{authorization="Bearer $($admin.access_token)"}
  $bytes=if($download.Content-is[byte[]]){$download.Content}else{[Text.Encoding]::Latin1.GetBytes([string]$download.Content)};$verify=[Security.Cryptography.SHA256]::Create();try{$downloadDigest=([BitConverter]::ToString($verify.ComputeHash($bytes))).Replace('-','').ToLowerInvariant()}finally{$verify.Dispose()};if($downloadDigest-ne$digest){throw 'encrypted chunk did not decrypt to its original digest'}
  Invoke-Json DELETE "/snapshots/$($manifest.id)" $null $admin.access_token|Out-Null;Invoke-Json DELETE "/snapshots/$($manifest2.id)" $null $admin.access_token|Out-Null
  Stop-Process $serverProcess.Id -Force;$serverProcess.WaitForExit();$serverProcess=$null;$env:COWORK_MODE='worker';$env:COWORK_WORKER_POLL_MS='10';$worker=Start-Process (Join-Path $workspace 'target/debug/cowork-server.exe') -WorkingDirectory $workspace -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $testRoot worker.stdout.log) -RedirectStandardError (Join-Path $testRoot worker.stderr.log);$serverProcess=$worker
  $deadline=(Get-Date).AddSeconds(15);do{Start-Sleep -Milliseconds 100;$left=docker exec open-cowork-postgres-1 psql -U cowork -d $databaseName -tAc 'SELECT count(*) FROM snapshot_chunks;'}while([int](($left-join'').Trim())-ne0-and(Get-Date)-lt$deadline);if([int](($left-join'').Trim())-ne0){throw 'garbage collection did not remove the unreferenced encrypted object'}
  Write-Output 'snapshot_20_gib_logical_limit=ok';Write-Output 'snapshot_resume=ok';Write-Output 'snapshot_scope_deduplication=ok';Write-Output 'snapshot_envelope_encryption_roundtrip=ok';Write-Output 'snapshot_garbage_collection=ok'
}catch{if(Test-Path (Join-Path $testRoot stderr.log)){Get-Content (Join-Path $testRoot stderr.log)};throw}finally{if($serverProcess-and-not$serverProcess.HasExited){Stop-Process $serverProcess.Id -Force -ErrorAction SilentlyContinue;$serverProcess.WaitForExit()};docker exec open-cowork-postgres-1 psql -v ON_ERROR_STOP=1 -U cowork -d postgres -c "DROP DATABASE IF EXISTS $databaseName WITH (FORCE)"|Out-Host;if(Test-Path -LiteralPath $testRoot){Remove-Item -LiteralPath $testRoot -Recurse -Force}}
