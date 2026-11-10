"""Private tool setup. No global pip, system installer actions, or credentials."""
from __future__ import annotations
import argparse, ctypes, hashlib, io, json, os, pathlib, shutil, subprocess
import sys, tarfile, time, urllib.request, urllib.error, ssl, uuid, zipfile, posixpath, shlex, copy, importlib.util, re

PIP=("https://files.pythonhosted.org/packages/62/36/a3aed958d60531cb442b7ab4596cda7b3621cfb916f8ae1d6769795c7dc1/pip-26.2-py3-none-any.whl","931c303696af6fa3417112103b1cad26890e5a07eccb5b99783700e33f2b8aad")
DOWNLOADS={
 "git_windows":("https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/MinGit-2.56.0.2-64-bit.zip","da35e72aa21c005a5a0d298cfbae110bc1609a815730ea0dde84b01a1b3cd3be"),
 "gh_windows":("https://github.com/cli/cli/releases/download/v2.102.0/gh_2.102.0_windows_amd64.zip","ae64e556ecc240b200f7eba60d550e4bb60d78e860e69dd88c449405b86067f4"),
 "gh_linux":("https://github.com/cli/cli/releases/download/v2.102.0/gh_2.102.0_linux_amd64.tar.gz","bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386"),
 "office_windows":("https://download.documentfoundation.org/libreoffice/stable/26.8.1/win/x86_64/LibreOffice_26.8.1_Win_x86-64.msi","c6298b10cfa1748bcbedad94c62ce771e240e5021bfd6d7a967cd571b04b5312"),
 "office_linux":("https://download.documentfoundation.org/libreoffice/stable/26.8.1/deb/x86_64/LibreOffice_26.8.1_Linux_x86-64_deb.tar.gz","30903df3b9f61360d9660cd707de48cd2831469114492a5008ed58a0ac77d044"),
 "tectonic_windows":("https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.17.0/tectonic-0.17.0-x86_64-pc-windows-msvc.zip","f61ce51f0b0ade1015b7de7ef368541c5424e9756ecbd0d7af97d6d48030845f"),
 "tectonic_linux":("https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.17.0/tectonic-0.17.0-x86_64-unknown-linux-musl.tar.gz","8533d07f9ccbd7a65824b9e0459041bca34af1eb33daba48f59215593753a3b7"),
}
# Profiles: "documents" is the automatic first-start setup (document wheels + Tectonic, no browsers).
# Browser adapters depend on wheels of the documents lock, so "browser" and "full" install both.
PROFILES={"documents":("documents","ocr"),"browser":("documents","ocr","browser"),"full":("documents","ocr","browser")}
# The extracted binary is pinned as well: the daemon binds the warm TeX cache to exactly this file.
TECTONIC={"version":"0.17.0","binary":{"windows":("tectonic.exe","99ffcfdbf1ebf8bdda9e791942e3d06aedb12463fddc33f07de6f5211c8bf08d"),"linux":("tectonic","a98aa59ad5c1df39a6c9e56cbfc5088f2b11d6c179c0130b97998e4bd46a46da")},
 "bundle":"https://relay.fullyjustified.net/default_bundle_v33.tar","bundle_digest":"6ffe055852f8faf66c0acbe1a7fb27f87b869a90bad1204f3bf4d9683f597c7c"}
