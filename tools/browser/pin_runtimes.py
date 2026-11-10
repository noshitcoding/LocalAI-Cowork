"""Maintainer operation: pin official browser archives and retain local test copies.

Runtime installation consumes the resulting lock; it never resolves latest.
"""
import argparse,concurrent.futures,hashlib,json,pathlib,urllib.request

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('--chrome-version',required=True);parser.add_argument('--firefox-revision',required=True);parser.add_argument('--playwright-version',required=True)
 args=parser.parse_args();root=pathlib.Path(__file__).resolve().parents[2];cache=root/'.local/browser-runtime-archives';cache.mkdir(parents=True,exist_ok=True)
 entries=[]
 for platform,chrome_platform,firefox_archive,chrome_path,firefox_path in [
  ('windows','win64','firefox-win64.zip','chrome-win64/chrome.exe','firefox/firefox.exe'),
  ('linux','linux64','firefox-ubuntu-24.04.zip','chrome-linux64/chrome','firefox/firefox')]:
  entries.extend([(platform,'chrome',f'https://storage.googleapis.com/chrome-for-testing-public/{args.chrome_version}/{chrome_platform}/chrome-{chrome_platform}.zip',chrome_path,args.chrome_version),
                  (platform,'firefox',f'https://cdn.playwright.dev/dbazure/download/playwright/builds/firefox/{args.firefox_revision}/{firefox_archive}',firefox_path,args.firefox_revision)])
 def fetch(entry):
  platform,engine,url,executable,version=entry;path=cache/f'{engine}-{platform}-{version}.zip'
  if not path.exists():
   with urllib.request.urlopen(url,timeout=60) as response,path.open('xb') as output:
    total=0
    while chunk:=response.read(1024*1024):
     total+=len(chunk)
     if total>600*1024*1024:raise RuntimeError('Browser archive exceeds limit')
     output.write(chunk)
  digest=hashlib.file_digest(path.open('rb'),'sha256').hexdigest();print(engine,platform,path.stat().st_size,digest,flush=True)
  return platform,engine,{'url':url,'sha256':digest,'executable':executable,'version':version}
 result={'format':1,'playwright':args.playwright_version,'platforms':{'windows':{},'linux':{}}}
 with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
  for platform,engine,value in pool.map(fetch,entries):result['platforms'][platform][engine]=value
 (root/'tools/toolbox/browser-runtimes.lock.json').write_text(json.dumps(result,indent=2)+'\n')
if __name__=='__main__':main()
