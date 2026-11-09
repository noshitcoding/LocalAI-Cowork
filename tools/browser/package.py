"""Deterministic store/development archives; never signs or uploads extensions."""
import hashlib,json,pathlib,zipfile
root=pathlib.Path(__file__).resolve().parents[2]
output=root/'dist/browser-extensions'
receipts=[]
for engine in ('chrome','firefox'):
 directory=output/engine
 manifest=json.loads((directory/'manifest.json').read_text())
 expected={'manifest.json','background.js','policy.js','popup.js','popup.html'}
 if {p.name for p in directory.iterdir()}!=expected:raise RuntimeError('Unexpected extension output files; use a clean output directory')
 archive=output/f'localai-cowork-{engine}-{manifest["version"]}.zip'
 with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as bundle:
  for file in sorted(directory.iterdir()):
   if not file.is_file() or file.is_symlink():raise RuntimeError('Unexpected extension package entry')
   info=zipfile.ZipInfo(file.name,(2026,1,1,0,0,0));info.compress_type=zipfile.ZIP_DEFLATED;info.external_attr=0o100644<<16
   bundle.writestr(info,file.read_bytes())
 receipts.append({'file':archive.name,'sha256':hashlib.sha256(archive.read_bytes()).hexdigest()})
(output/'SHA256SUMS.json').write_text(json.dumps(receipts,indent=2)+'\n')
print(json.dumps(receipts,indent=2))