# Fonts are loaded by file name from the bundle, never by family name from the host.
TEX_FONTS={"noto":r"""\setmainfont{NotoSerif}[Extension=.ttf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]
\setsansfont{NotoSans}[Extension=.ttf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]
\setmonofont{NotoSansMono}[Extension=.ttf,UprightFont=*-Regular,BoldFont=*-Bold]""",
 "gyre":r"""\setmainfont{texgyretermes}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]
\setsansfont{texgyreheros}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]
\setmonofont{texgyrecursor}[Extension=.otf,UprightFont=*-regular,BoldFont=*-bold,ItalicFont=*-italic,BoldItalicFont=*-bolditalic]"""}
# Each document caches the bundle files it needs; compiles afterwards run with --only-cached.
TEX_WARMUP={
 **{"cowork-template-"+str(size):r"\documentclass["+str(size)+r"pt,a4paper]{article}"+r"""
\usepackage{fontspec}
"""+TEX_FONTS["noto"]+r"""
\usepackage[english,ngerman]{babel}
\usepackage[a4paper,margin=2.5cm]{geometry}
\usepackage{graphicx,float,array,booktabs,longtable,enumitem,xcolor,amsmath,amssymb}
\IfFileExists{ulem.sty}{\usepackage[normalem]{ulem}}{}
\providecommand{\uline}[1]{\underline{#1}}
\providecommand{\sout}[1]{#1}
\usepackage[hidelinks]{hyperref}
\title{Cowork template}\author{Runtime qualification}\date{}
\begin{document}
\maketitle\section{Überschrift}
Text \textbf{bold} \textit{italic} \texttt{mono} \uline{underline} \sout{strike}.
\footnote{Footnote $\sum_{i=1}^{n}i^2$.}
Inline $\sum_{i=1}^{n}i^2+\frac{a}{b}+\mathbb{R}$.
\[\sum_{i=1}^{n}i^2+\frac{\sqrt{\pi}}{2}\]
{\small $\sum_i^n i$} {\footnotesize $\sum_i^n i$}
\end{document}
""" for size in (10,11,12)},
 "article":r"""\documentclass[11pt,a4paper]{article}
\usepackage{fontspec}
"""+TEX_FONTS["noto"]+r"""
\usepackage{amsmath,amssymb,mathtools,amsthm}
\usepackage[math-style=ISO]{unicode-math}
\setmathfont{latinmodern-math.otf}
\usepackage[english,ngerman,french,spanish,italian,russian]{babel}
\usepackage[a4paper,margin=2.5cm]{geometry}
\usepackage[table,dvipsnames]{xcolor}
\usepackage{graphicx,booktabs,tabularx,longtable,multirow,array,colortbl,caption,subcaption,float,enumitem,fancyhdr,titlesec,microtype,csquotes,listings,siunitx,url,parskip,lipsum,ragged2e,setspace,footmisc,xurl,makecell,threeparttable,pdflscape,wrapfig,tcolorbox,qrcode}
\usepackage{tikz,pgfplots}
\pgfplotsset{compat=1.18}
\usetikzlibrary{arrows.meta,positioning,shapes,calc,decorations.pathreplacing,patterns,backgrounds,fit,matrix,shadows}
\usepackage[hidelinks]{hyperref}
\usepackage{bookmark}
\pagestyle{fancy}
\begin{document}
\tableofcontents
\section{Übersicht – Привет – Ελληνικά}
\foreignlanguage{russian}{Съешь же ещё этих мягких французских булок.} \enquote{Zitat} \SI{3.5}{\kilo\metre} \num{12345.678}
\textbf{fett} \textit{kursiv} \textsf{sans \textbf{bold}} \texttt{mono \textbf{bold}}
\begin{equation} \int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2} \quad \mathbb{R}\ \mathcal{A}\ \symbf{v} \end{equation}
\begin{tabularx}{\linewidth}{lXr}\toprule A & B & C\\\midrule 1 & 2 & 3\\\bottomrule\end{tabularx}
\begin{longtable}{ll}a&b\\\end{longtable}
\begin{tikzpicture}\draw[-Stealth] (0,0) -- (1,1) node[right]{x}; \end{tikzpicture}
\begin{tikzpicture}\begin{axis}[width=5cm]\addplot {x^2};\end{axis}\end{tikzpicture}
\begin{tcolorbox}Box\end{tcolorbox}
\begin{lstlisting}[language=Python]
print("x")
\end{lstlisting}
\begin{itemize}\item a\end{itemize}
\lipsum[1]
\end{document}
""",
 "gyre":r"""\documentclass{article}
\usepackage{fontspec}
"""+TEX_FONTS["gyre"]+r"""
\usepackage{unicode-math}
\setmathfont{texgyretermes-math.otf}
\begin{document}
Termes \textbf{b} \textit{i} \textbf{\textit{bi}} \textsf{Heros \textbf{b} \textit{i} \textbf{\textit{bi}}} \texttt{Cursor \textbf{b} \textit{i} \textbf{\textit{bi}}} $a^2+b^2=c^2$
\end{document}
""",
 "report":r"""\documentclass{report}
\usepackage[ngerman]{babel}
\usepackage{amsmath,graphicx,hyperref}
\begin{document}
\chapter{Kapitel} Text \textbf{b} \textit{i} \textsf{s \textbf{b}} \texttt{t} $x^2$
\end{document}
""",
 "book":r"""\documentclass{book}
\usepackage{makeidx}\makeindex
\begin{document}\frontmatter\tableofcontents\mainmatter\chapter{A}Text\index{x}\appendix\chapter{B}\backmatter\printindex\end{document}
""",
 "scrartcl":r"""\documentclass[paper=a4,fontsize=11pt,parskip=half]{scrartcl}
\usepackage[ngerman]{babel}
\usepackage{scrlayer-scrpage}
\begin{document}\section{A}Text \textbf{b}\end{document}
""",
 "scrreprt":r"""\documentclass{scrreprt}
\usepackage[ngerman]{babel}
\begin{document}\chapter{A}Text\end{document}
""",
 "scrlttr2":r"""\documentclass[ngerman]{scrlttr2}
\usepackage{babel}
\begin{document}\begin{letter}{Empfänger\\Straße 1\\12345 Ort}\opening{Sehr geehrte Damen und Herren,}Text\closing{Mit freundlichen Grüßen}\end{letter}\end{document}
""",
 "beamer":r"""\documentclass{beamer}
\usetheme{Madrid}
\usepackage[ngerman]{babel}
\usepackage{tikz,booktabs}
\begin{document}
\begin{frame}{Titel}\begin{itemize}\item a \pause \item b\end{itemize}\begin{block}{B}x\end{block}\end{frame}
\end{document}
""",
 "beamer-noto":r"""\documentclass[aspectratio=169]{beamer}
\usetheme{default}\usecolortheme{beaver}
\usepackage{fontspec}
\setsansfont{NotoSans}[Extension=.ttf,UprightFont=*-Regular,BoldFont=*-Bold,ItalicFont=*-Italic,BoldItalicFont=*-BoldItalic]
\begin{document}\begin{frame}{T}\textbf{b} \textit{i}\end{frame}\end{document}
""",
 "cjk":r"""\documentclass{article}
\usepackage{fontspec}
\usepackage{xeCJK}
\setCJKmainfont{FandolSong-Regular.otf}[BoldFont=FandolSong-Bold.otf]
\setCJKsansfont{FandolHei-Regular.otf}[BoldFont=FandolHei-Bold.otf]
\newCJKfontfamily\jafont{HaranoAjiMincho-Regular.otf}[BoldFont=HaranoAjiMincho-Bold.otf]
\newCJKfontfamily\jasans{HaranoAjiGothic-Regular.otf}
\newCJKfontfamily\kofont{UnBatang.ttf}[BoldFont=UnBatangBold.ttf]
\newCJKfontfamily\kosans{UnDotum.ttf}
\begin{document}
中文测试 \textbf{粗体} \textsf{黑体}
{\jafont 日本語のテキスト \textbf{太字}} {\jasans ゴシック}
{\kofont 한국어 텍스트 \textbf{굵게}} {\kosans 돋움}
\end{document}
""",
 "natbib":r"""\documentclass{article}
\usepackage[numbers]{natbib}
\begin{filecontents*}[overwrite]{natbib.bib}
@article{a,author={A. Author},title={T},journal={J},year={2020}}
\end{filecontents*}
\begin{document}\cite{a}\bibliographystyle{plainnat}\bibliography{natbib}\end{document}
""",
 **{"bst-"+style:r"""\documentclass{article}
\begin{filecontents*}[overwrite]{STYLE.bib}
@book{b,author={B. Author},title={T},publisher={P},year={2021}}
\end{filecontents*}
\begin{document}\cite{b}\bibliographystyle{STYLE}\bibliography{STYLE}\end{document}
""".replace("STYLE",style) for style in ("plain","unsrt","alpha","abbrv","ieeetr","apalike")},
}

