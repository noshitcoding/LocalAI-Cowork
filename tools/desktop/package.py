"""Build native desktop bundles from a checked release sidecar and frozen inputs.

This stages a new source context and publishes a new preparation directory.
It never installs or uploads the application. Signing requires an explicit profile.
"""
from __future__ import annotations

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import time
import tomllib

import release
import signing

ROOT = Path(__file__).resolve().parents[2]
CLI_VERSION = "2.11.4"
DESKTOP = "localai-cowork-v2-desktop"
SUPPORTED_BUNDLES = {"x86_64-pc-windows-msvc": {"nsis"},
                     "x86_64-unknown-linux-gnu": {"deb", "appimage"}}
Refused = release.Refused


def checked_nsis_contract(path: Path, version: str) -> dict:
    """Check the actual PE resources consumed by the native update observer."""
    if os.name != "nt":
        raise Refused("NSIS resource qualification requires the native Windows host")
    path = release.regular(path)
    before = pin(path)
    library = ctypes.WinDLL("version", use_last_error=True)
    library.GetFileVersionInfoSizeW.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_uint32)]
    library.GetFileVersionInfoSizeW.restype = ctypes.c_uint32
    library.GetFileVersionInfoW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    library.GetFileVersionInfoW.restype = ctypes.c_int
    library.VerQueryValueW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_uint32)]
    library.VerQueryValueW.restype = ctypes.c_int
    ignored = ctypes.c_uint32()
    size = library.GetFileVersionInfoSizeW(str(path), ctypes.byref(ignored))
    if not 1 <= size <= 1024 * 1024:
        raise Refused("NSIS version resource is missing or exceeds the bound")
    buffer = ctypes.create_string_buffer(size)
    if not library.GetFileVersionInfoW(str(path), 0, size, buffer):
        raise Refused("NSIS version resource cannot be read")
    begin = ctypes.addressof(buffer)

    def query(name: str, string: bool = False):
        pointer = ctypes.c_void_p()
        length = ctypes.c_uint32()
        if not library.VerQueryValueW(buffer, name, ctypes.byref(pointer), ctypes.byref(length)):
            raise Refused("NSIS installer contract resource is missing")
        byte_length = length.value * (2 if string else 1)
        if not pointer.value or not byte_length or pointer.value < begin or pointer.value + byte_length > begin + size:
            raise Refused("NSIS resource pointer or length is invalid")
        if string:
            if length.value > 1024 or ctypes.string_at(pointer.value + byte_length - 2, 2) != b"\0\0":
                raise Refused("NSIS contract string is invalid")
            return ctypes.wstring_at(pointer.value, length.value - 1)
        return ctypes.string_at(pointer.value, byte_length)

    translations = query(r"\VarFileInfo\Translation")
    if len(translations) % 4 or len(translations) > 256:
        raise Refused("NSIS version translations are invalid")
    tables = []
    expected = {"CoworkInstallerContract": "nsis.currentUser.originalProcess.v1", "ProductVersion": version}
    for offset in range(0, len(translations), 4):
        language, codepage = struct.unpack_from("<HH", translations, offset)
        table = f"{language:04x}{codepage:04x}"
        if table in tables:
            raise Refused("NSIS version translations are ambiguous")
        for name, value in expected.items():
            if query("\\StringFileInfo\\" + table + "\\" + name, True) != value:
                raise Refused("NSIS installer contract or product version does not match this release")
        tables.append(table)
    if pin(path) != before:
        raise Refused("NSIS installer bytes changed while checking its contract")
    return {"format": 1, "contract": expected["CoworkInstallerContract"],
            "product_version": version, "resource_tables": tables, **before}


def strict_json(raw: bytes) -> dict:
    def pairs(items):
        result = {}
        for name, value in items:
            if name in result:
                raise Refused("duplicate JSON fields are unsupported")
            result[name] = value
        return result
    try:
        value = json.loads(raw, object_pairs_hook=pairs)
    except (ValueError, UnicodeError) as error:
        raise Refused("invalid preparation JSON") from error
    if not isinstance(value, dict):
        raise Refused("a preparation JSON object is required")
    return value


def pin(path: Path, *, cargo_artifact: bool = False) -> dict:
    path = release.regular(path, cargo_artifact=cargo_artifact)
    return {"size": path.stat().st_size, "sha256": release.sha256(path)}


def directory(path: Path) -> Path:
    path = Path(os.path.abspath(path))
    for part in [*reversed(path.parents), path]:
        info = part.lstat()
        if part.is_symlink() or getattr(info, "st_file_attributes", 0) & 0x400:
            raise Refused("ordinary directories without followed aliases are required")
    if not path.is_dir():
        raise Refused("an existing directory is required")
    return path


