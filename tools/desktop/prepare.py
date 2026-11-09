"""Stage the exact native development sidecar using Cargo's reported artifact path."""
import json, os, pathlib, shutil, subprocess

root=pathlib.Path(__file__).resolve().parents[2]
version=subprocess.check_output(["rustc","-vV"],cwd=root,text=True)
host=next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
result=subprocess.run(["cargo","build","--locked","-p","cowork-daemon","-p","cowork-browser-host","--target",host,"--message-format=json-render-diagnostics"],cwd=root,stdout=subprocess.PIPE,text=True,check=True)
artifacts=[json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
binary=next(pathlib.Path(item["executable"]) for item in artifacts if item.get("reason")=="compiler-artifact" and item.get("target",{}).get("name")=="cowork-daemon" and item.get("executable"))
directory=root/"apps/desktop/src-tauri/binaries"
directory.mkdir(parents=True,exist_ok=True)
destination=directory/("cowork-daemon-"+host+(".exe" if os.name=="nt" else ""))
shutil.copy2(binary,destination)
print("Staged native development sidecar:",destination)
print("Release builds still require the release profile and qualification workflow.")

browser_binary=next(pathlib.Path(item["executable"]) for item in artifacts if item.get("reason")=="compiler-artifact" and item.get("target",{}).get("name")=="cowork-browser-host" and item.get("executable"))
browser_destination=directory/("cowork-browser-host-"+host+(".exe" if os.name=="nt" else ""))
shutil.copy2(browser_binary,browser_destination)
print("Staged browser host:",browser_destination)
