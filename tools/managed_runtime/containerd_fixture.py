"""Download the exact upstream containerd binary for a private transport fixture.

Not an installer, runtime activation or platform qualification. No service starts.
"""
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import urllib.request
import uuid

VERSION="2.2.1"
ARCHIVE_SHA="f5d8e90ecb6c1c7e33ecddf8cc268a93b9e5b54e0e850320d765511d76624f41"
BINARY_SHA="d2bd1104a4bed940cc1bb1da819b87f187ce24064ec359e8cd1132e755f9475a"
URL="https://github.com/containerd/containerd/releases/download/v2.2.1/containerd-2.2.1-linux-amd64.tar.gz"


def main():
    root=Path(__file__).resolve().parents[2]
    parent=root/".local/managed-runtime-fixtures"
    parent.mkdir(parents=True,exist_ok=True)
    archive=parent/"containerd-2.2.1-linux-amd64.tar.gz"
    if not archive.exists():
        temporary=parent/("containerd-"+uuid.uuid4().hex+".download")
        with urllib.request.urlopen(URL,timeout=60) as source,temporary.open("xb") as target:
            count=0
            while block:=source.read(1024*1024):
                count+=len(block)
                if count>64*1024*1024:raise ValueError("Fixture download exceeded its bound")
                target.write(block)
        if hashlib.sha256(temporary.read_bytes()).hexdigest()!=ARCHIVE_SHA:raise ValueError("Upstream archive checksum changed")
        temporary.rename(archive)
    if archive.is_symlink() or hashlib.sha256(archive.read_bytes()).hexdigest()!=ARCHIVE_SHA:raise ValueError("Fixture archive changed")
    destination=parent/("containerd-"+VERSION)
    binary=destination/"containerd"
    if destination.exists():
        receipt=json.loads((destination/"receipt.json").read_text())
        if binary.is_symlink() or hashlib.sha256(binary.read_bytes()).hexdigest()!=BINARY_SHA or receipt["binary_sha256"]!=BINARY_SHA or receipt["archive_sha256"]!=ARCHIVE_SHA:raise ValueError("Fixture binary changed")
    else:
        destination.mkdir(mode=0o700)
        with tarfile.open(archive) as package:
            members=[m for m in package if m.name=="bin/containerd"]
            if len(members)!=1 or not members[0].isfile() or members[0].size>128*1024*1024:raise ValueError("Unexpected upstream binary")
            with package.extractfile(members[0]) as source,binary.open("xb") as target:
                shutil.copyfileobj(source,target,1024*1024)
        binary.chmod(0o755)
        if hashlib.sha256(binary.read_bytes()).hexdigest()!=BINARY_SHA:raise ValueError("Exact upstream ELF checksum changed")
        receipt={"version":VERSION,"upstream":URL,"archive_sha256":ARCHIVE_SHA,"binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
                 "scope":"private read-only RPC fixture binary only","runtime_installed":False,"profile_qualified":False}
        (destination/"receipt.json").write_text(json.dumps(receipt,indent=2)+"\n")
    print(json.dumps({"binary":str(binary),"sha256":receipt["binary_sha256"],"version":VERSION,"runtime_installed":False}))


if __name__=="__main__":main()