def input_manifest(root: Path) -> dict:
    records = release.source_manifest(root)
    extras = [root / "package.json", root / "package-lock.json", root / "tools/desktop/package.py",
              root / "tools/desktop/signing.py",root / "tools/release/sbom.py",root / "tools/release/runtime_inventory.py",
              root / "tools/desktop/test_package.py",root / "packages/ui/src/secretMessages.json"]
    # Desktop setup embeds these exact helpers; include them in private staging.
    extras.extend(path for path in (root / "tools/toolbox").rglob("*")
                  if path.is_file() and path.suffix in {".py", ".ps1", ".lock", ".json", ".md"}
                  and "__pycache__" not in path.parts)
    for folder in ("apps/desktop/src-tauri", "apps/web/dist"):
        directory(root / folder)
        extras.extend(path for path in (root / folder).rglob("*") if path.is_file()
                      and not {"target", "binaries", "gen", "node_modules", ".git"}.intersection(path.relative_to(root).parts))
    # Test include_bytes inputs are pinned along with their Rust callers.
    extras.extend(path for path in (root / "crates").rglob("*.sqlite3") if "target" not in path.parts)
    for path in sorted(set(extras)):
        records[path.relative_to(root).as_posix()] = pin(path)
    if "apps/web/dist/index.html" not in records:
        raise Refused("the actual built frontend index is missing")
    return records


def checked_sidecar(root: Path, prepared: Path, host: str, sidecar_origin_root: Path | None = None) -> tuple[dict, Path, dict]:
    # A frozen source copy retains the original Cargo evidence unchanged. Its
    # relative source manifest must still match exactly; only the explicitly
    # reviewed original absolute build location differs from the packaging root.
    origin = directory(sidecar_origin_root) if sidecar_origin_root is not None else root
    prepared = directory(prepared)
    receipt_path = release.regular(prepared / "receipt.json")
    if receipt_path.stat().st_size > 4 * 1024 * 1024:
        raise Refused("sidecar receipt exceeds the preparation bound")
    receipt = strict_json(receipt_path.read_bytes())
    if (receipt.get("format") != "localai-cowork-native-release-sidecar/1"
            or receipt.get("host") != host or receipt.get("profile") != "release"
            or receipt.get("cargo_exit_code") != 0 or receipt.get("source_before_after_equal") is not True
            or receipt.get("signed") is not False or receipt.get("installed") is not False):
        raise Refused("the matching native release.py sidecar receipt is required")
    if receipt.get("sources") != release.source_manifest(root):
        raise Refused("release sidecar core inputs differ from the current source inputs")
    artifact = receipt.get("artifact", {})
    filename = "cowork-daemon-" + host + (".exe" if host.endswith("msvc") else "")
    if artifact.get("filename") != filename:
        raise Refused("sidecar filename is outside the native naming contract")
    sidecar = release.regular(prepared / filename)
    expected_pin = {key: artifact.get(key) for key in ("size", "sha256")}
    if pin(sidecar) != expected_pin or not 0 < expected_pin["size"] <= release.MAX_BINARY:
        raise Refused("release sidecar bytes differ from their preparation receipt")
    release.validate_executable(sidecar, host)
    cargo_json = release.regular(prepared / "cargo-build.jsonl")
    raw = cargo_json.read_bytes()
    if hashlib.sha256(raw).hexdigest() != receipt.get("cargo_json_sha256"):
        raise Refused("release sidecar Cargo evidence changed")
    items = cargo_messages(raw.decode("utf-8"))
    candidates = [item for item in items if item.get("reason") == "compiler-artifact"
                  and item.get("package_id") == receipt.get("package_id")
                  and item.get("target", {}).get("name") == "cowork-daemon"
                  and item.get("target", {}).get("kind") == ["bin"] and item.get("executable")]
    if len(candidates) != 1 or [item.get("success") for item in items if item.get("reason") == "build-finished"] != [True]:
        raise Refused("sidecar Cargo evidence has no unique successful daemon artifact")
    item = candidates[0]
    profile = item.get("profile", {})
    if (profile.get("opt_level") != "3" or profile.get("debug_assertions") is not False
            or profile.get("test") is not False or profile.get("overflow_checks") is not False
            or item["executable"] != receipt.get("cargo_reported_artifact")
            or Path(item.get("target", {}).get("src_path", "")) != origin / "services/daemon/src/main.rs"):
        raise Refused("sidecar Cargo evidence is not the exact release daemon build")
    current_version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if receipt.get("package_version") != current_version:
        raise Refused("sidecar package version differs from the workspace")
    # The original Cargo cache may have been retired. The immutable independent
    # artifact and original Cargo evidence, rather than that cache, are inputs.
    return receipt, sidecar, {"receipt.json": pin(receipt_path), "cargo-build.jsonl": pin(cargo_json), filename: expected_pin}


def checked_packaging_sources(root: Path) -> dict:
    """The frozen tree must contain the exact tools actually executing here."""
    tools = {
        "tools/desktop/package.py": Path(__file__),
        "tools/desktop/test_package.py": Path(__file__).with_name("test_package.py"),
        "tools/desktop/release.py": Path(release.__file__),
        "tools/desktop/signing.py": Path(signing.__file__),
    }
    records = {}
    for name, actual in tools.items():
        current = pin(actual)
        if pin(root / name) != current:
            raise Refused("frozen packaging helper differs from the executing source: " + name)
        records[name] = current
    return records


def checked_source_roots(root: Path, source_root: Path | None, sidecar_origin_root: Path | None) -> tuple[Path, Path, dict | None]:
    if (source_root is None) != (sidecar_origin_root is None):
        raise Refused("frozen source and original sidecar build roots must both be explicit")
    if source_root is None:
        root = directory(root)
        return root, root, None
    root = directory(source_root)
    origin = directory(sidecar_origin_root)
    return root, origin, checked_packaging_sources(root)


