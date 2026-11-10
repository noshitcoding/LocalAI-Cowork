"""Acquire the exact complete upstream gVisor toolset for private physical tests.

No global installation, service configuration, activation or workload starts.
All cache bytes are checked against the pinned upstream archive on every use.
"""
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile
import urllib.request
import uuid

VERSION = "release-20261026.0"
ARCHIVE_SHA = "096589d2d6078ba4caa26aa0200932f571aa5da9d08a5156b98ab1d93fa54468"
URL = "https://github.com/google/gvisor/releases/download/" + VERSION + "/gvisor-x86_64.tar.bz2"


def sha(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    root = Path(__file__).resolve().parents[2]
    parent = root / ".local/managed-runtime-fixtures"
    parent.mkdir(parents=True, exist_ok=True)
    archive = parent / ("gvisor-" + VERSION + "-x86_64.tar.bz2")
    if not archive.exists():
        temporary = parent / ("gvisor-" + uuid.uuid4().hex + ".download")
        with urllib.request.urlopen(URL, timeout=60) as source, temporary.open("xb") as target:
            count = 0
            while block := source.read(1024 * 1024):
                count += len(block)
                if count > 128 * 1024**2:
                    raise ValueError("gVisor fixture download exceeded its bound")
                target.write(block)
        if sha(temporary) != ARCHIVE_SHA:
            raise ValueError("Pinned gVisor release archive changed")
        temporary.rename(archive)
    if archive.is_symlink() or sha(archive) != ARCHIVE_SHA:
        raise ValueError("Cached gVisor archive changed")
    destination = parent / ("gvisor-" + VERSION)
    if destination.is_symlink():
        raise ValueError("Toolset destination is a link")
    destination.mkdir(mode=0o700, exist_ok=True)
    files = {}
    total = 0
    with tarfile.open(archive) as bundle:
        entries = bundle.getmembers()
        if len(entries) > 64:
            raise ValueError("Unexpected toolset entry count")
        for entry in entries:
            path = PurePosixPath(entry.name)
            if (path.is_absolute() or any(p in ("", ".", "..") for p in path.parts)
                    or not (path.parts[0] == "gvisor-bin" or path.as_posix() in ("runsc", "containerd-shim-runsc-v1"))
                    or not (entry.isfile() or entry.isdir())):
                raise ValueError("Noncanonical or linked upstream toolset entry: " + entry.name)
            target = destination.joinpath(*path.parts)
            if target.is_symlink():
                raise ValueError("Linked cached toolset file")
            if entry.isdir():
                target.mkdir(mode=0o755, parents=True, exist_ok=True)
                continue
            if entry.size < 4 or entry.size > 128 * 1024**2 or entry.name in files:
                raise ValueError("Unexpected toolset executable")
            total += entry.size
            if total > 512 * 1024**2:
                raise ValueError("Expanded toolset exceeds its bound")
            with bundle.extractfile(entry) as source:
                data = source.read(entry.size + 1)
            if len(data) != entry.size or not data.startswith(b"\x7fELF"):
                raise ValueError("Toolset entry is not the exact ELF")
            expected = hashlib.sha256(data).hexdigest()
            if target.exists():
                if not target.is_file() or sha(target) != expected:
                    raise ValueError("Cached toolset executable changed")
            else:
                target.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
                with target.open("xb") as sink:
                    sink.write(data)
                target.chmod(0o555)
            files[path.as_posix()] = {"bytes": entry.size, "sha256": expected}
    if not {"runsc", "containerd-shim-runsc-v1"}.issubset(files) or not any(p.startswith("gvisor-bin/") for p in files):
        raise ValueError("Incomplete modern gVisor toolset")
    present = {p.relative_to(destination).as_posix() for p in destination.rglob("*") if p.is_file() and p.name != "receipt.json"}
    if present != set(files) or any(p.is_symlink() for p in destination.rglob("*")):
        raise ValueError("Unexpected cached toolset bytes")
    receipt = {"version": VERSION, "upstream": URL, "archive_sha256": ARCHIVE_SHA, "files": files,
               "scope": "Complete private development toolset only", "installed": False, "profile_qualified": False}
    (destination / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"directory": str(destination), **receipt}), flush=True)


if __name__ == "__main__":
    main()