def ordinary(path,create=False):
 path=pathlib.Path(os.path.abspath(path))
 for part in [*reversed(path.parents),path]:
  if part.exists() and (part.is_symlink() or getattr(part.lstat(),"st_file_attributes",0)&0x400):
   raise RuntimeError("Use an ordinary private directory without aliases/reparse points")
 if create:
  path.mkdir(parents=True,exist_ok=True)
  if os.name!="nt":path.chmod(0o700)
 return path

def atomic(path,value):
 ordinary(path);temporary=path.with_name(path.name+"."+uuid.uuid4().hex+".new")
 with temporary.open("x",encoding="utf-8") as stream:
  json.dump(value,stream,ensure_ascii=False,indent=2);stream.flush();os.fsync(stream.fileno())
 if os.name!="nt":temporary.chmod(0o600)
 os.replace(temporary,path)

def progress(stage,message):
 event={"stage":stage,"message":message,"updated_at":int(time.time()*1000)}
 atomic(DATA/"tool-setup-progress.json",event);print(json.dumps(event,ensure_ascii=False),flush=True)

def download(name,specification,bound=600*1024*1024):
 url,expected=specification;destination=STAGE/name
 started=time.monotonic()
 for attempt in range(3):
  created=False
  try:
   with urllib.request.urlopen(urllib.request.Request(url,headers={"User-Agent":"LocalAI-Cowork-V2-tool-setup"}),timeout=60) as response,destination.open("xb") as stream:
    created=True;digest=hashlib.sha256();total=0
    while True:
     if time.monotonic()-started>600:raise RuntimeError("Tool download exceeded its time bound")
     chunk=response.read1(64*1024)
     if not chunk:break
     total+=len(chunk)
     if total>bound:raise RuntimeError("Tool download exceeds its bound")
     digest.update(chunk);stream.write(chunk)
    stream.flush();os.fsync(stream.fileno())
   if digest.hexdigest()!=expected:raise RuntimeError("Official archive SHA-256 does not match")
   return destination
  except BaseException as error:
   # Only this attempt's exclusive creation may be removed, never an existing file.
   if created:destination.unlink(missing_ok=True)
   transient=(error.code in (408,429,500,502,503,504) if isinstance(error,urllib.error.HTTPError)
    else isinstance(error,urllib.error.URLError) and not isinstance(error.reason,(ssl.SSLError,PermissionError)))
   if not transient or attempt==2:raise
   delay=2*(attempt+1)
   if time.monotonic()-started+delay>=600:raise RuntimeError("Tool download exceeded its time bound") from error
   time.sleep(delay)

def extract_zip(archive,destination):
 ordinary(destination,True)
 with zipfile.ZipFile(archive) as package:
  total=0
  for member in package.infolist():
   relative=pathlib.PurePosixPath(member.filename)
   if relative.is_absolute() or ".." in relative.parts or ":" in member.filename or ((member.external_attr>>16)&0o170000)==0o120000:raise RuntimeError("Unexpected archive member")
   total+=member.file_size
   if total>2*1024**3:raise RuntimeError("Expanded archive exceeds its bound")
  package.extractall(destination)
  if os.name!='nt':
   for member in package.infolist():
    target=destination/member.filename
    if target.is_file():target.chmod(0o700 if (member.external_attr>>16)&0o111 else 0o600)

def install_managed_browsers(python,runtime,programs,owned,versions):
 lock_path=pathlib.Path(__file__).with_name('browser-runtimes.lock.json')
 recipe=json.loads(lock_path.read_text(encoding='utf-8'))
 version=subprocess.check_output([str(python),'-I','-c','import importlib.metadata; print(importlib.metadata.version("playwright"))'],env=ENV,text=True).strip()
 if recipe['format']!=1 or recipe['playwright']!=version:raise RuntimeError('Browser runtime lock does not match the pinned Playwright package')
 for engine,spec in recipe['platforms']['windows' if os.name=='nt' else 'linux'].items():
  progress(engine,'Installing private '+('Chrome for Testing' if engine=='chrome' else 'Playwright Firefox'))
  target=ordinary(runtime/(engine+'-'+spec['version']),True)
  archive=download(engine+'.zip',(spec['url'],spec['sha256']))
  extract_zip(archive,target);archive.unlink()
  executable=ordinary(target/spec['executable'])
  if not executable.is_file():raise RuntimeError('Pinned browser executable is missing')
  programs[engine]=str(executable);owned[engine]=target;versions[engine]=spec['version']

def execute(arguments):
 subprocess.run(arguments,check=True,env=ENV,stdin=subprocess.DEVNULL)