def checked_cli(root: Path, host: str) -> tuple[Path, dict]:
    package = strict_json((root / "package.json").read_bytes())
    lock = strict_json((root / "package-lock.json").read_bytes())
    if (package.get("devDependencies", {}).get("@tauri-apps/cli") != CLI_VERSION
            or lock.get("packages", {}).get("", {}).get("devDependencies", {}).get("@tauri-apps/cli") != CLI_VERSION):
        raise Refused("the workspace Tauri CLI must be pinned exactly")
    modules = root / "node_modules/@tauri-apps"
    native_name = "cli-win32-x64-msvc" if host.endswith("msvc") else "cli-linux-x64-gnu"
    paths = []
    for name in ("cli", native_name):
        folder = directory(modules / name)
        metadata = strict_json(release.regular(folder / "package.json").read_bytes())
        locked = lock.get("packages", {}).get("node_modules/@tauri-apps/" + name, {})
        if metadata.get("name") != "@tauri-apps/" + name or metadata.get("version") != CLI_VERSION or locked.get("version") != CLI_VERSION:
            raise Refused("installed native CLI metadata differs from the exact lock")
        if not isinstance(locked.get("integrity"), str) or not locked["integrity"].startswith("sha512-"):
            raise Refused("the native CLI lock lacks package-integrity provenance")
        paths.extend(path for path in folder.rglob("*") if path.is_file())
    records = {path.relative_to(root).as_posix(): pin(path) for path in sorted(paths)}
    entry = release.regular(modules / "cli/tauri.js")
    if not any(name.endswith(".node") for name in records if native_name in name):
        raise Refused("the native CLI addon is missing")
    return entry, records


def cargo_messages(text: str) -> list[dict]:
    try:
        return [strict_json(line.encode()) for line in text.splitlines() if line.startswith("{")]
    except ValueError as error:
        raise Refused("invalid Cargo JSON evidence") from error


def desktop_artifact(text: str, context: Path, target: Path, host: str) -> Path:
    items = cargo_messages(text)
    candidates = [item for item in items if item.get("reason") == "compiler-artifact"
                  and item.get("target", {}).get("name") == DESKTOP
                  and item.get("target", {}).get("kind") == ["bin"] and item.get("executable")]
    if [item.get("success") for item in items if item.get("reason") == "build-finished"] != [True] or len(candidates) != 1:
        raise Refused("exactly one successful Cargo-reported desktop artifact is required")
    item = candidates[0]; profile = item.get("profile", {})
    suffix = ".exe" if host.endswith("msvc") else ""
    expected = target / host / "release" / (DESKTOP + suffix)
    if (Path(item["executable"]) != expected
            or Path(item.get("target", {}).get("src_path", "")) != context / "apps/desktop/src-tauri/src/main.rs"
            or profile.get("opt_level") != "3" or profile.get("debug_assertions") is not False
            or profile.get("test") is not False or profile.get("overflow_checks") is not False):
        raise Refused("the desktop artifact is outside the exact native release profile")
    artifact = release.regular(expected, cargo_artifact=True)
    if not 0 < artifact.stat().st_size <= release.MAX_BINARY:
        raise Refused("desktop executable size is unsupported")
    release.validate_executable(artifact, host)
    return artifact


def validate_bundle(path: Path, kind: str, host: str) -> None:
    release.regular(path)
    if not 1024 <= path.stat().st_size <= 1024 * 1024 * 1024:
        raise Refused("bundle size is unsupported")
    with path.open("rb") as stream:
        header = stream.read(64)
        if kind == "nsis" and host.endswith("msvc"):
            # NSIS commonly uses an x86 installer stub for an x64 payload.
            if header[:2] != b"MZ": raise Refused("NSIS output is not a PE installer")
            offset = struct.unpack_from("<I", header, 60)[0]
            if not 64 <= offset <= min(path.stat().st_size - 26, 1024 * 1024): raise Refused("invalid NSIS PE header")
            stream.seek(offset); pe = stream.read(26)
            machine, magic, flags = struct.unpack_from("<H", pe, 4)[0], struct.unpack_from("<H", pe, 24)[0], struct.unpack_from("<H", pe, 22)[0]
            if pe[:4] != b"PE\0\0" or (machine, magic) not in ((0x14c, 0x10b), (0x8664, 0x20b)) or not flags & 2 or flags & 0x2000:
                raise Refused("NSIS output is not an executable native installer stub")
        elif kind == "appimage" and host.endswith("gnu"):
            release.validate_executable(path, host)
            if header[8:11] != b"AI\x02": raise Refused("the ELF output is not a type-2 AppImage")
        elif kind == "deb" and host.endswith("gnu"):
            if header[:8] != b"!<arch>\n": raise Refused("the output is not a Debian package")
            stream.seek(8); names = []
            while stream.tell() < path.stat().st_size:
                item = stream.read(60)
                if len(item) != 60 or item[58:60] != b"`\n": raise Refused("invalid Debian archive header")
                name = item[:16].decode("ascii").strip().rstrip("/")
                try: size = int(item[48:58].decode("ascii").strip())
                except ValueError as error: raise Refused("invalid Debian archive size") from error
                if size < 0 or stream.tell() + size > path.stat().st_size: raise Refused("invalid Debian archive extent")
                if name == "debian-binary" and stream.read(size) != b"2.0\n": raise Refused("unsupported Debian package format")
                elif name != "debian-binary": stream.seek(size, 1)
                if size & 1: stream.seek(1, 1)
                names.append(name)
            if len(names) != 3 or names[0] != "debian-binary" or not names[1].startswith("control.tar.") or not names[2].startswith("data.tar."):
                raise Refused("the Debian package members are unsupported")
        else: raise Refused("unsupported bundle/native-host combination")


