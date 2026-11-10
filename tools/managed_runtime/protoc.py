"""Fetch a pinned host build tool into .local; never installs a global program.

Runtime users do not need protoc. Generated containerd bindings are compiled into
the runtime service; this tool is only for reproducible developer/CI builds.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import urllib.request
import uuid
import zipfile

VERSION="33.5"
PINS={"linux-x86_64":("protoc-33.5-linux-x86_64.zip","24e58fb231d50306ee28491f33a170301e99540f7e29ca461e0e80fd1239f8d1"),
      "windows-x86_64":("protoc-33.5-win64.zip","7e3468cd1fbd1ae9361a5304d4ac28fbd593aa1a425b5464bd9d4da5fca224b4")}
UPSTREAM="https://api.github.com/repos/protocolbuffers/protobuf/releases/tags/v33.5"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def bootstrap(root,platform):
    archive_name,expected=PINS[platform]
    parent=root/".local/managed-runtime-build-tools"/VERSION
    parent.mkdir(parents=True,exist_ok=True)
    destination=parent/platform
    if destination.exists():
        receipt=json.loads((destination/"receipt.json").read_text())
        if receipt.get("archive_sha256")!=expected or any(not (destination/n).is_file() or (destination/n).is_symlink() or sha(destination/n)!=h for n,h in receipt["files"].items()):
            raise ValueError("Private build tool changed; select a new version, do not overwrite it")
        return destination
    archive=parent/archive_name
    if not archive.exists():
        temporary=parent/(archive_name+"."+uuid.uuid4().hex+".download")
        url="https://github.com/protocolbuffers/protobuf/releases/download/v"+VERSION+"/"+archive_name
        with urllib.request.urlopen(url,timeout=60) as source, temporary.open("xb") as target:
            total=0
            while block:=source.read(1024*1024):
                total+=len(block)
                if total>32*1024*1024:
                    raise ValueError("Build tool download exceeded its bound")
                target.write(block)
            target.flush();os.fsync(target.fileno())
        if sha(temporary)!=expected:
            raise ValueError("Build tool archive checksum mismatch")
        temporary.rename(archive)
    if archive.is_symlink() or sha(archive)!=expected:
        raise ValueError("Pinned build tool archive changed")
    stage=parent/(platform+".stage-"+uuid.uuid4().hex)
    stage.mkdir(mode=0o700)
    files={}
    with zipfile.ZipFile(archive) as bundle:
        entries=bundle.infolist()
        if len(entries)>512 or sum(e.file_size for e in entries)>128*1024*1024:
            raise ValueError("Oversized build tool archive")
        seen=set()
        for entry in entries:
            path=PurePosixPath(entry.filename)
            if path.is_absolute() or any(p in (".","..") for p in path.parts) or "\\" in entry.filename or ":" in entry.filename or stat.S_ISLNK(entry.external_attr>>16):
                raise ValueError("Nonportable build tool entry")
            key=entry.filename.rstrip("/").casefold()
            if key in seen:
                raise ValueError("Duplicate build tool entry")
            seen.add(key)
            target=stage/Path(*path.parts)
            if entry.is_dir():
                target.mkdir(parents=True,exist_ok=True)
                continue
            target.parent.mkdir(parents=True,exist_ok=True)
            with bundle.open(entry) as source,target.open("xb") as sink:
                shutil.copyfileobj(source,sink,1024*1024)
            if platform=="linux-x86_64" and entry.filename=="bin/protoc":
                target.chmod(0o755)
            files[entry.filename]=sha(target)
    receipt={"version":VERSION,"platform":platform,"upstream_release":UPSTREAM,"archive_sha256":expected,
             "files":files,"scope":"private host build dependency only","runtime_installed":False}
    (stage/"receipt.json").write_text(json.dumps(receipt,indent=2)+"\n",encoding="utf-8")
    stage.rename(destination)
    return destination


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform",choices=PINS,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    directory=bootstrap(root,args.platform)
    print(json.dumps({"directory":str(directory),"version":VERSION,"platform":args.platform,"global_install":False}))