def msi_extract(archive,destination):
 """Read tables/cabinet streams only. Never call any MSI installer action."""
 api=ctypes.WinDLL("msi",use_last_error=True);handle=ctypes.c_uint
 api.MsiOpenDatabaseW.argtypes=[ctypes.c_wchar_p,ctypes.c_wchar_p,ctypes.POINTER(handle)]
 api.MsiDatabaseOpenViewW.argtypes=[handle,ctypes.c_wchar_p,ctypes.POINTER(handle)]
 api.MsiViewExecute.argtypes=[handle,handle];api.MsiViewFetch.argtypes=[handle,ctypes.POINTER(handle)]
 api.MsiRecordGetStringW.argtypes=[handle,ctypes.c_uint,ctypes.c_wchar_p,ctypes.POINTER(ctypes.c_uint)]
 api.MsiRecordReadStream.argtypes=[handle,ctypes.c_uint,ctypes.c_void_p,ctypes.POINTER(ctypes.c_uint)]
 api.MsiCloseHandle.argtypes=[handle];database=handle()
 if api.MsiOpenDatabaseW(str(archive),None,ctypes.byref(database)):raise RuntimeError("Cannot open pinned Office archive")
 def rows(query):
  view=handle()
  if api.MsiDatabaseOpenViewW(database,query,ctypes.byref(view)) or api.MsiViewExecute(view,0):raise RuntimeError("Cannot inspect Office archive")
  try:
   while True:
    record=handle();code=api.MsiViewFetch(view,ctypes.byref(record))
    if code==259:break
    if code:raise RuntimeError("Cannot read Office row")
    try:yield record
    finally:api.MsiCloseHandle(record)
  finally:api.MsiCloseHandle(view)
 def text(record,index):
  size=ctypes.c_uint(32768);buffer=ctypes.create_unicode_buffer(size.value)
  if api.MsiRecordGetStringW(record,index,buffer,ctypes.byref(size)):raise RuntimeError("Invalid Office field")
  return buffer.value
 cabinets=ordinary(STAGE/"office-cabinets",True);expanded=ordinary(STAGE/"office-expanded",True)
 try:
  media=[text(record,1) for record in rows("SELECT `Cabinet` FROM `Media`")]
  for cabinet in media:
   if not cabinet.startswith("#") or not cabinet[1:].replace(".","").replace("_","").isalnum():raise RuntimeError("Unexpected external cabinet")
   name=cabinet[1:]
   for record in rows("SELECT `Data` FROM `_Streams` WHERE `Name`='"+name+"'"):
    output=cabinets/name
    with output.open("xb") as stream:
     while True:
      size=ctypes.c_uint(65536);buffer=ctypes.create_string_buffer(size.value)
      if api.MsiRecordReadStream(record,1,buffer,ctypes.byref(size)):raise RuntimeError("Cannot read cabinet")
      if not size.value:break
      stream.write(buffer.raw[:size.value])
    execute([str(pathlib.Path(os.environ["SystemRoot"])/"System32/expand.exe"),"-F:*",str(output),str(expanded)])
  directories={text(r,1):(text(r,2),text(r,3)) for r in rows("SELECT `Directory`,`Directory_Parent`,`DefaultDir` FROM `Directory`")}
  components={text(r,1):text(r,2) for r in rows("SELECT `Component`,`Directory_` FROM `Component`")}
  def directory(key,seen=None):
   seen=set() if seen is None else seen
   if key in seen or len(seen)>64:raise RuntimeError("Invalid Office directory graph")
   seen.add(key);parent,name=directories[key];part=name.split(":")[0].split("|")[-1]
   if part in (".","SourceDir") or not parent:part=""
   if part and (part==".." or any(c in part for c in "/\\:")):raise RuntimeError("Invalid Office directory")
   return (directory(parent,seen) if parent else pathlib.Path())/part
  for record in rows("SELECT `File`,`Component_`,`FileName` FROM `File`"):
   identifier,component,name=[text(record,n) for n in (1,2,3)];name=name.split("|")[-1]
   if name in (".","..") or any(c in name for c in "/\\:"):raise RuntimeError("Invalid Office filename")
   source=expanded/identifier
   if not source.is_file():raise RuntimeError("Office cabinet is missing a required file")
   target=destination/directory(components[component])/name;ordinary(target.parent,True);shutil.copyfile(source,target)
 finally:api.MsiCloseHandle(database)

def extract_deb(deb,destination):
 def confined(member,target):
  if member.issym() and member.linkname.startswith("/"):
   member=copy.copy(member);member.linkname=posixpath.relpath(member.linkname.lstrip("/"),posixpath.dirname(member.name))
  if member.islnk() and member.linkname.startswith("/"):member=copy.copy(member);member.linkname=member.linkname.lstrip("/")
  return tarfile.data_filter(member,target)
 with deb.open("rb") as stream:
  if stream.read(8)!=b"!<arch>\n":raise RuntimeError("Invalid private package")
  while header:=stream.read(60):
   if len(header)!=60 or header[-2:]!=b"`\n":raise RuntimeError("Invalid private archive member")
   size=int(header[48:58]);name=header[:16].decode("ascii").strip().rstrip("/");content=stream.read(size)
   if len(content)!=size:raise RuntimeError("Incomplete private archive member")
   if size%2:stream.read(1)
   if name.startswith("data.tar"):
    with tarfile.open(fileobj=io.BytesIO(content)) as payload:payload.extractall(destination,filter=confined)

def linux_office_extract(archive,destination):
 packages=ordinary(STAGE/"office-debs",True)
 with tarfile.open(archive) as package:package.extractall(packages,filter="data")
 for deb in packages.rglob("*.deb"):extract_deb(deb,destination)

def linux_git(runtime):
 lock=pathlib.Path(__file__).with_name("linux-git.lock.json")
 specification=json.loads(lock.read_text(encoding="utf-8"))
 git=ordinary(runtime/("git-debian12-2.39.5-u3-"+uuid.uuid4().hex),True)
 for number,package in enumerate(specification["packages"],1):
  progress("git",f"Extracting private Git dependency {number}/{len(specification['packages'])}: {package['name']}")
  archive=download(package["name"]+".deb",(package["url"],package["sha256"]),64*1024*1024)
  extract_deb(archive,git);archive.unlink()
 wrapper_dir=ordinary(git/"cowork-bin",True)
 certificates=git/"cowork-ca.pem"
 with certificates.open("wb") as stream:
  for certificate in sorted((git/"usr/share/ca-certificates").rglob("*.crt")):stream.write(certificate.read_bytes()+b"\n")
 library_paths=[git/"usr/lib/x86_64-linux-gnu",git/"lib/x86_64-linux-gnu"]
 perl_paths=[git/"usr/share/perl5",git/"usr/share/perl/5.36",git/"usr/lib/x86_64-linux-gnu/perl/5.36"]
 prefix="#!/bin/sh\nexport LD_LIBRARY_PATH="+shlex.quote(":".join(map(str,library_paths)))+"\nexport PATH="+shlex.quote(str(wrapper_dir)+":"+str(git/"usr/bin"))+':/usr/bin:/bin\nexport GIT_EXEC_PATH='+shlex.quote(str(git/"usr/lib/git-core"))+"\nexport GIT_SSL_CAINFO="+shlex.quote(str(certificates))+"\nexport PERL5LIB="+shlex.quote(":".join(map(str,perl_paths)))+"\n"
 # Git's helper scripts use the private Perl ABI instead of an unrelated host Perl.
 for helper in (git/"usr/lib/git-core").iterdir():
  if helper.is_file() and not helper.is_symlink():
   with helper.open("rb") as stream:first=stream.readline(256)
   if first.startswith(b"#!") and b"perl" in first:
    original=helper.with_name("cowork-original-"+helper.name);helper.rename(original)
    helper.write_text(prefix+"exec "+shlex.quote(str(git/"usr/bin/perl"))+" "+shlex.quote(str(original))+' "$@"\n',encoding="utf-8");helper.chmod(0o700)
 wrapper=wrapper_dir/"git"
 wrapper.write_text(prefix+"exec "+shlex.quote(str(git/"usr/bin/git"))+' "$@"\n',encoding="utf-8");wrapper.chmod(0o700)
 return wrapper