def collect_bundles(base: Path, kinds: list[str], host: str) -> dict[str, Path]:
    result = {}
    extensions = {"nsis": ".exe", "deb": ".deb", "appimage": ".AppImage"}
    for kind in kinds:
        folder = directory(base / kind)
        candidates = [path for path in folder.iterdir() if path.is_file() and path.name.endswith(extensions[kind])]
        if len(candidates) != 1: raise Refused("a fresh bundle directory must contain one output per selected kind")
        validate_bundle(candidates[0], kind, host); result[kind] = candidates[0]
    return result


def publish(staged: Path, output: Path) -> None:
    if os.name == "nt":
        staged.rename(output)  # Windows refuses any existing destination.
    else:
        libc = ctypes.CDLL(None, use_errno=True)
        rename = libc.renameat2; rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]; rename.restype = ctypes.c_int
        if rename(-100, os.fsencode(staged), -100, os.fsencode(output), 1) != 0:
            raise OSError(ctypes.get_errno(), "fresh bundle publication refused")


class ProcessScope:
    """Native build commands only. Linux never places wsl.exe in a kill Job."""
    def __init__(self):
        self.records = []; self.job = None; self.closed_receipt = None
        if os.name == "nt":
            from ctypes import wintypes as w
            self.w = w; self.k = ctypes.WinDLL("kernel32", use_last_error=True)
            def bind(name, args, result=w.BOOL):
                fn = getattr(self.k, name); fn.argtypes = args; fn.restype = result; return fn
            bind("CreateJobObjectW", [ctypes.c_void_p, w.LPCWSTR], w.HANDLE)
            bind("SetInformationJobObject", [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD])
            bind("AssignProcessToJobObject", [w.HANDLE, w.HANDLE])
            bind("QueryInformationJobObject", [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD, ctypes.c_void_p])
            bind("GetExitCodeProcess", [w.HANDLE, ctypes.POINTER(w.DWORD)])
            bind("CloseHandle", [w.HANDLE]); bind("TerminateJobObject", [w.HANDLE, w.UINT])
            class Basic(ctypes.Structure):
                _fields_ = [("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64), ("flags", w.DWORD),
                            ("min_ws", ctypes.c_size_t), ("max_ws", ctypes.c_size_t), ("process_limit", w.DWORD),
                            ("affinity", ctypes.c_size_t), ("priority", w.DWORD), ("scheduling", w.DWORD)]
            class Extended(ctypes.Structure):
                _fields_ = [("basic", Basic), ("io", ctypes.c_uint64 * 6), ("memory", ctypes.c_size_t * 4)]
            self.job = self.k.CreateJobObjectW(None, None)
            if not self.job: raise ctypes.WinError(ctypes.get_last_error())
            limits = Extended(); limits.basic.flags = 0x2000
            if not self.k.SetInformationJobObject(self.job, 9, ctypes.byref(limits), ctypes.sizeof(limits)): raise ctypes.WinError(ctypes.get_last_error())

    def run(self, command: list[str], cwd: Path, env: dict, stdout: Path, stderr: Path) -> str:
        with stdout.open("wb") as out, stderr.open("wb") as err:
            if os.name == "nt":
                # The small child cannot start the command before Job assignment.
                gate = "import sys,subprocess;assert sys.stdin.buffer.readline()==b'go\\n';p=subprocess.Popen(sys.argv[1:],stdin=subprocess.DEVNULL,stdout=sys.stdout.buffer,stderr=sys.stderr.buffer,creationflags=subprocess.CREATE_NO_WINDOW);sys.exit(p.wait())"
                import sys
                process = subprocess.Popen([sys.executable, "-I", "-B", "-S", "-c", gate, *command], cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=out, stderr=err, creationflags=subprocess.CREATE_NO_WINDOW)
                if not self.k.AssignProcessToJobObject(self.job, self.w.HANDLE(int(process._handle))):
                    process.terminate(); process.wait(); raise ctypes.WinError(ctypes.get_last_error())
                process.stdin.write(b"go\n"); process.stdin.close(); fd = None
            else:
                process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err, start_new_session=True)
                fd = os.pidfd_open(process.pid)
            try:
                code = process.wait()
            except BaseException:
                if os.name != "nt":
                    self.stop_linux_session(process.pid)
                    process.wait(timeout=20)
                    poll = select.poll(); poll.register(fd, select.POLLIN)
                    done = bool(poll.poll(0)); os.close(fd); fd = None
                    if not done: raise Refused("own native Linux cleanup lacks positive PIDfd completion")
                else: self.k.TerminateJobObject(self.job, 125); process.wait(timeout=20)
                raise
            finally:
                if fd is not None:
                    poll = select.poll(); poll.register(fd, select.POLLIN)
                    done = bool(poll.poll(0))
                    if not done: raise Refused("native command lacks a positive PIDfd completion")
            record = {"command": command, "exit_code": code, "retained_popen_wait": True}
            try:
                if os.name == "nt":
                    native = self.w.DWORD()
                    if not self.k.GetExitCodeProcess(self.w.HANDLE(int(process._handle)), ctypes.byref(native)) or native.value != code & 0xffffffff:
                        raise Refused("native Windows process completion disagrees")
                    record["native_get_exit_confirmed"] = True
                else:
                    if self.linux_session_members(process.pid):
                        self.stop_linux_session(process.pid)
                        raise Refused("an own native Linux session required descendant cleanup")
                    record.update(retained_pidfd_completion=True, own_session_empty=True)
            finally:
                if fd is not None: os.close(fd)
            self.records.append(record)
            if code != 0: raise subprocess.CalledProcessError(code, command)
        if stdout.stat().st_size > 64 * 1024 * 1024: raise Refused("command evidence exceeds the preparation bound")
        return stdout.read_text(encoding="utf-8")

    @staticmethod
    def linux_session_members(session: int) -> dict[int, dict]:
        members = {}
        for path in Path("/proc").iterdir():
            if path.name.isdecimal():
                try:
                    raw = (path / "stat").read_text(); fields = raw[raw.rfind(")") + 2:].split()
                    if int(fields[3]) == session and fields[0] != "Z":
                        members[int(path.name)] = {"session":session,"start_ticks":int(fields[19]),"uid":path.stat().st_uid}
                except (OSError, ValueError, IndexError): pass
        return members

    @classmethod
    def stop_linux_session(cls, session: int) -> None:
        # Signal only retained native PIDfds rechecked against this own session.
        # Neither the Windows launcher nor the shared WSL host is a member.
        deadline = time.monotonic() + 20
        while cls.linux_session_members(session):
            for pid, birth in cls.linux_session_members(session).items():
                if birth["uid"] != os.geteuid(): raise Refused("an own native session contains a foreign user")
                try: fd = os.pidfd_open(pid)
                except ProcessLookupError: continue
                try:
                    if cls.linux_session_members(session).get(pid) == birth:
                        signal.pidfd_send_signal(fd, signal.SIGTERM if time.monotonic() < deadline - 15 else signal.SIGKILL)
                        waiter = select.poll(); waiter.register(fd, select.POLLIN); waiter.poll(50)
                except ProcessLookupError: pass
                finally: os.close(fd)
            if time.monotonic() >= deadline: raise Refused("own native Linux session cleanup did not acknowledge completion")
            time.sleep(.05)

    def close(self, success: bool) -> dict:
        if self.closed_receipt is not None: return self.closed_receipt
        if os.name != "nt":
            self.closed_receipt = {"separate_native_linux_sessions": True, "windows_launcher_kill_job_used": False}
            return self.closed_receipt
        value = ctypes.create_string_buffer(48)
        deadline = time.monotonic() + 60
        while True:
            if not self.k.QueryInformationJobObject(self.job, 1, value, len(value), None): raise ctypes.WinError(ctypes.get_last_error())
            active = struct.unpack_from("<I", value.raw, 40)[0]
            if not active or not success or time.monotonic() >= deadline: break
            time.sleep(.05)
        forced = bool(active)
        if forced:
            if not self.k.TerminateJobObject(self.job, 125): raise ctypes.WinError(ctypes.get_last_error())
            deadline = time.monotonic() + 20
            while active:
                if not self.k.QueryInformationJobObject(self.job, 1, value, len(value), None): raise ctypes.WinError(ctypes.get_last_error())
                active = struct.unpack_from("<I", value.raw, 40)[0]
                if active and time.monotonic() >= deadline: raise Refused("own native build cleanup did not acknowledge process exit")
                if active: time.sleep(.05)
        self.k.CloseHandle(self.job); self.job = None
        self.closed_receipt = {"own_native_windows_job": True, "active_after_wait": active, "own_job_empty": active == 0, "forced_cleanup": forced}
        if forced and success: raise Refused("own Windows build descendants did not complete naturally")
        return self.closed_receipt


