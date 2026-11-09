# One owned interactive STA. Native Object Model comes only from this original child.
$ErrorActionPreference='Stop'; Set-StrictMode -Version Latest
$requestPath=$args[0]; $request=Get-Content -LiteralPath $requestPath -Raw -Encoding UTF8 | ConvertFrom-Json
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Diagnostics;
using System.Runtime.InteropServices;
public sealed class CoworkOwnedOffice : IDisposable {
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct SI { public int cb;public string reserved,desktop,title;public uint x,y,xs,ys,xc,yc,fill,flags;public short show,cb2;public IntPtr r2,input,output,error; }
 [StructLayout(LayoutKind.Sequential)] struct PI {public IntPtr process,thread;public uint pid,tid;}
 [StructLayout(LayoutKind.Sequential)] struct BASIC {public long processTime,jobTime;public uint flags;public UIntPtr min,max;public uint active;public UIntPtr affinity;public uint priority,scheduling;}
 [StructLayout(LayoutKind.Sequential)] struct IO {public ulong a,b,c,d,e,f;}
 [StructLayout(LayoutKind.Sequential)] struct LIMIT {public BASIC basic;public IO io;public UIntPtr processMemory,jobMemory,peakProcess,peakJob;}
 delegate bool EnumProc(IntPtr hwnd,IntPtr param);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string app,StringBuilder command,IntPtr pa,IntPtr ta,bool inherit,uint flags,IntPtr env,string dir,ref SI si,out PI pi);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr sa,string name);
 [DllImport("kernel32",SetLastError=true)] static extern bool SetInformationJobObject(IntPtr h,int kind,ref LIMIT info,uint size);
 [DllImport("kernel32",SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
 [DllImport("kernel32")] static extern uint ResumeThread(IntPtr h);
 [DllImport("kernel32")] static extern uint WaitForSingleObject(IntPtr h,uint ms);
 [DllImport("kernel32")] static extern bool TerminateJobObject(IntPtr h,uint code);
 [DllImport("kernel32")] static extern bool TerminateProcess(IntPtr h,uint code);
 [DllImport("kernel32")] static extern bool CloseHandle(IntPtr h);
 [DllImport("kernel32")] static extern bool QueryInformationJobObject(IntPtr h,int kind,IntPtr data,uint size,IntPtr ret);
 [DllImport("user32",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateDesktop(string name,IntPtr device,IntPtr mode,uint flags,uint access,IntPtr sa);
 [DllImport("user32",SetLastError=true)] static extern bool SetThreadDesktop(IntPtr desktop);
 [DllImport("user32")] static extern bool CloseDesktop(IntPtr desktop);
 [DllImport("user32")] static extern bool EnumDesktopWindows(IntPtr desktop,EnumProc proc,IntPtr p);
 [DllImport("user32")] static extern bool EnumChildWindows(IntPtr parent,EnumProc proc,IntPtr p);
 [DllImport("user32")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
 [DllImport("user32",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window,StringBuilder value,int length);
 [DllImport("oleacc")] static extern int AccessibleObjectFromWindow(IntPtr hwnd,uint id,ref Guid iid,[MarshalAs(UnmanagedType.Interface)] out object value);
 [DllImport("kernel32")] static extern uint GetCurrentProcessId();
 [DllImport("kernel32")] static extern uint GetCurrentThreadId();
 [DllImport("kernel32")] static extern bool ProcessIdToSessionId(uint pid,out uint session);
 [DllImport("user32")] static extern IntPtr GetThreadDesktop(uint tid);
 [DllImport("user32",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32",CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr handle,int index,StringBuilder text,uint size,out uint needed);
 public static bool Interactive(){uint session;if(!ProcessIdToSessionId(GetCurrentProcessId(),out session)||session==0||!Connected(session))return false;IntPtr input=OpenInputDesktop(0,false,1);if(input==IntPtr.Zero)return false;try{StringBuilder name=new StringBuilder(128);uint needed;return GetUserObjectInformation(input,2,name,256,out needed)&&String.Equals(name.ToString(),"Default",StringComparison.OrdinalIgnoreCase);}finally{CloseDesktop(input);}}
 [DllImport("wtsapi32",CharSet=CharSet.Unicode)] static extern bool WTSQuerySessionInformation(IntPtr server,uint session,int kind,out IntPtr buffer,out uint bytes);
 [DllImport("wtsapi32")] static extern void WTSFreeMemory(IntPtr buffer);
 static bool Connected(uint session){IntPtr buffer;uint bytes;if(!WTSQuerySessionInformation(IntPtr.Zero,session,8,out buffer,out bytes))return false;try{return bytes>=4&&Marshal.ReadInt32(buffer)==0;}finally{WTSFreeMemory(buffer);}}
 IntPtr previousDesktop;
 IntPtr desktop,job,process;uint pid;public string DesktopName {get;private set;} public uint OriginalPid {get{return pid;}}
 public CoworkOwnedOffice(string executable,string source,string directory,string nonce) {
  if(!Interactive())throw new Exception("office_interactive_session_unavailable");previousDesktop=GetThreadDesktop(GetCurrentThreadId());
  DesktopName="LocalAI.Office."+nonce;desktop=CreateDesktop(DesktopName,IntPtr.Zero,IntPtr.Zero,0,0x01ff,IntPtr.Zero);
  if(desktop==IntPtr.Zero || !SetThreadDesktop(desktop))throw new Exception("office_owned_desktop_failed");
  job=CreateJobObject(IntPtr.Zero,null);if(job==IntPtr.Zero)throw new Exception("office_owned_job_failed");
  LIMIT limits=new LIMIT();limits.basic.flags=0x2000;
  if(!SetInformationJobObject(job,9,ref limits,(uint)Marshal.SizeOf(limits)))throw new Exception("office_owned_job_policy_failed");
  SI si=new SI();si.cb=Marshal.SizeOf(si);si.desktop=DesktopName;PI pi;
  // Safe mode suppresses personal startup addins/templates; original document policy
  // has already rejected macros, XLM, links, OLE and active fields before opening.
  string command="\""+executable+"\" /safe \""+source+"\"";
  if(!CreateProcess(executable,new StringBuilder(command),IntPtr.Zero,IntPtr.Zero,false,4,IntPtr.Zero,directory,ref si,out pi))throw new Exception("office_original_spawn_failed");
  process=pi.process;pid=pi.pid;
  try {if(!AssignProcessToJobObject(job,process))throw new Exception("office_original_job_assignment_failed");if(ResumeThread(pi.thread)==0xffffffff)throw new Exception("office_original_resume_failed");}
  catch {TerminateProcess(process,1);WaitForSingleObject(process,5000);throw;}
  finally {CloseHandle(pi.thread);}
 }
 public bool IsOriginalWindow(IntPtr window){uint actual;GetWindowThreadProcessId(window,out actual);return process!=IntPtr.Zero && WaitForSingleObject(process,0)==258 && actual==pid;}
 public object Bind(string application) {
  if(!Interactive())throw new Exception("office_interactive_session_changed");
  string expected=application=="word"?"_WwG":application=="excel"?"EXCEL7":"paneClassDC";IntPtr found=IntPtr.Zero;
  EnumProc child=delegate(IntPtr h,IntPtr ignored){StringBuilder cls=new StringBuilder(256);GetClassName(h,cls,cls.Capacity);if(IsOriginalWindow(h)&&cls.ToString()==expected){found=h;return false;}return true;};
  EnumProc top=delegate(IntPtr h,IntPtr ignored){if(IsOriginalWindow(h)){child(h,IntPtr.Zero);EnumChildWindows(h,child,IntPtr.Zero);}return found==IntPtr.Zero;};
  EnumDesktopWindows(desktop,top,IntPtr.Zero);
  if(found==IntPtr.Zero)return null;
  object value;Guid dispatch=new Guid("00020400-0000-0000-C000-000000000046");
  if(!IsOriginalWindow(found)||AccessibleObjectFromWindow(found,0xfffffff0,ref dispatch,out value)!=0||!IsOriginalWindow(found))throw new Exception("office_original_native_binding_failed");
  return value;
 }
 public bool CloseOwned() {
  if(job==IntPtr.Zero)return false;
  TerminateJobObject(job,0);IntPtr data=Marshal.AllocHGlobal(48);
  try {Stopwatch deadline=Stopwatch.StartNew();while(deadline.ElapsedMilliseconds<10000){if(!QueryInformationJobObject(job,1,data,48,IntPtr.Zero))return false;if(Marshal.ReadInt32(data,40)==0)return true;System.Threading.Thread.Sleep(25);}return false;}
  finally {Marshal.FreeHGlobal(data);}
 }
 public void Dispose(){if(job!=IntPtr.Zero){CloseOwned();CloseHandle(job);job=IntPtr.Zero;}if(process!=IntPtr.Zero){WaitForSingleObject(process,5000);CloseHandle(process);process=IntPtr.Zero;}if(desktop!=IntPtr.Zero){if(previousDesktop!=IntPtr.Zero)SetThreadDesktop(previousDesktop);CloseDesktop(desktop);desktop=IntPtr.Zero;}}
}
'@
$state=[System.Management.Automation.Runspaces.InitialSessionState]::CreateDefault()
$state.Variables.Add([System.Management.Automation.Runspaces.SessionStateVariableEntry]::new('original',$request,'Immutable original operation'))
$runspace=[System.Management.Automation.Runspaces.RunspaceFactory]::CreateRunspace($state)
$runspace.ApartmentState='STA';$runspace.ThreadOptions='UseNewThread';$runspace.Open()
$pipeline=[PowerShell]::Create();$pipeline.Runspace=$runspace
[void]$pipeline.AddScript({
 $ErrorActionPreference='Stop';Set-StrictMode -Version Latest
 $owner=$null;$window=$null;$app=$null;$document=$null;$quiescent=$false
 function Parameter([string]$Name,$Default=$null,[bool]$Required=$false){$p=$original.parameters.PSObject.Properties[$Name];if($null-ne$p-and$null-ne$p.Value){return $p.Value};if($Required){throw 'office_parameter_missing'};return $Default}
 function Font($f){foreach($name in @('bold','italic','font_size','font_name','font_color')){$v=Parameter $name;if($null-ne$v){switch($name){bold{$f.Bold=$(if([bool]$v){-1}else{0})};italic{$f.Italic=$(if([bool]$v){-1}else{0})};font_size{$f.Size=[double]$v};font_name{$f.Name=[string]$v};font_color{$f.Color=[int]$v}}}}}
 try {
  $owner=[CoworkOwnedOffice]::new($original.executable,$original.working_source,$original.directory,$original.nonce)
  $deadline=[DateTime]::UtcNow.AddSeconds(60)
  while($null-eq$window-and[DateTime]::UtcNow-lt$deadline){$window=$owner.Bind($original.application);if($null-eq$window){Start-Sleep -Milliseconds 50}}
  if($null-eq$window){throw 'office_original_window_unavailable_or_dialog'}
  $app=$window.Application;$app.AutomationSecurity=3
  if($original.application-eq'word'){$document=$window.Document;$app.DisplayAlerts=0}
  elseif($original.application-eq'excel'){$document=$window.ActiveSheet.Parent;$app.DisplayAlerts=$false;$app.AskToUpdateLinks=$false;$app.EnableEvents=$false}
  else{$document=$window.Presentation}
  if([System.IO.Path]::GetFullPath([string]$document.FullName)-ne[System.IO.Path]::GetFullPath($original.working_source)){throw 'office_original_document_differs'}
  switch($original.application){
   word {
    switch($original.action){
     export_pdf{$document.ExportAsFixedFormat($original.working_output,17)}
     replace_text{$find=$document.Content.Find;$find.ClearFormatting();$find.Replacement.ClearFormatting();[void]$find.Execute([string](Parameter 'old_text' $null $true),$false,$false,$false,$false,$false,$true,1,$false,[string](Parameter 'new_text' $null $true),$(if(Parameter 'replace_all' $true){2}else{1}));$document.SaveAs2($original.working_output,16)}
     word_append_paragraph{$range=$document.Content;$range.Collapse(0);$range.InsertAfter("`r`n"+[string](Parameter 'text' $null $true));Font $range.Font;$document.SaveAs2($original.working_output,16)}
     word_format_text{$range=$document.Content;if(-not$range.Find.Execute([string](Parameter 'target_text' $null $true))){throw 'office_target_not_found'};Font $range.Font;$document.SaveAs2($original.working_output,16)}
     default{throw 'office_action_not_allowed'}
    }
    if($original.working_preview){$document.ExportAsFixedFormat($original.working_preview,17)}
   }
   excel {
    $sheetName=Parameter 'sheet';$sheet=if($null-eq$sheetName){$document.Worksheets.Item(1)}else{$document.Worksheets.Item([string]$sheetName)}
    switch($original.action){
     export_pdf{$document.ExportAsFixedFormat(0,$original.working_output)}
     replace_text{$replaced=$false;foreach($ws in $document.Worksheets){if($ws.Cells.Replace([string](Parameter 'old_text' $null $true),[string](Parameter 'new_text' $null $true),2,1,$false,$false,$false,$false)){$replaced=$true}};if(-not$replaced){throw 'office_target_not_found'};$document.SaveAs($original.working_output,51)}
     excel_set_cell{$cell=$sheet.Range([string](Parameter 'cell' $null $true));$formula=Parameter 'formula';if($null-ne$formula){$cell.Formula=[string]$formula}else{$value=Parameter 'value' $null $true;if($value-is[string]){$cell.NumberFormat='@'};$cell.Value2=$value};$number=Parameter 'number_format';if($null-ne$number){$cell.NumberFormat=[string]$number};$app.CalculateFull();$document.SaveAs($original.working_output,51)}
     excel_format_range{$range=$sheet.Range([string](Parameter 'range' $null $true));Font $range.Font;$number=Parameter 'number_format';if($null-ne$number){$range.NumberFormat=[string]$number};$document.SaveAs($original.working_output,51)}
     excel_add_chart{$range=$sheet.Range([string](Parameter 'range' $null $true));$chart=$sheet.ChartObjects().Add([double](Parameter 'left' 320),[double](Parameter 'top' 20),[double](Parameter 'width' 560),[double](Parameter 'height' 320)).Chart;$chart.SetSourceData($range);$chart.ChartType=[int](Parameter 'chart_type' 51);$title=Parameter 'title';if($null-ne$title){$chart.HasTitle=$true;$chart.ChartTitle.Text=[string]$title};$document.SaveAs($original.working_output,51)}
     default{throw 'office_action_not_allowed'}
    }
    if($original.working_preview){$document.ExportAsFixedFormat(0,$original.working_preview)}
   }
   powerpoint {
    switch($original.action){
     export_pdf{$document.SaveAs($original.working_output,32)}
     replace_text{$count=0;foreach($slide in $document.Slides){foreach($shape in $slide.Shapes){if($shape.HasTextFrame-and$shape.TextFrame.HasText){$value=$shape.TextFrame.TextRange.Text;$old=[string](Parameter 'old_text' $null $true);if($value.Contains($old)){$shape.TextFrame.TextRange.Text=$value.Replace($old,[string](Parameter 'new_text' $null $true));$count++}}}};if($count-eq0){throw 'office_target_not_found'};$document.SaveAs($original.working_output,24)}
     powerpoint_add_slide{$slide=$document.Slides.Add($document.Slides.Count+1,[int](Parameter 'layout' 2));$title=Parameter 'title';$body=Parameter 'body';if($null-ne$title-and$null-ne$slide.Shapes.Title){$slide.Shapes.Title.TextFrame.TextRange.Text=[string]$title};if($null-ne$body-and$slide.Shapes.Placeholders.Count-ge2){$slide.Shapes.Placeholders.Item(2).TextFrame.TextRange.Text=[string]$body};$document.SaveAs($original.working_output,24)}
     powerpoint_format_text{$count=0;foreach($slide in $document.Slides){foreach($shape in $slide.Shapes){if($shape.HasTextFrame-and$shape.TextFrame.HasText-and$shape.TextFrame.TextRange.Text.Contains([string](Parameter 'target_text' $null $true))){Font $shape.TextFrame.TextRange.Font;$count++}}};if($count-eq0){throw 'office_target_not_found'};$document.SaveAs($original.working_output,24)}
     default{throw 'office_action_not_allowed'}
    }
    if($original.working_preview){$document.SaveAs($original.working_preview,32)}
   }
   default{throw 'office_application_not_allowed'}
  }
  if(-not[CoworkOwnedOffice]::Interactive()){throw "office_interactive_session_changed"}
  # Never close a user application through a class lookup. These proxies were
  # obtained only from the held original window on our fresh private desktop.
  if($original.application-eq'word'){$document.Close(0)}elseif($original.application-eq'excel'){$document.Close($false)}else{$document.Close()}
  $app.Quit();$quiescent=$owner.CloseOwned();if(-not$quiescent){throw 'office_original_quiescence_unknown'}
  @{ok=$true;format='device-office-native-receipt/1';nonce=$original.nonce;request_sha256=$original.request_sha256;source_sha256=$original.source_sha256;application=$original.application;action=$original.action;original_process_id=$owner.OriginalPid;private_desktop=$true;owned_job_empty=$quiescent;automatic_replay=$false} | ConvertTo-Json -Compress
 } finally {
  foreach($value in @($document,$app,$window)){if($null-ne$value-and[Runtime.InteropServices.Marshal]::IsComObject($value)){[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($value)}}
  if($null-ne$owner){$owner.Dispose()}
 }
}.ToString())
try {
 $values=$pipeline.Invoke()
 if($pipeline.HadErrors-or$values.Count-ne1){throw 'office_original_operation_uncertain'}
 [Console]::Out.WriteLine([string]$values[0])
} catch {[Console]::Error.WriteLine('office_original_operation_uncertain_no_replay');exit 1}
finally {$pipeline.Dispose();$runspace.Close();$runspace.Dispose()}