def file_sha256(path):
 digest=hashlib.sha256()
 with open(path,"rb") as stream:
  while chunk:=stream.read(1024*1024):digest.update(chunk)
 return digest.hexdigest()

def extract_single(archive,member,destination,bound):
 """Write only the one expected regular file; nothing else in the archive is extracted."""
 if os.name=="nt":
  with zipfile.ZipFile(archive) as package:
   info=package.getinfo(member)
   if info.is_dir() or info.file_size>bound or ((info.external_attr>>16)&0o170000)==0o120000:raise RuntimeError("Unexpected Tectonic archive member")
   with package.open(info) as source,destination.open("xb") as stream:shutil.copyfileobj(source,stream);stream.flush();os.fsync(stream.fileno())
 else:
  with tarfile.open(archive) as package:
   info=package.getmember(member)
   if not info.isfile() or info.size>bound:raise RuntimeError("Unexpected Tectonic archive member")
   with package.extractfile(info) as source,destination.open("xb") as stream:shutil.copyfileobj(source,stream);stream.flush();os.fsync(stream.fileno())
  destination.chmod(0o700)

def tex_run(tectonic,work,name,env,offline):
 """Compile one trusted warm-up document; online runs fill the cache, offline runs prove it suffices."""
 output=ordinary(work/("offline" if offline else "online"),True)
 arguments=[str(tectonic),"-X","compile","--untrusted",*(["--only-cached"] if offline else []),"--outdir",str(output),name+".tex"]
 result=subprocess.run(arguments,cwd=work,env=env,stdin=subprocess.DEVNULL,capture_output=True,text=True,encoding="utf-8",errors="replace",timeout=300 if offline else 1800)
 if result.returncode==0 and not (output/(name+".pdf")).is_file():result.returncode=1
 return result

def tex_failure(name,result):
 lines=[line for line in (result.stderr+result.stdout).splitlines() if line and not line.startswith("note: downloading")]
 return "LaTeX warm-up failed for "+name+": "+"\n".join(lines[-12:])[-1500:]

def bundle_digests(cache):
 """Bundle URL -> digest as recorded by Tectonic (file names escape ':' and '/' as ',58,' and ',47,')."""
 hashes=cache/"bundles"/"hashes";found={}
 if hashes.is_dir():
  for entry in hashes.iterdir():
   if entry.is_file() and not entry.name.endswith(".lock") and entry.stat().st_size<256:
    found[entry.name.replace(",58,",":").replace(",47,","/")]=entry.read_text(encoding="ascii",errors="replace").strip()
 return found

def warm_tectonic(tectonic,runtime):
 """Pre-fill the private Tectonic cache so compiles need no network, then bind it to this binary."""
 cache=ordinary(runtime/"tectonic-cache",True);marker=runtime/"tectonic-warm.json";ordinary(marker)
 # Tectonic cannot persist <cache>\formats\<64 hex>-latex-33.fmt at 260 characters (os error 3);
 # the longest bundle file is <cache>\bundles\data\<64 hex>\ plus 59 characters.
 if os.name=="nt" and len(str(cache))>115:raise RuntimeError("The private data folder path is too long for the offline LaTeX cache; use a data folder with a shorter path")
 digest=file_sha256(tectonic);sources=hashlib.sha256(json.dumps(TEX_WARMUP,sort_keys=True,ensure_ascii=False).encode("utf-8")).hexdigest()
 pinned=digest==TECTONIC["binary"]["windows" if os.name=="nt" else "linux"][1]
 env=dict(ENV,TECTONIC_CACHE_DIR=str(cache))
 # A dead local proxy proves the offline compile never reaches the bundle server.
 offline=dict(env,HTTPS_PROXY="http://127.0.0.1:9",HTTP_PROXY="http://127.0.0.1:9",ALL_PROXY="http://127.0.0.1:9",NO_PROXY="")
 work=ordinary(STAGE/"tex-warmup",True)
 for name,source in TEX_WARMUP.items():(work/(name+".tex")).write_text(source,encoding="utf-8",newline="\n")
 try:existing=json.loads(marker.read_text(encoding="utf-8")) if marker.exists() else None
 except ValueError:existing=None
 if isinstance(existing,dict) and existing.get("format")=="cowork-tectonic-warm/1" and existing.get("tectonic_sha256")==digest and existing.get("warmup_sha256")==sources:
  progress("tectonic","Checking the existing offline LaTeX cache")
  if tex_run(tectonic,work,"article",offline,True).returncode==0:return existing
 if marker.exists():marker.unlink()
 for number,name in enumerate(TEX_WARMUP,1):
  progress("tectonic",f"Downloading LaTeX packages and fonts for offline use ({number}/{len(TEX_WARMUP)})")
  for attempt in range(3):
   result=tex_run(tectonic,work,name,env,False)
   if result.returncode==0:break
   time.sleep(5*(attempt+1))
  else:raise RuntimeError(tex_failure(name,result))
 progress("tectonic","Checking that LaTeX compiles without network")
 for name in TEX_WARMUP:
  result=tex_run(tectonic,work,name,offline,True)
  if result.returncode:raise RuntimeError(tex_failure(name,result))
 bundles=bundle_digests(cache)
 if pinned and bundles.get(TECTONIC["bundle"])!=TECTONIC["bundle_digest"]:raise RuntimeError("The downloaded LaTeX package bundle does not match the pinned bundle")
 packages=set()
 for source in TEX_WARMUP.values():
  for match in re.finditer(r"\\(?:usepackage|documentclass|usetikzlibrary)(?:\[[^\]]*\])?\{([^}]*)\}",source):packages.update(p.strip() for p in match[1].split(",") if p.strip())
 fonts=sorted(set(re.findall(r"[A-Za-z0-9-]+\.(?:otf|ttf)",json.dumps(TEX_WARMUP)))|{"NotoSerif","NotoSans","NotoSansMono","texgyretermes","texgyreheros","texgyrecursor"})
 value={"format":"cowork-tectonic-warm/1","tectonic_version":TECTONIC["version"] if pinned else None,"tectonic_sha256":digest,
  "bundle":TECTONIC["bundle"],"bundle_digest":bundles.get(TECTONIC["bundle"]),"packages":sorted(packages),"fonts":fonts,
  "warmup_sha256":sources,"completed_at":int(time.time()*1000)}
 atomic(marker,value)
 return value

