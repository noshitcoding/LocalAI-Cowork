"""Independent Python USTAR vectors; synthetic bytes, no runtime qualification."""
import io
import json
from pathlib import Path
import tarfile


def archive(entries):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w", format=tarfile.USTAR_FORMAT) as target:
        for name, kind, payload in entries:
            entry = tarfile.TarInfo(name)
            entry.uid = entry.gid = 0
            entry.mtime = 0
            entry.mode = 0o755 if kind == tarfile.DIRTYPE else 0o644
            entry.type = kind
            if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                entry.linkname = payload
            elif kind == tarfile.REGTYPE:
                entry.size = len(payload)
            target.addfile(entry, io.BytesIO(payload) if kind == tarfile.REGTYPE else None)
    return out.getvalue()


def main():
    directory = ("bin", tarfile.DIRTYPE, b"")
    file = ("bin/hello", tarfile.REGTYPE, b"signed archive fixture\n")
    valid = archive([directory, file, ("hello", tarfile.SYMTYPE, "bin/hello")])
    cases = {"valid": (valid, None),
             "traversal": (archive([("../outside", tarfile.REGTYPE, b"bad")]), "unsafe_path"),
             "absolute": (archive([("/outside", tarfile.REGTYPE, b"bad")]), "unsafe_path"),
             "drive": (archive([("C:/outside", tarfile.REGTYPE, b"bad")]), "unsafe_path"),
             "duplicate": (archive([directory, file, file]), "duplicate_path"),
             "missing-parent": (archive([file]), "parent_not_directory"),
             "symlink-parent": (archive([("bin", tarfile.SYMTYPE, "/tmp"), file]), "parent_not_directory"),
             "link-escape": (archive([("escape", tarfile.SYMTYPE, "../../outside")]), "link_escape"),
             "hardlink": (archive([("link", tarfile.LNKTYPE, "target")]), "unsupported_member"),
             "device": (archive([("device", tarfile.CHRTYPE, b"")]), "unsupported_member"),
             "fifo": (archive([("fifo", tarfile.FIFOTYPE, b"")]), "unsupported_member"),
             "pax": (archive([("extended", tarfile.XHDTYPE, b"")]), "unsupported_member")}
    damaged = bytearray(valid)
    damaged[0] ^= 1
    cases["checksum"] = (bytes(damaged), "header_checksum")
    trailer = bytearray(valid)
    trailer[-1] = 1
    cases["trailer"] = (bytes(trailer), "trailing_data")
    output = {"scope": __doc__, "cases": [{"name": name, "hex": data.hex(), "error": error}
                                           for name, (data, error) in cases.items()]}
    deep_entries = [("/".join(["d"] * level), tarfile.DIRTYPE, b"") for level in range(1, 41)]
    deep_path = "/".join(["d"] * 40) + "/leaf.txt"
    deep = archive(deep_entries + [(deep_path, tarfile.REGTYPE, b"deep valid archive\n")])
    output["deep_valid"] = {"hex": deep.hex(), "directories": 40, "entries": 41, "file": deep_path}
    roles = ("base_rootfs", "browser_tools", "containerd_bundle", "document_tools", "git_tools",
             "gvisor_bundle", "node_tools", "python_tools", "runtime_service")
    output["installation_roles"] = [{"role": role, "file": "bin/" + role,
        "body": "tool role: " + role + "\n", "hex": archive([
            ("bin", tarfile.DIRTYPE, b""),
            ("bin/" + role, tarfile.REGTYPE, ("tool role: " + role + "\n").encode()),
            ("entry", tarfile.SYMTYPE, "bin/" + role)]).hex()} for role in roles]
    large = tarfile.TarInfo("bin/runtime_service")
    large.size = 9 * 1024**2
    large.mode = 0o644
    output["large_file_header_hex"] = large.tobuf(format=tarfile.USTAR_FORMAT).hex()
    root = Path(__file__).resolve().parents[2]
    (root / "crates/managed-runtime/tests/fixtures/python-ustar.json").write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"vectors": len(cases), "format": "Python tarfile USTAR_FORMAT"}))


if __name__ == "__main__":
    main()
