param(
    [Parameter(Mandatory=$true)][string]$Binding,
    [Parameter(Mandatory=$true)][string]$OriginalDir,
    [Parameter(Mandatory=$true)][string]$WorkRoot,
    [Parameter(Mandatory=$true)][string]$Output
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Require($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Absolute-Ordinary([string]$Path, [bool]$Existing=$true) {
    Require ([IO.Path]::IsPathRooted($Path) -and $Path -notmatch '["\r\n]' -and $Path -notmatch '(^|[\\/])\.\.?([\\/]|$)') 'An absolute path without traversal is required'
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    Require ($full -notmatch '^\\\\' -and $full.Substring(2) -notmatch ':') 'UNC and alternate streams are refused'
    $current = $full
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -Force -LiteralPath $current
            Require (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) 'Reparse paths are refused'
        }
        $parent = [IO.Path]::GetDirectoryName($current)
        if ($parent -eq $current) { break }; $current = $parent
    }
    if ($Existing) { Require (Test-Path -LiteralPath $full) 'Required path is absent' }
    return $full
}
function Assert-Pin([string]$Path, $Pin) {
    $full = Absolute-Ordinary $Path
    Require ($Pin.sha256 -cmatch '^[0-9a-f]{64}$' -and ($Pin.size -is [int] -or $Pin.size -is [long]) -and $Pin.size -gt 0 -and $Pin.size -le 4294967296) 'Invalid expected file pin'
    $item = Get-Item -Force -LiteralPath $full
    Require (-not $item.PSIsContainer -and $item.Length -eq $Pin.size) 'File size differs from the reviewed payload'
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $full).Hash.ToLowerInvariant()
    Require ($hash -ceq $Pin.sha256) 'File hash differs from the reviewed payload'
    return @{size=$item.Length;sha256=$hash}
}
function New-Pin([string]$Path) {
    $full=Absolute-Ordinary $Path; $item=Get-Item -Force -LiteralPath $full
    Require (-not $item.PSIsContainer) 'Expected a regular file'
    return @{size=$item.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $full).Hash.ToLowerInvariant()}
}
function Assert-DesktopPayload([string]$Path, $Pin) {
    Require ($Pin.marker_policy -ceq 'tauri-cli-2.11.4-single-bundle-marker/1' -and $Pin.allowed_marker -ceq 'NSS') 'Unexpected desktop marker policy'
    Require ($Pin.sha256 -cmatch '^[0-9a-f]{64}$' -and ($Pin.size -is [int] -or $Pin.size -is [long]) -and $Pin.size -gt 0 -and $Pin.size -le 536870912) 'Invalid bounded Cargo desktop pin'
    $full=Absolute-Ordinary $Path;$raw=New-Pin $full
    Require ($raw.size -eq $Pin.size) 'Installed desktop size differs from the Cargo artifact'
    if ($raw.sha256 -ceq $Pin.sha256) {
        $null=Assert-Pin $full $raw
        return @{pin=$raw;comparison='exact_cargo';marker_policy=$Pin.marker_policy;normalized_marker_offset=$null;candidates_checked=0}
    }
    # Tauri CLI 2.11.4 changes only UNK to NSS when building NSIS, then restores
    # the bundle input. Other constants can contain this marker: test each
    # candidate separately, never replace all occurrences or modify the file.
    $bytes=[IO.File]::ReadAllBytes($full)
    $marker='__TAURI_BUNDLE_TYPE_VAR_NSS';$offset=$marker.Length-3
    $text=[Text.Encoding]::ASCII.GetString($bytes);$start=0;$candidates=0;$matches=0;$matchingOffset=$null
    $sha=[Security.Cryptography.SHA256]::Create()
    try {
        $readHash=([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-','').ToLowerInvariant()
        Require ($bytes.Length -eq $raw.size -and $readHash -ceq $raw.sha256) 'Installed desktop changed between raw pinning and normalization'
        while (($position=$text.IndexOf($marker,$start,[StringComparison]::Ordinal)) -ge 0) {
            $candidates++;Require ($candidates -le 64) 'Desktop marker candidate count exceeds its bound'
            $bytes[$position+$offset]=[byte][char]'U';$bytes[$position+$offset+1]=[byte][char]'N';$bytes[$position+$offset+2]=[byte][char]'K'
            try {$normalized=([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-','').ToLowerInvariant()}
            finally {$bytes[$position+$offset]=[byte][char]'N';$bytes[$position+$offset+1]=[byte][char]'S';$bytes[$position+$offset+2]=[byte][char]'S'}
            if ($normalized -ceq $Pin.sha256) {$matches++;$matchingOffset=$position}
            $start=$position+1
        }
    } finally {$sha.Dispose()}
    Require ($matches -eq 1) 'Installed desktop is neither exact Cargo bytes nor one exact reviewed NSS marker change'
    $null=Assert-Pin $full $raw
    # Keep the raw installed pin for every subsequent retained GUI image check.
    return @{pin=$raw;comparison='single_nss_to_unk';marker_policy=$Pin.marker_policy;normalized_marker_offset=$matchingOffset;candidates_checked=$candidates}
}
function Package-Path([string]$Root, [string]$Name) {
    Require ($Name -cmatch '^[A-Za-z0-9_. -]+\.exe$' -and $Name -notmatch '^\.' -and $Name -eq $Name.TrimEnd(' ','.')) 'A package basename is required'
    return Join-Path $Root $Name
}
function Api([string]$Method, [string]$Path, $Body=$null, [bool]$Native=$false) {
    Require ($Path.StartsWith('/') -and -not $Path.Contains('://')) 'Only this loopback API is permitted'
    $request=[Net.HttpWebRequest]::Create('http://127.0.0.1:43821/api/v2'+$Path)
    $request.Proxy=$null; $request.AllowAutoRedirect=$false; $request.Timeout=5000; $request.ReadWriteTimeout=5000
    $request.Method=$Method; $request.Headers['Authorization']='Bearer '+$script:daemonToken
    if ($Native) { $request.Headers['x-cowork-native']=$script:nativeToken }
    if ($null -ne $Body) {
        $raw=[Text.Encoding]::UTF8.GetBytes(($Body|ConvertTo-Json -Depth 20 -Compress))
        $request.ContentType='application/json';$request.ContentLength=$raw.Length
        $stream=$request.GetRequestStream();try {$stream.Write($raw,0,$raw.Length)} finally {$stream.Dispose()}
    }
    $response=$request.GetResponse()
    try {
        Require ([int]$response.StatusCode -ge 200 -and [int]$response.StatusCode -lt 300) 'Loopback API returned an unsuccessful response'
        $stream=$response.GetResponseStream();$memory=New-Object IO.MemoryStream
        try {
            $buffer=New-Object byte[] 8192
            while (($read=$stream.Read($buffer,0,$buffer.Length)) -gt 0) {
                Require ($memory.Length+$read -le 2097152) 'Loopback response exceeds its bound';$memory.Write($buffer,0,$read)
            }
            return [Text.Encoding]::UTF8.GetString($memory.ToArray())|ConvertFrom-Json
        } finally {$stream.Dispose();$memory.Dispose()}
    } finally {$response.Dispose()}
}
function Browser-Snapshot {
    $records=@()
    foreach ($key in @('Software\Google\Chrome\NativeMessagingHosts\dev.localai.cowork.browser','Software\Mozilla\NativeMessagingHosts\dev.localai.cowork.browser')) {
        $opened=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($key)
        try {
            $manifest=if($opened){[string]$opened.GetValue('')}else{$null}
            $record=@{key=$key;exists=($null -ne $opened);manifest=$manifest;manifest_pin=$null;host_path=$null;host_pin=$null}
            if ($manifest) {
                $record.manifest_pin=New-Pin $manifest
                Require ((Get-Item -LiteralPath $manifest).Length -le 65536) 'Browser manifest exceeds its bound'
                $document=Get-Content -LiteralPath $manifest -Raw|ConvertFrom-Json
                Require ($document.name -ceq 'dev.localai.cowork.browser') 'Unexpected native-host manifest'
                $record.host_path=[string]$document.path;$record.host_pin=New-Pin $record.host_path
            }
            $records+=,$record
        } finally {if($opened){$opened.Dispose()}}
    }
    return ,$records
}
function App-Registration {
    $base=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Uninstall')
    $found=@()
    try {
        if ($base) {foreach($name in $base.GetSubKeyNames()) {
            $key=$base.OpenSubKey($name)
            try {if ([string]$key.GetValue('DisplayName') -ceq 'LocalAI Cowork V2 Preview' -or $name -ceq 'dev.localai.cowork.v2.preview') {
                $found+=@{key=$name;display_name=[string]$key.GetValue('DisplayName');version=[string]$key.GetValue('DisplayVersion');location=[string]$key.GetValue('InstallLocation');uninstall=[string]$key.GetValue('UninstallString')}
            }} finally {$key.Dispose()}
        }}
    } finally {if($base){$base.Dispose()}}
    return ,$found
}
function Assert-Health($Health, [string]$Version) {
    Require ($Health.product -ceq 'LocalAI Cowork V2' -and $Health.version -ceq $Version -and $Health.schema_version -eq 8 -and $Health.deployment -ceq 'single_owner_preview') 'Installed health contract differs'
    foreach($name in @('personal_money_status','orders','durable_runs','exact_approvals','text_artifacts')) {Require ($Health.capabilities -ccontains $name) 'Installed capability contract differs'}
}
function Assert-LiveRunBeforeGuiExit($Detail, [string]$ExpectedId) {
    Require ($Detail.run.id -ceq $ExpectedId -and $Detail.run.state -cin @('queued','running')) 'The exact committed run must still be queued or running before GUI termination'
    return [string]$Detail.run.state
}
function Process-Evidence($Lease, [string]$Role) {return @{role=$Role;pid=$Lease.Id;image=$Lease.Image;created_utc=$Lease.CreatedUtc.ToString('o');exited=$Lease.Exited;exit_code=$(if($Lease.Exited){$Lease.ExitCode}else{$null})}}
function Start-Owned([string]$File,[string]$Arguments,[string]$Label) {
    $lease=[CoworkSmoke.Lease]::Start($File,$Arguments,$script:workRoot,$script:childEnvironment,(Join-Path $script:out ($Label+'.stdout.log')),(Join-Path $script:out ($Label+'.stderr.log')))
    $script:leases.Add($lease)
    $script:evidence.commands+=@{file=$File;arguments=$Arguments;started_utc=$lease.CreatedUtc.ToString('o');pid=$lease.Id;label=$Label}
    return $lease
}
function Wait-Command($Lease,[int]$Seconds,[string]$Label) {
    Require ($Lease.Wait($Seconds*1000)) ($Label+' did not physically exit within its bound')
    $script:evidence.processes+=Process-Evidence $Lease $Label
    Require ($Lease.ExitCode -eq 0) ($Label+' returned a nonzero exit code')
}
function Open-Installed([string]$Profile,[string]$Label) {
    Require (-not (Get-NetTCPConnection -LocalPort 43821 -State Listen -ErrorAction SilentlyContinue)) 'Port 43821 is already owned; existing processes are never stopped'
    $desktop=Start-Owned $script:desktopExe ('--cowork-data-dir "'+$Profile+'"') $Label
    $null=Assert-Pin $desktop.Image $script:desktopRawPin
    $until=[DateTime]::UtcNow.AddSeconds(40);$ready=$false
    while([DateTime]::UtcNow -lt $until) {
        Require (-not $desktop.Exited) 'The installed GUI exited before connecting its daemon'
        try {
            $tokenPath=Join-Path $Profile 'daemon.token';$nativePath=Join-Path $Profile 'native.token'
            foreach($path in @($tokenPath,$nativePath)){ $null=Absolute-Ordinary $path;Require ((Get-Item -LiteralPath $path).Length -le 128) 'Authority token exceeds its bound' }
            $script:daemonToken=[IO.File]::ReadAllText($tokenPath).Trim();$script:nativeToken=[IO.File]::ReadAllText($nativePath).Trim()
            Assert-Health (Api GET '/health') $script:bindingRecord.version;$ready=$true;break
        } catch {Start-Sleep -Milliseconds 200}
    }
    Require $ready 'The installed embedded frontend did not connect its matching daemon'
    $listeners=@(Get-NetTCPConnection -LocalPort 43821 -State Listen)
    $ids=@($listeners|Select-Object -ExpandProperty OwningProcess -Unique);Require ($ids.Count -eq 1) 'Ambiguous daemon listener'
    $daemon=[CoworkSmoke.Lease]::Open([int]$ids[0],$script:daemonExe)
    $script:leases.Add($daemon)
    Require ($daemon.CreatedUtc -ge $desktop.CreatedUtc) 'Daemon predates the fresh installed GUI launch'
    $null=Assert-Pin $daemon.Image $script:bindingRecord.expected_payloads.daemon
    $lifecycle=Api GET '/native/lifecycle' $null $true
    Require ($lifecycle.pid -eq $daemon.Id -and $lifecycle.version -ceq $script:bindingRecord.version -and $lifecycle.schema_version -eq 8 -and $lifecycle.instance_id -and -not $lifecycle.closing) 'Native authority does not bind this retained daemon'
    $script:evidence.processes+=Process-Evidence $desktop ($Label+'-gui')
    $script:evidence.processes+=Process-Evidence $daemon ($Label+'-daemon')
    return @{desktop=$desktop;daemon=$daemon;lifecycle=$lifecycle;profile=$Profile}
}
function Stop-Daemon($Session,[string]$Label) {
    $value=$Session.lifecycle
    $null=Api POST '/native/lifecycle/shutdown' @{version=$value.version;schema_version=$value.schema_version;pid=$value.pid;instance_id=$value.instance_id} $true
    Require ($Session.daemon.Wait(15000)) 'Exact retained daemon did not physically exit after native shutdown'
    $script:evidence.processes+=Process-Evidence $Session.daemon $Label
    Require ($Session.daemon.ExitCode -eq 0) 'Exact daemon returned a nonzero exit code after native shutdown'
    Require (-not (Get-NetTCPConnection -LocalPort 43821 -State Listen -ErrorAction SilentlyContinue)) 'Listener remains after the exact daemon exited'
}

# Never execute this helper on a developer host or an unreviewed workflow ref.
Require ($env:GITHUB_ACTIONS -ceq 'true' -and $env:CI_RUNNER_ENVIRONMENT -ceq 'github-hosted' -and $env:GITHUB_EVENT_NAME -ceq 'workflow_dispatch' -and $env:RUNNER_OS -ceq 'Windows' -and $env:GITHUB_REF -ceq 'refs/heads/main' -and $env:GITHUB_SHA -cmatch '^[0-9a-f]{40}$') 'A manually dispatched reviewed-main GitHub-hosted Windows job is required'
$bindingPath=Absolute-Ordinary $Binding;$original=Absolute-Ordinary $OriginalDir
Require ((Get-Item -LiteralPath $bindingPath).Length -le 16777216) 'Binding exceeds its bound'
$script:bindingRecord=Get-Content -LiteralPath $bindingPath -Raw|ConvertFrom-Json
$record=$script:bindingRecord
Require ($record.format -ceq 'localai-cowork-unsigned-installer-smoke-input/1' -and $record.platform -ceq 'windows' -and $record.host -ceq 'x86_64-pc-windows-msvc' -and $record.version -ceq '2.0.0-alpha.8' -and $record.source_commit -cmatch '^[0-9a-f]{40}$' -and $record.original_receipts_unchanged -eq $true -and $record.installer_execution_performed -eq $false -and $record.signed -eq $false -and $record.published -eq $false -and $record.release_qualified -eq $false) 'An unchanged verified original unsigned Windows binding is required'
$installer=Package-Path $original $record.packages.nsis.filename
$null=Assert-Pin $installer $record.packages.nsis
$null=Assert-Pin (Join-Path $original 'receipt.json') $record.package_receipt
$null=Assert-Pin (Join-Path $original 'UNSIGNED-BUILD-REVIEW.json') $record.review_receipt
$work=Absolute-Ordinary $WorkRoot $false;$script:out=Absolute-Ordinary $Output $false
Require (-not (Test-Path -LiteralPath $work) -and -not (Test-Path -LiteralPath $script:out)) 'Work and evidence roots must both be fresh'
Require ($work.Length -le 64 -and $work -notmatch '\s' -and [IO.Path]::GetDirectoryName($work) -eq [IO.Path]::GetPathRoot($work)) 'Select a short direct drive-root work directory'
foreach($pair in @(@($work,$script:out),@($work,$original),@($script:out,$original))) {
    Require (-not $pair[0].StartsWith($pair[1]+'\',[StringComparison]::OrdinalIgnoreCase) -and -not $pair[1].StartsWith($pair[0]+'\',[StringComparison]::OrdinalIgnoreCase) -and $pair[0] -ine $pair[1]) 'Work, original, and external evidence roots must be disjoint'
}
Require (-not (Get-NetTCPConnection -LocalPort 43821 -State Listen -ErrorAction SilentlyContinue)) 'Port 43821 is occupied'

Add-Type -TypeDefinition @'
using System; using System.IO; using System.Text; using System.Diagnostics; using System.Collections; using System.Runtime.InteropServices; using System.Security.Principal; using System.Security.AccessControl;
namespace CoworkSmoke {
public static class Native {
 [StructLayout(LayoutKind.Sequential)] struct SA { public int length; public IntPtr descriptor; public int inherit; }
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateDirectory(string p,ref SA sa);
 public static void PrivateDirectory(string path) { var sid=WindowsIdentity.GetCurrent().User.Value;var sd=new RawSecurityDescriptor("O:"+sid+"D:P(A;OICI;FA;;;"+sid+")(A;OICI;FA;;;SY)");var bytes=new byte[sd.BinaryLength];sd.GetBinaryForm(bytes,0);var pin=GCHandle.Alloc(bytes,GCHandleType.Pinned);try{var sa=new SA{length=Marshal.SizeOf(typeof(SA)),descriptor=pin.AddrOfPinnedObject(),inherit=0};if(!CreateDirectory(path,ref sa))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());}finally{pin.Free();} }
}
public sealed class Lease:IDisposable {
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint a,bool i,int p);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
 [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h,uint t);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr h,uint c);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr h,out uint c);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr h,uint f,StringBuilder s,ref uint n);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetProcessTimes(IntPtr h,out long c,out long e,out long k,out long u);
 [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool DuplicateHandle(IntPtr s,IntPtr h,IntPtr t,out IntPtr d,uint a,bool i,uint o);
 IntPtr handle; Process process; StreamWriter stdout,stderr; int stdoutBytes,stderrBytes; readonly object gate=new object();
 public int Id {get;private set;} public string Image{get;private set;} public DateTime CreatedUtc{get;private set;}
 Lease(IntPtr h,int id,string expected) {handle=h;Id=id;try{var buffer=new StringBuilder(32768);uint n=32768;long c,e,k,u;if(!QueryFullProcessImageName(h,0,buffer,ref n)||!GetProcessTimes(h,out c,out e,out k,out u))throw new Exception("Exact process identity unavailable");Image=Path.GetFullPath(buffer.ToString());CreatedUtc=DateTime.FromFileTimeUtc(c);if(!String.Equals(Image,Path.GetFullPath(expected),StringComparison.OrdinalIgnoreCase))throw new Exception("Exact process image differs");}catch{CloseHandle(h);handle=IntPtr.Zero;throw;} }
 public static Lease Open(int pid,string expected) {var h=OpenProcess(0x00101001,false,pid);if(h==IntPtr.Zero)throw new Exception("Cannot retain daemon handle");return new Lease(h,pid,expected);}
 public static Lease Start(string file,string args,string cwd,IDictionary env,string outPath,string errPath) {
  var psi=new ProcessStartInfo(file,args){WorkingDirectory=cwd,UseShellExecute=false,CreateNoWindow=true,WindowStyle=ProcessWindowStyle.Hidden,RedirectStandardOutput=true,RedirectStandardError=true};psi.EnvironmentVariables.Clear();foreach(DictionaryEntry pair in env)psi.EnvironmentVariables[(string)pair.Key]=(string)pair.Value;
  var p=new Process{StartInfo=psi};Lease lease=null;StreamWriter output=null,error=null;bool started=false;
  try {
   // Create bounded evidence sinks before any child can execute.
   output=new StreamWriter(new FileStream(outPath,FileMode.CreateNew,FileAccess.Write,FileShare.Read));error=new StreamWriter(new FileStream(errPath,FileMode.CreateNew,FileAccess.Write,FileShare.Read));
   if(!p.Start())throw new Exception("Process did not start");started=true;IntPtr h;if(!DuplicateHandle(GetCurrentProcess(),p.Handle,GetCurrentProcess(),out h,0,false,2))throw new Exception("Cannot retain original child handle");
   lease=new Lease(h,p.Id,file);lease.process=p;lease.stdout=output;lease.stderr=error;
   p.OutputDataReceived+=(s,e)=>lease.Log(e.Data,false);p.ErrorDataReceived+=(s,e)=>lease.Log(e.Data,true);p.BeginOutputReadLine();p.BeginErrorReadLine();return lease;
  } catch {
   // This is solely the original Process.Start handle, never a PID lookup.
   if(started){TerminateProcess(p.Handle,1);if(WaitForSingleObject(p.Handle,15000)!=0)throw new Exception("Failed launch cleanup could not confirm physical child exit");}
   if(lease!=null)lease.Dispose();else{p.Dispose();if(output!=null)output.Dispose();if(error!=null)error.Dispose();}throw;
  }
 }
 void Log(string line,bool error){if(line==null)return;lock(gate){int count=error?stderrBytes:stdoutBytes;if(count>1048574)return;string part=line.Substring(0,Math.Min(line.Length,(1048576-count-2)/4));var writer=error?stderr:stdout;if(writer==null)return;writer.WriteLine(part);writer.Flush();count+=Encoding.UTF8.GetByteCount(part)+2;if(error)stderrBytes=count;else stdoutBytes=count;}}
 public bool Exited{get{uint status=WaitForSingleObject(handle,0);if(status!=0&&status!=258)throw new Exception("Process wait failed");return status==0;}}
 public bool Wait(int milliseconds){uint status=WaitForSingleObject(handle,(uint)milliseconds);if(status!=0&&status!=258)throw new Exception("Process wait failed");return status==0;}
 public long ExitCode{get{uint code;if(!Exited||!GetExitCodeProcess(handle,out code))throw new Exception("Physical process exit unavailable");return code;}}
 public void Stop(){if(!Exited&&!TerminateProcess(handle,1))throw new Exception("Exact retained process termination failed");if(!Wait(15000))throw new Exception("Exact retained process remains alive");}
 public void Dispose(){lock(gate){if(process!=null){process.Dispose();process=null;}if(stdout!=null){stdout.Dispose();stdout=null;}if(stderr!=null){stderr.Dispose();stderr=null;}if(handle!=IntPtr.Zero){CloseHandle(handle);handle=IntPtr.Zero;}}}
}
}
'@
[CoworkSmoke.Native]::PrivateDirectory($script:out)
$script:leases=New-Object 'Collections.Generic.List[CoworkSmoke.Lease]'
$script:workRoot=$work
$script:installRoot=Join-Path $work 'app'
$script:childEnvironment=@{}
foreach($name in @('SystemRoot','WINDIR','PATH','PATHEXT','COMSPEC','LOCALAPPDATA','APPDATA','USERPROFILE','HOMEDRIVE','HOMEPATH','USERNAME','ProgramFiles','ProgramFiles(x86)','CommonProgramFiles','TEMP','TMP')) {
    $value=[Environment]::GetEnvironmentVariable($name,'Process');if($null -ne $value){$script:childEnvironment[$name]=$value}
}
$script:childEnvironment['COWORK_ENABLE_TEST_PROVIDER']='1'
$script:evidence=@{format='localai-cowork-windows-nsis-smoke/1';result='failed';started_utc=[DateTime]::UtcNow.ToString('o');harness_commit=$env:GITHUB_SHA;input_binding=New-Pin $bindingPath;selection=$record;commands=@();processes=@();cleanup=@();checks=@{};signed=$false;published=$false;release_qualified=$false;full_installed_product_acceptance='pending';error=$null}
$status=1;$installed=$false;$uninstaller=$null
try {
    [CoworkSmoke.Native]::PrivateDirectory($work)
    $script:evidence.os=@{version=[Environment]::OSVersion.VersionString;image=$env:ImageOS;image_version=$env:ImageVersion}
    $webview=@()
    foreach($hive in @([Microsoft.Win32.Registry]::LocalMachine,[Microsoft.Win32.Registry]::CurrentUser)) {foreach($base in @('SOFTWARE\Microsoft\EdgeUpdate\Clients','SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients')) {
        $key=$hive.OpenSubKey($base);try{if($key){foreach($name in $key.GetSubKeyNames()){$client=$key.OpenSubKey($name);try{if([string]$client.GetValue('name') -match 'WebView2'){$webview+=@{name=[string]$client.GetValue('name');version=[string]$client.GetValue('pv')}}}finally{$client.Dispose()}}}}finally{if($key){$key.Dispose()}}
    }}
    Require ($webview.Count -gt 0) 'Preinstalled WebView2 is absent; bootstrap is a separate acceptance case';$script:evidence.webview2=$webview
    $script:evidence.registrations_before=Browser-Snapshot;$script:evidence.uninstall_before=App-Registration
    Require (@($script:evidence.registrations_before|Where-Object {$_.exists}).Count -eq 0 -and $script:evidence.uninstall_before.Count -eq 0) 'Existing app-specific registration refuses this disposable install'
    foreach($relative in @('profile-a','profile-b','v1-location','temp')){[CoworkSmoke.Native]::PrivateDirectory((Join-Path $work $relative))}
    $script:childEnvironment['TEMP']=Join-Path $work 'temp';$script:childEnvironment['TMP']=Join-Path $work 'temp'
    $sentinel=Join-Path $work 'v1-location\sentinel.txt';[IO.File]::WriteAllText($sentinel,'synthetic V1 location; must remain unchanged')
    $sentinelBefore=New-Pin $sentinel;$profileA=Join-Path $work 'profile-a';$profileB=Join-Path $work 'profile-b'
    # NSIS /D is deliberately last. The installer is the original rehashed package.
    $null=Assert-Pin $installer $record.packages.nsis
    Wait-Command (Start-Owned $installer ('/S /D='+$script:installRoot) 'install') 180 'install';$installed=$true
    $payloads=@{}
    foreach($role in @('desktop','daemon','browser_host')) {
        if ($role -ceq 'desktop') {
            $matches=@()
            foreach($candidate in @(Get-ChildItem -LiteralPath $script:installRoot -File -Recurse|Where-Object {$_.Length -eq $record.expected_payloads.desktop.size})) {
                try {$verified=Assert-DesktopPayload $candidate.FullName $record.expected_payloads.desktop;$verified.path=Absolute-Ordinary $candidate.FullName;$matches+=,$verified} catch {}
            }
            Require ($matches.Count -eq 1) 'Expected exactly one installed desktop matching the reviewed Cargo/NSIS marker policy'
            $payloads.desktop=$matches[0];continue
        }
        $matches=@(Get-ChildItem -LiteralPath $script:installRoot -File -Recurse|Where-Object {$_.Length -eq $record.expected_payloads.$role.size -and (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant() -ceq $record.expected_payloads.$role.sha256})
        Require ($matches.Count -eq 1) ('Expected exactly one installed '+$role+' matching original payload bytes')
        $path=Absolute-Ordinary $matches[0].FullName;$payloads[$role]=@{path=$path;pin=Assert-Pin $path $record.expected_payloads.$role}
    }
    $script:desktopExe=$payloads.desktop.path;$script:daemonExe=$payloads.daemon.path
    $script:desktopRawPin=$payloads.desktop.pin
    $script:evidence.installed_payloads=$payloads
    $uninstallers=@(Get-ChildItem -LiteralPath $script:installRoot -File -Recurse|Where-Object {$_.Name -like '*uninstall*.exe'})
    Require ($uninstallers.Count -eq 1) 'Expected one generated installed uninstaller'
    $uninstaller=Absolute-Ordinary $uninstallers[0].FullName;$uninstallerPin=New-Pin $uninstaller
    $script:evidence.uninstaller=@{installed=$uninstaller;pin=$uninstallerPin;mode='copied original /S _?=, no TEMP restart'}
    $script:evidence.uninstall_installed=App-Registration;Require ($script:evidence.uninstall_installed.Count -eq 1) 'Current-user app uninstall registration is absent or ambiguous'
    Require ($script:evidence.uninstall_installed[0].uninstall.IndexOf($uninstaller,[StringComparison]::OrdinalIgnoreCase) -ge 0) 'Uninstall registration does not name this installed uninstaller'
    $session=Open-Installed $profileA 'first'
    $script:evidence.registrations_installed=Browser-Snapshot
    foreach($registration in $script:evidence.registrations_installed){Require $registration.exists 'Native-host registration is missing';$null=Assert-Pin $registration.host_path $record.expected_payloads.browser_host}
    $project=Api POST '/projects' @{name='NSIS own synthetic durable state'}
    $order=Api POST '/orders' @{project_id=$project.id;title='Installed GUI lifetime'}
    $provider=Api POST '/providers' @{name='Own deterministic provider';kind='test';endpoint='';model='slow-test';context_tokens=32768}
    $request=@{order_id=$order.id;base_revision=1;idempotency_key=[Guid]::NewGuid().ToString();prompt='Create exactly one deterministic artifact.';provider_id=$provider.id;provider_revision=1;access_mode='autonomous'}
    $prepared=Api POST '/execution-plans' $request
    Require (-not $prepared.retired -and $prepared.plan.order_id -eq $order.id -and $prepared.plan.idempotency_key -ceq $request.idempotency_key -and $prepared.sha256 -cmatch '^[0-9a-f]{64}$') 'Prepared plan differs from the original synthetic request'
    $commitPath='/orders/'+[Uri]::EscapeDataString($order.id)+'/execution-plans/'+[Uri]::EscapeDataString($prepared.plan.id)+'/commit'
    $run=Api POST $commitPath @{sha256=$prepared.sha256}
    $beforeExit=Api GET ('/runs/'+$run.id)
    $beforeExitState=Assert-LiveRunBeforeGuiExit $beforeExit $run.id
    Require (-not $session.desktop.Exited) 'The exact GUI already exited before the live-run observation'
    $script:evidence.run_before_gui_exit=@{run_id=$run.id;state=$beforeExitState;observed_utc=[DateTime]::UtcNow.ToString('o');exact_gui_still_running=$true}
    $session.desktop.Stop();$script:evidence.processes+=Process-Evidence $session.desktop 'first-gui-terminated'
    $until=[DateTime]::UtcNow.AddSeconds(40);$completed=$false
    while([DateTime]::UtcNow -lt $until){$detail=Api GET ('/runs/'+$run.id);Require ($detail.run.state -notin @('failed','interrupted','canceled','expired')) 'The synthetic run failed after GUI exit';if($detail.run.state -ceq 'completed'){$completed=$true;break};Start-Sleep -Milliseconds 200}
    Require ($completed -and @($detail.artifacts).Count -eq 1 -and -not $session.daemon.Exited) 'Exactly one durable artifact must complete after the exact GUI exits'
    $artifactId=$detail.artifacts[0].id;$script:evidence.synthetic=@{project_id=$project.id;order_id=$order.id;run_id=$run.id;artifact_id=$artifactId;plan_sha256=$prepared.sha256}
    Stop-Daemon $session 'first-daemon-native-shutdown'
    Require (Test-Path -LiteralPath (Join-Path $profileA 'cowork-v2.sqlite3')) 'Selected synthetic profile has no persisted database'
    $script:evidence.profile_database_before_reopen=New-Pin (Join-Path $profileA 'cowork-v2.sqlite3')
    $reopened=Open-Installed $profileA 'reopen-same';$retained=Api GET ('/runs/'+$run.id)
    Require ($retained.run.state -ceq 'completed' -and @($retained.artifacts).Count -eq 1 -and $retained.artifacts[0].id -eq $artifactId) 'Same-profile reopen lost or duplicated its durable synthetic artifact'
    $reopened.desktop.Stop();Stop-Daemon $reopened 'reopened-daemon-native-shutdown'
    $fresh=Open-Installed $profileB 'reopen-fresh';$projects=@(Api GET '/projects')
    Require ($projects.Count -eq 0) 'Fresh profile exposed first-profile state'
    $fresh.desktop.Stop();Stop-Daemon $fresh 'fresh-daemon-native-shutdown'
    $script:evidence.checks=@{installed_frontend_started_matching_daemon=$true;run_survived_exact_gui_exit=$true;one_durable_artifact=$true;same_profile_reopen=$true;fresh_profile_isolated=$true}
    foreach($lease in $script:leases){Require $lease.Exited 'An owned process remains before uninstall'}
    $copy=Join-Path $script:out 'original-uninstaller.exe';[IO.File]::Copy($uninstaller,$copy,$false);$null=Assert-Pin $copy $uninstallerPin
    # _?= is deliberately last, so NSIS executes this retained original process.
    Wait-Command (Start-Owned $copy ('/S _?='+$script:installRoot) 'uninstall') 180 'uninstall'
    foreach($payload in $payloads.Values){Require (-not (Test-Path -LiteralPath $payload.path)) 'Installed application payload remains after uninstall'}
    Require (-not (Test-Path -LiteralPath $uninstaller)) 'Installed uninstaller remains'
    $script:evidence.registrations_after=Browser-Snapshot;$script:evidence.uninstall_after=App-Registration
    Require ($script:evidence.uninstall_after.Count -eq 0 -and @($script:evidence.registrations_after|Where-Object {$_.exists}).Count -eq 0) 'App/native-host registration remains after uninstall'
    foreach($registration in $script:evidence.registrations_installed){Require (-not (Test-Path -LiteralPath $registration.manifest)) 'Native-host manifest remains';$null=Assert-Pin $registration.host_path $record.expected_payloads.browser_host}
    $script:evidence.retained_browser_host_cache=@($script:evidence.registrations_installed|ForEach-Object {@{path=$_.host_path;pin=New-Pin $_.host_path}})
    $sentinelAfter=New-Pin $sentinel;Require ($sentinelAfter.sha256 -ceq $sentinelBefore.sha256) 'Own synthetic V1-location sentinel changed'
    $script:evidence.sentinel=@{before=$sentinelBefore;after=$sentinelAfter};$script:evidence.profiles_retained=@($profileA,$profileB)
    $script:evidence.checks.uninstall_payloads_and_registrations_removed=$true;$script:evidence.result='passed';$status=0
} catch {
    # Do not include response bodies, token bytes, or arbitrary OS diagnostics.
    $script:evidence.error=@{stage='windows-smoke';message=$_.Exception.Message.Substring(0,[Math]::Min(512,$_.Exception.Message.Length))}
} finally {
    foreach($lease in $script:leases){try{if(-not $lease.Exited){$lease.Stop();$script:evidence.cleanup+=@{pid=$lease.Id;forced_exact_handle_stop=$true;exited=$lease.Exited};$status=1;$script:evidence.result='failed'}}catch{$script:evidence.cleanup+=@{pid=$lease.Id;exited=$false};$status=1;$script:evidence.result='failed'}}
    foreach($lease in $script:leases){$lease.Dispose()}
    $script:daemonToken=$null;$script:nativeToken=$null
    $script:evidence.finished_utc=[DateTime]::UtcNow.ToString('o');$script:evidence.exit_code=$status
    $script:evidence.install_command_succeeded=$installed;$script:evidence.removal_scope='Exact installed executables, current-user uninstall key and both native-host registrations/manifests; synthetic profiles, external evidence and observed immutable browser-host cache are retained'
    $script:evidence.logs=@(Get-ChildItem -LiteralPath $script:out -Filter '*.log' -File|ForEach-Object {@{filename=$_.Name;pin=New-Pin $_.FullName}})
    [IO.File]::WriteAllText((Join-Path $script:out 'smoke.json'),($script:evidence|ConvertTo-Json -Depth 35),[Text.UTF8Encoding]::new($false))
}
exit $status