def install_tectonic(runtime,programs,owned,versions):
 """Pinned private Tectonic unless the user selected another program; a stale private copy is replaced."""
 platform="windows" if os.name=="nt" else "linux";name,expected=TECTONIC["binary"][platform]
 configured=pathlib.Path(programs["tectonic"]) if programs.get("tectonic") else None
 private=configured is not None and configured.parent.parent==runtime and configured.parent.name.startswith("tectonic-")
 if private and (not configured.is_file() or file_sha256(configured)!=expected):configured=None
 if configured is not None and not configured.is_file():raise RuntimeError("The selected Tectonic program is missing")
 if configured is None:
  progress("tectonic","Installing private Tectonic "+TECTONIC["version"])
  target=ordinary(runtime/("tectonic-"+TECTONIC["version"]+"-"+uuid.uuid4().hex),True)
  archive=download("tectonic.archive",DOWNLOADS["tectonic_"+platform],64*1024*1024)
  extract_single(archive,name,target/name,256*1024*1024);archive.unlink()
  if file_sha256(target/name)!=expected:raise RuntimeError("Pinned Tectonic binary SHA-256 does not match")
  configured=target/name;private=True
 if private:owned['tectonic']=configured.parent
 programs["tectonic"]=str(configured.absolute())
 result=subprocess.run([str(configured),"--version"],env=ENV,check=True,timeout=45,stdin=subprocess.DEVNULL,capture_output=True,text=True)
 versions['tectonic']=result.stdout.strip().splitlines()[0][:256] if result.stdout.strip() else None
 warm=warm_tectonic(configured,runtime)
 versions['tectonic-cache']=warm.get("bundle_digest")
 return configured,warm

def install_libreoffice(runtime,programs,owned):
 if "libreoffice" in programs:
  configured=pathlib.Path(programs["libreoffice"])
  if not configured.is_absolute() or not ordinary(configured).is_file():raise RuntimeError("The selected LibreOffice program is missing")
  return
 platform="windows" if os.name=="nt" else "linux"
 progress("libreoffice","Extracting private LibreOffice; no system installation")
 office=ordinary(runtime/("libreoffice-26.8.1-"+uuid.uuid4().hex),True)
 archive=download("office.archive",DOWNLOADS["office_"+platform])
 if os.name=="nt":msi_extract(archive,office)
 else:linux_office_extract(archive,office)
 names=("soffice.com","soffice.exe") if os.name=="nt" else ("soffice",)
 selected=next((p for name in names for p in sorted(office.rglob(name)) if p.is_file()),None)
 if selected is None:raise RuntimeError("Private LibreOffice is missing")
 programs["libreoffice"]=str(selected);owned['libreoffice']=office