def source_context() -> Path:
    context=directory(Path(tempfile.mkdtemp(prefix="cowork-desktop-package-context-")))
    if any((parent / "Cargo.toml").exists() for parent in context.parents):
        raise Refused("the private source context has an outer Cargo workspace")
    return context


def checked_bundle_copy(original: Path, copied: Path, bundles: list[str], signed_windows:bool=False) -> dict:
    before = release.regular(original, cargo_artifact=True).read_bytes()
    after = release.regular(copied).read_bytes()
    if signed_windows:
        before=signing.normalized_pe(before);after=signing.normalized_pe(after)
        if len(after)<len(before) or len(after)-len(before)>7 or any(after[len(before):]):
            raise Refused("signed executable changed beyond its certificate table and alignment")
        after=after[:len(before)]
    if before == after:
        return {**pin(copied), "marker": "unchanged", "other_bytes_unchanged": True}
    # Pinned Tauri CLI patches a temporary or bundle-input copy. The original
    # Cargo executable must keep its exact bytes. Permit only this single,
    # documented three-byte metadata replacement for a selected bundle kind.
    marker = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
    if before.count(marker) != 1:
        raise Refused("the native artifact has no unique Tauri bundle marker")
    for kind in bundles:
        code = {"nsis": b"NSS", "deb": b"DEB", "appimage": b"APP"}[kind]
        if after == before.replace(marker, b"__TAURI_BUNDLE_TYPE_VAR_" + code, 1):
            return {**pin(copied), "marker": code.decode("ascii"), "other_bytes_unchanged": True}
    raise Refused("the bundle copy changed beyond its selected Tauri metadata marker")