def main():
 global DATA,STAGE,ENV
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument("--data-dir",type=pathlib.Path,required=True)
 parser.add_argument("--embedded",action="store_true")
 parser.add_argument("--profile",choices=sorted(PROFILES),default="full",help="documents: document wheels and offline LaTeX; full/browser: also Playwright browsers")
 only=parser.add_mutually_exclusive_group()
 only.add_argument("--tectonic-only",action="store_true",help="image builds: only install and warm Tectonic and record it in toolbox.json")
 only.add_argument("--libreoffice-only",action="store_true",help="qualification/image builds: only extract private LibreOffice and record it in toolbox.json")
 parser.add_argument("--git",action="store_true");parser.add_argument("--github-cli",action="store_true");parser.add_argument("--libreoffice",action="store_true")
 args=parser.parse_args();DATA=ordinary(args.data_dir,True);runtime=ordinary(DATA/"tool-runtime",True)
 STAGE=ordinary(runtime/("setup-"+uuid.uuid4().hex),True)
 # Windows reports environment names upper-case (SYSTEMROOT); without it TLS in pip and Tectonic fails.
 ENV={k:v for k,v in os.environ.items() if k.upper() in ("PATH","SYSTEMROOT","WINDIR","TEMP","TMP","LANG","LC_ALL")}
 for name in ("HOME","USERPROFILE","TMP","TEMP","TMPDIR"):ENV[name]=str(ordinary(runtime/("tmp" if name in ("TMP","TEMP","TMPDIR") else "home"),True))
 # Tectonic resolves the Windows known folders below USERPROFILE and exits without them.
 if os.name=="nt":
  for folder in ("AppData/Roaming","AppData/Local"):ordinary(runtime/"home"/folder,True)
 ENV.update({"PYTHONNOUSERSITE":"1","PIP_CONFIG_FILE":os.devnull,"PIP_DISABLE_PIP_VERSION_CHECK":"1","PIP_NO_INPUT":"1","PIP_CACHE_DIR":str(runtime/"pip-cache"),"PLAYWRIGHT_BROWSERS_PATH":str(runtime/"browsers")})
 profiles=PROFILES[args.profile];browser="browser" in profiles
 python=pathlib.Path(sys.executable);owned={};versions={}
 config_path=DATA/"toolbox.json";ordinary(config_path)
 if args.tectonic_only or args.libreoffice_only:
  config=json.loads(config_path.read_text(encoding="utf-8")) if config_path.exists() else {};programs=config.setdefault("programs",{})
  if args.tectonic_only:install_tectonic(runtime,programs,owned,versions)
  else:install_libreoffice(runtime,programs,owned)
  atomic(config_path,config);progress("ready","Private tool runtime is ready")
  scratch=ordinary(STAGE)
  if scratch.parent==runtime and scratch.name.startswith("setup-") and scratch.is_relative_to(DATA):shutil.rmtree(scratch)
  return
 if not args.embedded:
  progress("python","Creating a private Python environment");environment=runtime/"python"
  if not environment.exists():execute([str(python),"-I","-m","venv","--copies",str(environment)])
  python=environment/("Scripts/python.exe" if os.name=="nt" else "bin/python")
  owned['python']=environment
 elif python.is_relative_to(DATA):owned['python']=python.parent if os.name=='nt' else python.parent.parent
 progress("packages","Installing pinned Office and PDF"+(" and browser" if browser else "")+" wheels");pip=download("pip-26.2-py3-none-any.whl",PIP,8*1024*1024)
 if args.embedded and os.name=="nt":extract_zip(pip,ordinary(python.parent/"Lib/site-packages",True))
 else:execute([str(python),"-I","-m","pip","install","--no-index","--no-deps",str(pip)])
 locks={"documents":pathlib.Path(__file__).with_name("requirements-documents.lock"),"ocr":pathlib.Path(__file__).with_name("requirements-ocr.lock"),"browser":pathlib.Path(__file__).with_name("requirements-browser.lock")}
 selected=[locks[profile] for profile in profiles if profile!="ocr"]
 if not all(locks[profile].is_file() for profile in profiles):raise RuntimeError("Pinned requirements are missing")
 execute([str(python),"-I","-m","pip","install","--no-cache-dir","--index-url","https://pypi.org/simple","--require-hashes","--only-binary=:all:","--no-deps",*[argument for lock in selected for argument in ("-r",str(lock))]])
 # pip considers an installed version satisfied even when its native files have changed.
 # Reinstall OCR from the pinned wheel so repeating setup actually repairs that state.
 progress("packages","Installing verified local text recognition")
 execute([str(python),"-I","-m","pip","install","--no-cache-dir","--index-url","https://pypi.org/simple","--require-hashes","--only-binary=:all:","--no-deps","--force-reinstall","-r",str(locks["ocr"])])
 progress("packages","Installing pinned local OCR language data")
 ocr_path=pathlib.Path(__file__).with_name("ocr_setup.py")
 ocr_spec=importlib.util.spec_from_file_location('cowork_private_ocr_setup',ocr_path)
 ocr_module=importlib.util.module_from_spec(ocr_spec);ocr_spec.loader.exec_module(ocr_module)
 ocr_receipt=ocr_module.install_models(runtime,download,ordinary,atomic)
 owned['ocr']=runtime/'ocr'
 if browser:
  progress("chromium","Installing a private Chromium browser");execute([str(python),"-I","-m","playwright","install","chromium"])
  chromium=subprocess.check_output([str(python),"-I","-c","from playwright.sync_api import sync_playwright\nwith sync_playwright() as p: print(p.chromium.executable_path)"],env=ENV,text=True).strip()
  owned['chromium']=runtime/'browsers'
 config=json.loads(config_path.read_text(encoding="utf-8")) if config_path.exists() else {};programs=config.setdefault("programs",{})
 programs["python"]=str(python.absolute())
 if browser:
  programs["chromium"]=chromium
  install_managed_browsers(python,runtime,programs,owned,versions)
 for name in ("git","gh","node","libreoffice","powershell","bash","docker"):
  executable=shutil.which(name)
  if executable:programs.setdefault(name,str(pathlib.Path(executable).absolute()))
 platform="windows" if os.name=="nt" else "linux"
 if args.git and "git" not in programs:
  progress("git","Installing private Git")
  if os.name=="nt":
   git=ordinary(runtime/("git-2.56.0.2-"+uuid.uuid4().hex),True);extract_zip(download("git.zip",DOWNLOADS["git_windows"]),git);programs["git"]=str(git/"cmd/git.exe");owned['git']=git
  else:
   programs["git"]=str(linux_git(runtime));owned['git']=pathlib.Path(programs['git']).parent.parent
 if args.github_cli and "gh" not in programs:
  progress("github_cli","Installing private GitHub CLI");gh=ordinary(runtime/("gh-2.102.0-"+uuid.uuid4().hex),True);archive=download("gh.archive",DOWNLOADS["gh_"+platform])
  if os.name=="nt":extract_zip(archive,gh)
  else:
   with tarfile.open(archive) as package:package.extractall(gh,filter="data")
  selected=next((p for p in gh.rglob("gh.exe" if os.name=="nt" else "gh") if p.is_file()),None)
  if selected is None:raise RuntimeError("Private GitHub CLI is missing")
  programs["gh"]=str(selected)
  owned['gh']=gh
 if args.libreoffice and "libreoffice" not in programs:
  install_libreoffice(runtime,programs,owned)
 tectonic,warm=install_tectonic(runtime,programs,owned,versions)
 owned['tectonic-cache']=runtime/"tectonic-cache"
 progress("verify","Checking the configured adapter imports")
 execute([str(python),"-I","-c","import docx,openpyxl,pptx,pypdf,pypdfium2,PIL,lxml,reportlab,xlsxwriter; print('Office/PDF/image imports ready')"])
 ocr_module.verify_runtime(python,runtime,execute)
 if browser:
  execute([str(python),"-I","-c","import playwright.sync_api; print('Playwright imports ready')"])
  if not pathlib.Path(chromium).is_file():raise RuntimeError("Chromium executable is missing")
  progress("verify","Opening private Chromium on a blank local page")
  versions['chromium']=subprocess.check_output([str(python),"-I","-c","import sys; from playwright.sync_api import sync_playwright\nwith sync_playwright() as p:\n b=p.chromium.launch(executable_path=sys.argv[1],headless=True); page=b.new_page(); page.goto('about:blank'); print(b.version); b.close()",chromium],env=ENV,text=True,timeout=60).strip()
  for engine in ('chrome','firefox'):
   progress('verify','Checking private '+engine)
   subprocess.run([str(python),'-I','-c','import sys; from playwright.sync_api import sync_playwright\nwith sync_playwright() as p:\n engine=p.firefox if sys.argv[1]=="firefox" else p.chromium\n b=engine.launch(executable_path=sys.argv[2],headless=True); page=b.new_page(); page.goto("about:blank"); b.close()',engine,programs[engine]],env=ENV,check=True,timeout=60,stdin=subprocess.DEVNULL)
 versions['python']=subprocess.check_output([str(python),'-I','-c','import platform; print(platform.python_version())'],env=ENV,text=True,timeout=15).strip()
 for name in ("git","gh","libreoffice"):
  if name in programs:
   result=subprocess.run([programs[name],"--version"],env=ENV,check=True,timeout=45,stdin=subprocess.DEVNULL,capture_output=True,text=True)
   versions[name]=result.stdout.strip().splitlines()[0][:256] if result.stdout.strip() else None
 progress('inventory','Recording actual private native libraries, Python packages and tool files')
 helper=pathlib.Path(__file__).with_name('sbom.py')
 if not helper.is_file():helper=pathlib.Path(__file__).resolve().parents[1]/'release/sbom.py'
 specification=importlib.util.spec_from_file_location('cowork_installed_sbom',helper)
 sbom_module=importlib.util.module_from_spec(specification);specification.loader.exec_module(sbom_module)
 inventories=[];external=[]
 for name,path in sorted(owned.items()):
  components=[]
  if name=='git' and os.name!='nt':
   recipe=json.loads(pathlib.Path(__file__).with_name('linux-git.lock.json').read_text(encoding='utf-8'))
   components=[{**package,'ecosystem':'deb','source':'Original pinned Debian archive used by this private setup'} for package in recipe['packages']]
  if name=='tectonic-cache':
   components=[{'name':'tectonic-bundle-tlextras','version':'2022.0r0','ecosystem':'generic','license':'NOASSERTION','sha256':warm.get('bundle_digest'),'source':'Files fetched on demand from '+TECTONIC['bundle']+' (per-file TeX Live licenses)'}]
  inventories.append(sbom_module.runtime.capture(path,name=name,version=versions.get(name),components=components))
 for name,path in sorted(programs.items()):
  if name not in owned:external.append((name,pathlib.Path(path).resolve(strict=True),versions.get(name)))
 installed_sbom=sbom_module.generate_runtime(inventories,external_programs=external)
 sbom_path=DATA/('tool-runtime-sbom-'+uuid.uuid4().hex+'.spdx.json')
 with sbom_path.open('x',encoding='utf-8',newline='\n') as stream:
  json.dump(installed_sbom,stream,sort_keys=True,indent=2);stream.write('\n');stream.flush();os.fsync(stream.fileno())
 if os.name!='nt':sbom_path.chmod(0o600)
 sbom_receipt={'filename':sbom_path.name,'sha256':hashlib.sha256(sbom_path.read_bytes()).hexdigest(),'files':len(installed_sbom['files']),'components':len(installed_sbom['packages'])}
 atomic(config_path,config)
 # Receipt /3 keeps one entry per profile; a later documents-only run keeps an earlier browser entry.
 receipt_path=DATA/"tool-setup-receipt.json";ordinary(receipt_path)
 try:previous=json.loads(receipt_path.read_text(encoding="utf-8")) if receipt_path.exists() else {}
 except ValueError:previous={}
 recorded=dict(previous.get("profiles",{})) if previous.get("format")=="localai-cowork-tool-setup/3" else {}
 if previous.get("format")=="localai-cowork-tool-setup/2" and previous.get("ready") is True:
  recorded["browser"]={"ready":True,"legacy_format":"localai-cowork-tool-setup/2","completed_at":previous.get("completed_at")}
 now=int(time.time()*1000)
 recorded["documents"]={"ready":True,"requirements_sha256":file_sha256(locks["documents"]),"tectonic_sha256":warm["tectonic_sha256"],"tectonic_version":warm.get("tectonic_version"),"bundle_digest":warm.get("bundle_digest"),"python":versions['python'],"completed_at":now}
 recorded["ocr"]={"ready":True,"requirements_sha256":file_sha256(locks["ocr"]),"language_manifest_sha256":ocr_receipt['language_manifest_sha256'],"binding_version":ocr_receipt['binding_version'],"completed_at":now}
 if browser:
  recorded["browser"]={"ready":True,"requirements_sha256":file_sha256(locks["browser"]),"browser_lock_sha256":file_sha256(pathlib.Path(__file__).with_name('browser-runtimes.lock.json')),"completed_at":now}
 atomic(receipt_path,{"format":"localai-cowork-tool-setup/3","ready":all(recorded.get(profile,{}).get("ready") is True for profile in profiles),"profile":args.profile,"profiles":recorded,"programs":programs,"completed_at":now,"global_installation":False,'sbom':sbom_receipt})
 progress("ready","Private tools configured; reload this instance's toolbox to use them")
 # Delete only this completed setup's own scratch directory, after checking its
 # ordinary absolute location. The actual Python/browser/tool runtimes remain.
 scratch=ordinary(STAGE)
 if scratch.parent==runtime and scratch.name.startswith("setup-") and scratch.is_relative_to(DATA):shutil.rmtree(scratch)

if __name__=="__main__":
 try:main()
 except Exception as error:
  if "DATA" in globals():progress("failed",str(error)[:2048])
  raise