def prepare(root: Path, sidecar_dir: Path, output: Path, target_dir: Path, bundles: list[str],
            offline: bool = False, node: Path | None = None, fast: bool = False, signing_profile:Path|None=None,
            source_root:Path|None=None, sidecar_origin_root:Path|None=None) -> dict:
    root, origin, packaging_tools = checked_source_roots(root, source_root, sidecar_origin_root)
    output = Path(os.path.abspath(output)); target_dir = Path(os.path.abspath(target_dir))
    if output.exists() or output.is_symlink(): raise Refused("the immutable package output must be new")
    directory(output.parent)
    if len(set(bundles)) != len(bundles) or not bundles: raise Refused("select each bundle kind exactly once")
    if any(os.environ.get(name) for name in ("RUSTFLAGS", "RUSTDOCFLAGS", "RUST_MIN_STACK", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS", "TAURI_CONFIG", "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "NAPI_RS_NATIVE_LIBRARY_PATH", "NAPI_RS_FORCE_WASI", "NODE_OPTIONS", "NODE_PATH")):
        raise Refused("unqualified compiler or Tauri configuration overrides are unsupported")
    if any(name.startswith("CARGO_PROFILE_RELEASE_") for name in os.environ): raise Refused("release profile overrides are unsupported")
    env = {key: value for key, value in os.environ.items() if not key.startswith("COWORK_")}
    env.update(CARGO_BUILD_JOBS=str(min(os.cpu_count() or 1, 4) if fast else 1), CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=str(target_dir), CI="true")
    if offline: env["CARGO_NET_OFFLINE"] = "true"
    before = input_manifest(root)
    signing_configuration,signing_environment,signing_receipt=signing.profile(signing_profile) if signing_profile else (None,{},None)
    # An excluded standalone Cargo package must have no outer workspace above
    # this complete source context. Keep publication on the output volume.
    context=source_context()
    scope = ProcessScope(); succeeded = False
    report = {"format": "localai-cowork-native-desktop-package/1", "created_at": datetime.now(timezone.utc).isoformat(),
              "result": "running", "source_context": str(context), "sources": before,
              "source_root": str(root), "sidecar_origin_root": str(origin),
              "frozen_source_selected": source_root is not None, "executing_packaging_tools": packaging_tools,
              "signed": False, "installed": False, "published": False, "release_qualified": False}
    try:
        def run(command, label, cwd=root, selected_env=env):
            return scope.run(command, cwd, selected_env, context / (label + ".stdout.log"), context / (label + ".stderr.log"))
        rustc = run(["rustc", "-vV"], "rustc")
        host = release.native_host(rustc)
        if not set(bundles) <= SUPPORTED_BUNDLES[host]: raise Refused("bundle kinds do not match the actual native host")
        if signing_profile and host.endswith('msvc') and not signing_receipt['windows_certificate_thumbprint']:
            raise Refused('Windows release signing requires the actual Authenticode certificate')
        receipt, sidecar, sidecar_pins = checked_sidecar(root, sidecar_dir, host, origin)
        cli, cli_pins = checked_cli(root, host)
        resolved_node = node or Path(shutil.which("node") or "")
        resolved_node = release.regular(resolved_node); node_pin = pin(resolved_node, cargo_artifact=True)
        version = run([str(resolved_node), str(cli), "--version"], "tauri-version").strip()
        if version != "tauri-cli " + CLI_VERSION: raise Refused("the executed native Tauri CLI version differs from the lock")
        for name, expected in before.items():
            if pin(root / name) != expected: raise Refused("input changed during private staging")
            staged = context / "source" / name; staged.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / name, staged)
            if pin(staged) != expected: raise Refused("private source staging changed input bytes")
        staged_root = context / "source"
        tauri = staged_root / "apps/desktop/src-tauri"
        configuration = strict_json((tauri / "tauri.conf.json").read_bytes())
        config_build = configuration.get("build", {}); config_bundle = configuration.get("bundle", {})
        if (config_build.get("frontendDist") != "../../web/dist" or config_bundle.get("externalBin") != ["binaries/cowork-daemon", "binaries/cowork-browser-host"]
                or config_bundle.get("active") is not True or config_build.get("beforeBuildCommand") or config_build.get("beforeBundleCommand")
                or config_bundle.get("createUpdaterArtifacts") or config_bundle.get("windows", {}).get("signCommand")
                or any(tauri.glob("tauri.*.conf.*"))):
            raise Refused("desktop configuration needs a separately reviewed packaging profile")
        staged_sidecar = tauri / "binaries" / receipt["artifact"]["filename"]
        staged_sidecar.parent.mkdir(); shutil.copyfile(sidecar, staged_sidecar); shutil.copymode(sidecar, staged_sidecar)
        if pin(staged_sidecar) != sidecar_pins[staged_sidecar.name]: raise Refused("sidecar staging changed bytes")
        browser_build = run(["cargo", "build", "--locked", "--release", "-p", "cowork-browser-host", "--target", host,
                             "--target-dir", str(context / "browser-target"), "--message-format=json-render-diagnostics"], "browser-host-build", staged_root)
        browser_records = [json.loads(line) for line in browser_build.splitlines() if line.startswith("{")]
        browser_candidates = [Path(item["executable"]) for item in browser_records if item.get("reason")=="compiler-artifact" and item.get("target",{}).get("name")=="cowork-browser-host" and item.get("executable")]
        if len(browser_candidates)!=1: raise Refused("exactly one browser host artifact is required")
        browser_artifact = release.regular(browser_candidates[0], cargo_artifact=True)
        browser_expected = context / "browser-target" / host / "release" / ("cowork-browser-host.exe" if host.endswith("msvc") else "cowork-browser-host")
        browser_record=next(item for item in browser_records if item.get("executable")==str(browser_artifact) and item.get("target",{}).get("name")=="cowork-browser-host")
        browser_profile=browser_record.get("profile",{})
        if (browser_artifact!=browser_expected or Path(browser_record.get("target",{}).get("src_path",""))!=staged_root/"services/browser-host/src/main.rs"
                or browser_profile.get("opt_level")!="3" or browser_profile.get("debug_assertions") is not False or browser_profile.get("test") is not False):
            raise Refused("browser host must be the exact current native release artifact")
        release.validate_executable(browser_artifact, host)
        browser_host_pin = pin(browser_artifact, cargo_artifact=True)
        browser_staged = tauri / "binaries" / ("cowork-browser-host-"+host+(".exe" if host.endswith("msvc") else ""))
        shutil.copyfile(browser_artifact,browser_staged); shutil.copymode(browser_artifact,browser_staged)
        if pin(browser_staged)!=browser_host_pin: raise Refused("browser host staging changed bytes")
        manifest = str(tauri / "Cargo.toml")
        signing_arguments=[]
        if signing_configuration:
            override=tauri/'release-signing.json';override.write_text(json.dumps(signing_configuration),encoding='utf-8')
            signing_arguments=['--config',str(override)]
        if not fast:
            run(["cargo", "fmt", "--manifest-path", manifest, "--check"], "desktop-format", tauri)
            for action in ("test", "clippy"):
                gate = ["cargo", action, "--release", "--locked", "--manifest-path", manifest, "--target", host]
                if offline: gate.append("--offline")
                gate.extend(["--", "--test-threads=1"] if action == "test" else ["--", "-D", "warnings"])
                run(gate, "desktop-" + action, tauri)
        build = [str(resolved_node), str(cli), "build", "--no-bundle", "--ci", *signing_arguments, *([] if signing_profile else ['--no-sign']), "--target", host,
                 "--", "--locked", "--message-format=json-diagnostic-short"]
        if offline: build.append("--offline")
        evidence = run(build, "desktop-build", tauri,dict(env,**signing_environment))
        artifact = desktop_artifact(evidence, staged_root, target_dir, host)
        desktop_pin = pin(artifact, cargo_artifact=True)
        bundle_target = context / "bundle-input"
        release_dir = bundle_target / host / "release"; release_dir.mkdir(parents=True)
        bundled_desktop = release_dir / artifact.name; shutil.copyfile(artifact, bundled_desktop); shutil.copymode(artifact, bundled_desktop)
        bundled_sidecar = release_dir / ("cowork-daemon.exe" if host.endswith("msvc") else "cowork-daemon")
        shutil.copyfile(sidecar, bundled_sidecar); shutil.copymode(sidecar, bundled_sidecar)
        if pin(bundled_desktop) != desktop_pin or pin(bundled_sidecar) != sidecar_pins[sidecar.name]: raise Refused("bundle inputs changed bytes")
        bundle_env = dict(env, **signing_environment, CARGO_TARGET_DIR=str(bundle_target))
        bundle_command = [str(resolved_node), str(cli), "bundle", "--ci", *signing_arguments, *([] if signing_profile else ['--no-sign']), "--target", host, "--bundles", ",".join(bundles)]
        run(bundle_command, "desktop-bundle", tauri, bundle_env)
        found = collect_bundles(release_dir / "bundle", bundles, host)
        nsis_contract = checked_nsis_contract(found['nsis'], configuration['version']) if 'nsis' in found else None
        bundle_copy = checked_bundle_copy(artifact, bundled_desktop, bundles,bool(signing_profile and host.endswith('msvc')))
        if signing_profile and host.endswith('msvc'):
            verifier=context/'verify-authenticode.ps1'
            verifier.write_text('param([string]$Artifact,[string]$Thumbprint)\n$ErrorActionPreference="Stop"\n$sig=Get-AuthenticodeSignature -LiteralPath $Artifact\nif($sig.Status -ne "Valid" -or $sig.SignerCertificate.Thumbprint -ne $Thumbprint){throw "Authenticode verification failed"}\nWrite-Output "Verified"\n',encoding='utf-8')
            for number,path in enumerate([bundled_desktop,*found.values()]):
                run(['powershell','-NoProfile','-NonInteractive','-File',str(verifier),'-Artifact',str(path),
                     '-Thumbprint',signing_receipt['windows_certificate_thumbprint']],f'authenticode-{number}',tauri)
        if (input_manifest(root) != before or checked_cli(root, host)[1] != cli_pins
                or checked_sidecar(root, sidecar_dir, host, origin)[2] != sidecar_pins
                or packaging_tools is not None and checked_packaging_sources(root) != packaging_tools
                or pin(resolved_node, cargo_artifact=True) != node_pin or pin(artifact, cargo_artifact=True) != desktop_pin
                or pin(bundled_desktop) != {key:bundle_copy[key] for key in ("size", "sha256")}
                or pin(bundled_sidecar) != sidecar_pins[sidecar.name]):
            raise Refused("source, CLI, node, sidecar or native artifact drifted during packaging")
        for name, expected in before.items():
            if pin(staged_root / name) != expected: raise Refused("Tauri unexpectedly changed staged source inputs")
        if pin(browser_staged)!=browser_host_pin: raise Refused("browser host drifted during packaging")
        native_scope = scope.close(True); succeeded = True
        staged_output = Path(tempfile.mkdtemp(prefix=".desktop-package-publication-", dir=output.parent))
        report["publication_staging"] = str(staged_output)
        artifacts = {}
        for kind, path in found.items():
            expected = pin(path); target = staged_output / path.name
            shutil.copyfile(path, target)
            if pin(path) != expected or pin(target) != expected: raise Refused("bundle changed during immutable publication")
            artifacts[kind] = {"filename": path.name, **expected}
            if signing_profile and kind in ('nsis','appimage'):
                signature=release.regular(path.with_suffix(path.suffix+'.sig'))
                signature_target=staged_output/signature.name;shutil.copyfile(signature,signature_target)
                artifacts[kind]['updater_signature']={'filename':signature.name,**pin(signature_target)}
        import importlib.util
        specification=importlib.util.spec_from_file_location('cowork_release_sbom',root/'tools/release/sbom.py')
        sbom_module=importlib.util.module_from_spec(specification);specification.loader.exec_module(sbom_module)
        sbom=sbom_module.generate(root,[staged_output/item['filename'] for item in artifacts.values()],
                                  payloads=[artifact,staged_sidecar,browser_staged])
        (staged_output/'sbom.spdx.json').write_text(json.dumps(sbom,indent=2,sort_keys=True),encoding='utf-8')
        report.update(result="passed", host=host, bundles=artifacts, sidecar_preparation=sidecar_pins,
                      desktop_artifact=desktop_pin, browser_host=browser_host_pin, bundle_input_desktop=bundle_copy,
                      cli_version=CLI_VERSION, cli_inputs=cli_pins, node=node_pin,
                      rustc=rustc, commands=scope.records, native_process_scope=native_scope,
                      source_before_after_equal=True, staged_source_bytes_equal=True,
                      desktop_format_test_clippy_exit_codes=None if fast else [0,0,0],
                      validation_scope="build_only" if fast else "format_tests_clippy_build",
                      native_payload_architecture="x86_64", installer_payload_extraction_qualified=False)
        report['native_installer_contract'] = nsis_contract
        report.update(signed=bool(signing_profile),signing_profile=signing_receipt,
                      product_version=configuration['version'],
                      sbom={'filename':'sbom.spdx.json',**pin(staged_output/'sbom.spdx.json')})
        (staged_output / "receipt.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        publish(staged_output, output)
        return report
    except BaseException as error:
        report.update(result="failed", failure=type(error).__name__ + ": " + str(error))
        raise
    finally:
        if not succeeded: report["native_process_scope"] = scope.close(False)
        report["commands"] = scope.records
        (context / "attempt.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        # This new context is evidence, including failures; no old cache or
        # source directory is recursively deleted or overwritten here.


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sidecar-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--bundles", required=True, help="nsis or deb,appimage on their native host")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--node", type=Path)
    parser.add_argument("--fast", action="store_true", help="build and package without tests/clippy; receipt records build-only validation")
    parser.add_argument('--signing-profile',type=Path,help='Explicit private release profile with actual updater key and Windows certificate')
    parser.add_argument('--source-root',type=Path,help='Explicit ordinary frozen source tree; requires --sidecar-origin-root')
    parser.add_argument('--sidecar-origin-root',type=Path,help='Original absolute Cargo source root recorded by the unchanged sidecar evidence')
    args = parser.parse_args()
    receipt = prepare(ROOT, args.sidecar_dir, args.output_dir, args.target_dir, args.bundles.split(","), args.offline, args.node, args.fast,args.signing_profile,args.source_root,args.sidecar_origin_root)
    print(json.dumps({"output_directory": str(args.output_dir.absolute()), "bundles": receipt["bundles"], "installed": False, "signed":receipt['signed']}, indent=2))


if __name__ == "__main__":
    main()
