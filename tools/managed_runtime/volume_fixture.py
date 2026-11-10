"""Privileged, bounded Linux fixture for the actual native volume implementation.

Creates only its own bounded outer ext4 image, nested test volumes and loop devices.
Actual-task suite reserves 768 MiB for eleven retained jobs plus negative package
copies and filesystem metadata; other suites retain their 512-MiB fixture.
No global WSL/Docker/cgroup settings, foreign processes or recursive deletion.
"""
import argparse
from contextlib import closing
import ctypes
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import struct
import subprocess
import sqlite3
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--expected-test-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--service-binary",type=Path)
    parser.add_argument("--missing-once", action="store_true", help="One native readiness/Begin/Missing observation; no chunks/start or Controller")
    parser.add_argument("--service-sha256")
    parser.add_argument("--controller-binary",type=Path)
    parser.add_argument("--controller-sha256")
    parser.add_argument("--controller-case",choices=("copy","cancel","stop_read","delivery"),default="copy")
    parser.add_argument("--reject-output",action="store_true",help="Actual backend rejects unsupported output after preserving completion")
    parser.add_argument("--backend-only",action="store_true",help="One actual native backend copy/result path; no app qualification")
    parser.add_argument("--input-only", action="store_true", help="Native input boundary only; no workload starts")
    parser.add_argument("--suite", choices=("volumes", "task-components", "actual-tasks", "receiver-claim"), default="volumes")
    parser.add_argument("--gvisor-directory", type=Path)
    parser.add_argument("--gvisor-manifest", type=Path)
    parser.add_argument("--gvisor-manifest-sha256")
    for label in ("fence", "driver", "reaper", "containerd", "probe", "pause"):
        parser.add_argument("--"+label+"-binary", type=Path)
        parser.add_argument("--"+label+"-sha256")
    args = parser.parse_args()
    if args.missing_once and (not args.service_binary or args.suite != "actual-tasks" or args.controller_binary or args.controller_sha256 or args.controller_case != "copy" or args.reject_output or args.backend_only or args.input_only):
        raise ValueError("Missing-once requires only its actual service fixture, with no Controller/guest case")
    if bool(args.controller_binary)!=bool(args.controller_sha256) or (args.controller_binary and not args.service_binary): raise ValueError("Exact controller callback requires the actual service fixture")
    if args.controller_case!="copy" and not args.controller_binary: raise ValueError("Cancellation requires the actual Controller fixture")
    if bool(args.service_binary)!=bool(args.service_sha256): raise ValueError("Exact service ELF and hash required together")
    if args.service_binary and (args.suite!="actual-tasks" or args.input_only or args.backend_only): raise ValueError("Service requires its own actual-tasks fixture")
    if args.reject_output and not args.backend_only: raise ValueError("Rejected output requires backend-only")
    if args.backend_only and (args.input_only or args.suite!="actual-tasks"): raise ValueError("Backend-only requires actual-tasks and excludes input-only")
    if args.input_only and args.suite!="actual-tasks": raise ValueError("Input-only requires actual-tasks fixtures")
    if os.geteuid() != 0:
        raise ValueError("Use root only for this explicitly private volume fixture")
    # Every fixture owns its mount namespace, including volume mounts made by
    # its child test process. Concurrent own fixtures cannot alter each other's
    # parent mount observations or publish mounts into the user's WSL surface.
    namespace_before=os.readlink("/proc/thread-self/ns/mnt")
    native=ctypes.CDLL(None,use_errno=True)
    native.mount.argtypes=[ctypes.c_char_p,ctypes.c_char_p,ctypes.c_char_p,ctypes.c_ulong,ctypes.c_void_p]
    if native.unshare(0x00020000)!=0 or native.mount(None,b"/",None,16384|262144,None)!=0:
        raise OSError(ctypes.get_errno(),"Cannot isolate own fixture mounts")
    namespace_after=os.readlink("/proc/thread-self/ns/mnt")
    if namespace_after==namespace_before:raise RuntimeError("Own fixture namespace did not change")
    repo = Path(__file__).resolve().parents[2]
    output = args.output.resolve(strict=True)
    if not output.is_relative_to(repo / ".local/v2-rebuild-validation"):
        raise ValueError("Select a fresh output within the project qualification directory")
    binary = args.test_binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("Select the actual native test executable")
    with binary.open("rb") as source:
        binary_bytes=source.read(128*1024*1024+1)
    binary_sha=hashlib.sha256(binary_bytes).hexdigest()
    if len(binary_bytes)>128*1024*1024 or not binary_bytes.startswith(b"\x7fELF") or binary_sha!=args.expected_test_sha256:
        raise ValueError("Native test executable checksum changed")
    formatter_digest=hashlib.sha256(Path("/usr/sbin/mke2fs").resolve().read_bytes()).hexdigest()
    root = Path("/var/tmp") / ("cowork-runtime-volumes-" + uuid.uuid4().hex)
    root.mkdir(mode=0o700)
    native_root = (root.stat().st_dev, root.stat().st_ino)
    image = root / "fixture.ext4"
    mountpoint = root / "case-storage"
    loop = None
    root_image_id = None
    private_binary=root/"volume-tests"
    binary_id=None
    tools_directory=None
    tools_id=None
    tools_files={}
    manifest_id=None
    private_manifest=root/"gvisor-manifest.json"
    extras={}
    rootfs=None
    rootfs_entries={}
    generation=None
    generation_ids={}
    test_process=None
    report = {"scope": "Native reserved-volume component only; no workload runtime or profile qualification", "result": "running",
              "private_mount_namespace":True,
              "fixture": str(root), "outer_reserved_bytes": (768 if args.suite=="actual-tasks" else 512) * 1024**2, "test_binary_sha256": binary_sha,
              "formatter_sha256": formatter_digest,
              "fixture_removed": False, "mounts_absent": False, "loops_detached": False, "workload_tasks_launched": False,
              "production_accepted": False, "runtime_qualified": False}
    if args.suite=="task-components":
        report["scope"]="Actual reserved-volume receiver-grant, bounded FIFO, immutable toolset/rootfs and cgroup configuration components; no workload task starts"
    report["suite"]=args.suite
    report["toolset_removed"]=args.suite in ("volumes","receiver-claim")
    if args.suite=="receiver-claim":
        report["scope"]="Native original receiver ownership and retained-volume restart exclusion only; no workload starts or clock qualification"
    if args.suite=="actual-tasks":
        report["input_only"]=args.input_only
        report["backend_only"]=args.backend_only
        report["rejected_output"]=args.reject_output
        report["scope"]="Actual private gVisor task adapter with synthetic fixture admission metadata; no released profile or app route"
    if args.missing_once:
        report.update(scope="Native HTTPS single readiness/Begin/Missing observation; no guest start; F51 remains open", missing_once=True, F51_closed=False)

    def call(argv):
        r = subprocess.run(argv, capture_output=True, timeout=60, check=False)
        if r.returncode:
            raise RuntimeError("Own fixture operation failed: " + str(argv[0]))
        return r.stdout.decode()

    def info(device):
        descriptor = os.open(device, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            m = os.fstat(descriptor)
            if not stat.S_ISBLK(m.st_mode) or os.major(m.st_rdev) != 7:
                raise ValueError("Unexpected loop device type")
            data = bytearray(232)
            try:
                fcntl.ioctl(descriptor, 0x4C05, data, True)
            except OSError as error:
                if error.errno == errno.ENXIO:
                    return None
                raise
            v = struct.unpack("=QQQQQIIII64s64s32sQQ", data)
            return {"image_id": (v[0], v[1]), "offset": v[3], "size": v[4], "number": v[5], "flags": v[8],
                    "nonce": v[9].split(b"\0", 1)[0].decode(), "rdev": m.st_rdev}
        finally:
            os.close(descriptor)

    def cleanup():
        if (root.stat().st_dev, root.stat().st_ino) != native_root or root.parent != Path("/var/tmp"):
            raise ValueError("Own fixture root identity changed")
        if generation is not None:
            # The test and all its daemon/shim/helper descendants were confined
            # to this fresh owned generation before exec. Killing the complete
            # generation is fixture retirement, never a product task receipt.
            for path,identity in generation_ids.items():
                m=path.lstat()
                if not stat.S_ISDIR(m.st_mode) or (m.st_dev,m.st_ino)!=identity:
                    raise ValueError("Own fixture cgroup replaced")
            # Capture kernel counters before generation retirement, including
            # failed jobs. Discarding these at rmdir would hide whether a real
            # OOM/PID limit, deadline or unrelated authority failure stopped it.
            observations={};pending=[generation]
            while pending:
                path=pending.pop()
                if len(observations)>=128:raise ValueError("Own cgroup diagnostic scope exceeded")
                m=path.lstat()
                if not stat.S_ISDIR(m.st_mode) or m.st_uid!=0 or path.is_symlink():raise ValueError("Own cgroup diagnostic directory changed")
                item={"dev":m.st_dev,"ino":m.st_ino}
                for name in ("cgroup.events","cpu.stat","memory.events","memory.peak","pids.events","pids.peak","pids.current"):
                    file=path/name
                    if file.exists():
                        data=file.read_text()
                        if len(data)>4096:raise ValueError("Own cgroup counter unbounded")
                        item[name]=data
                observations[str(path.relative_to(generation))]=item
                pending.extend(child for child in path.iterdir() if child.is_dir() and not child.is_symlink())
            report["cgroup_observations_before_retirement"]=observations
            (output/"cgroup-observations.json").write_text(json.dumps(observations,indent=2)+"\n")
            if test_process is not None and test_process.poll() is None:
                test_process.kill();test_process.wait(timeout=5)
            (generation/"cgroup.kill").write_text("1")
            end=time.monotonic()+5
            while "populated 1" in (generation/"cgroup.events").read_text() and time.monotonic()<end:
                time.sleep(.01)
            if "populated 1" in (generation/"cgroup.events").read_text():
                raise RuntimeError("Own fixture generation still populated; volumes retained")
            def retire(path):
                for child in list(path.iterdir()):
                    if child.is_dir() and not child.is_symlink():retire(child)
                path.rmdir()
            retire(generation)
            report["fixture_generation_retired"]=True
            report["task_records"]=[]
            usage=os.statvfs(mountpoint)
            report["outer_available_bytes_at_cleanup"]=usage.f_bavail*usage.f_frsize
            report["outer_available_inodes_at_cleanup"]=usage.f_favail
            for name in ("service.stdout.log","service.stderr.log","managed-service-observation.json","owner-child.stdout.log","owner-child.stderr.log","lease-child.stdout.log","lease-child.stderr.log","f51-missing-once-slot-before.bin","f51-missing-once-slot.bin","f51-missing-once-observation.json"):
                path=mountpoint/name
                if path.exists():
                    m=path.lstat()
                    if not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_size>1024**2:raise ValueError("Own bounded child diagnostics changed")
                    (output/name).write_bytes(path.read_bytes())
            for name in ("controller-prepare.stdout.log","controller-prepare.stderr.log","controller-exercise.stdout.log","controller-exercise.stderr.log","controller-result.json","controller-failure.json","controller-cancel-armed.json","controller-native-start.json","controller-native-start-record.json","controller-native-completed.json","controller-native-completed-record.json"):
                child=mountpoint/"controller"/name
                if child.exists():
                    m=child.lstat()
                    if child.is_symlink() or not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_size>1024**2:raise ValueError("Own controller diagnostic changed or exceeded its bound")
                    (output/name).write_bytes(child.read_bytes())
            try:
                # Private, positively owned fixture evidence only. Never copy keys,
                # tokens, raw input grants, project documents or the database itself.
                failure_slot=mountpoint/"service-state/managed-failure.bin"
                if failure_slot.exists():
                    m=failure_slot.lstat()
                    if failure_slot.is_symlink() or not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_nlink!=1 or stat.S_IMODE(m.st_mode)!=0o600 or m.st_size!=16384:raise ValueError("Private native failure slot changed")
                    data=failure_slot.read_bytes();length=int.from_bytes(data[:4],"little")
                    if length:
                        if not 0<length<=16384-36 or hashlib.sha256(data[36:36+length]).digest()!=data[4:36] or any(data[36+length:]):raise ValueError("Native failure diagnostic framing rejected")
                        value=json.loads(data[36:36+length])
                        if value.get("diagnostic_only") is not True:raise ValueError("Native failure diagnostic cannot be authority")
                        (output/"native-failure-diagnostic.json").write_bytes(data[36:36+length])
                    elif any(data):raise ValueError("Empty native failure slot contains nonzero framing")
                    (output/"native-failure-slot.bin").write_bytes(data)
                    slot_sha256=hashlib.sha256(data).hexdigest()
                    if hashlib.sha256((output/"native-failure-slot.bin").read_bytes()).hexdigest()!=slot_sha256:raise ValueError("Copied native failure slot changed")
                    report["native_failure_slot"]={"format_version":1,"diagnostic_only":True,"bytes":len(data),"sha256":slot_sha256,"declared_payload_bytes":length,"readback_verified":True}
                    (output/"native-failure-slot-observation.json").write_text(json.dumps(report["native_failure_slot"],indent=2)+"\n")
                database=mountpoint/"service-state/runner.db"
                if database.exists():
                    m=database.lstat()
                    if database.is_symlink() or not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_nlink!=1 or m.st_size>16*1024**2:raise ValueError("Own broker database diagnostic source changed")
                    with closing(sqlite3.connect(database.as_uri()+"?mode=ro",uri=True)) as connection:
                        connection.execute("PRAGMA query_only=ON")
                        summary={"diagnostic_only":True,"high_water":connection.execute("SELECT high_water FROM runner_binding WHERE id=1").fetchone()[0],"executions":[],"fences":[],"input_custody":[]}
                        for attempt,run,tool,revision,payload in connection.execute("SELECT attempt_id,run_id,tool_call_id,revision,payload FROM runner_executions LIMIT 16"):
                            if len(payload)>2*1024**2:raise ValueError("Own execution diagnostic exceeded bound")
                            value=json.loads(payload)["payload"];request=value["request"]
                            summary["executions"].append({"attempt_id":attempt,"run_id":run,"tool_call_id":tool,"revision":revision,"state":value["state"]["state"],"request_envelope_sha256":value["request_envelope_sha256"],"issued_at":request["issued_at"],"expires_at":request["expires_at"],"updated_at":value["updated_at"]})
                        for run,epoch,revoked in connection.execute("SELECT run_id,epoch,revoked FROM runner_fences LIMIT 16"):summary["fences"].append({"run_id":run,"epoch":epoch,"revoked":bool(revoked)})
                        for attempt,run,tool,payload in connection.execute("SELECT attempt_id,run_id,tool_call_id,payload FROM runner_input_custody LIMIT 16"):
                            summary["input_custody"].append({"attempt_id":attempt,"run_id":run,"tool_call_id":tool,"original_marker_sha256":hashlib.sha256(payload.encode()).hexdigest()})
                        encoded=(json.dumps(summary,indent=2)+"\n").encode()
                        if len(encoded)>65536:raise ValueError("Own broker diagnostic summary exceeded bound")
                        (output/"native-broker-summary.json").write_bytes(encoded)
            except Exception as diagnostic_error:
                report["diagnostic_export_failure"]=str(diagnostic_error)
                report["result"]="failed"
            report["owner_observations"]={}
            for name in ("reaper", "package-path"):
                path=root/"case-storage"/(name+"-observation.json")
                if path.exists():
                    data=path.read_bytes()
                    if path.is_symlink() or path.stat().st_uid!=0 or len(data)>4096:raise ValueError("Own preflight observation changed")
                    report[name+"_observation"]=json.loads(data)
                    (output/(name+"-observation.json")).write_bytes(data)
            for name in ("owner","lease"):
                path=root/"case-storage"/(name+"-observation.json")
                if path.exists():
                    data=path.read_bytes()
                    if path.is_symlink() or path.stat().st_uid!=0 or len(data)>4096:raise ValueError("Own owner observation changed")
                    report["owner_observations"][name]=json.loads(data)
                    (output/(name+"-observation.json")).write_bytes(data)
            for path in sorted(root.glob("case-storage/actual-tasks*/volumes/*/root/cw-*")):
                m=path.lstat()
                if not stat.S_ISDIR(m.st_mode) or m.st_uid!=0 or stat.S_IMODE(m.st_mode)!=0o700:raise ValueError("Task control directory changed")
                records=[]
                for name in ("task.a","task.b"):
                    slot=path/name
                    if not slot.exists():continue
                    data=slot.read_bytes();length=int.from_bytes(data[:4],"little")
                    if len(data)==16384 and 0<length<=16384-36 and hashlib.sha256(data[36:36+length]).digest()==data[4:36]:records.append(json.loads(data[36:36+length]))
                if not records:continue
                record=max(records,key=lambda r:r["sequence"]);index=len(report["task_records"])
                debug=path/"runsc-debug.log"
                if debug.exists():
                    m=debug.lstat()
                    if not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_size>16*1024**2:raise ValueError("Unbounded or substituted private debug log")
                    (output/("task-runtime-"+str(index)+".log")).write_bytes(debug.read_bytes())
                (output/("task-record-"+str(index)+".json")).write_text(json.dumps(record,indent=2)+"\n")
                report["task_records"].append(record)
                diagnostic=path/"authority-observation.bin"
                if diagnostic.exists():
                    data=diagnostic.read_bytes();length=int.from_bytes(data[:4],"little")
                    if len(data)==4096 and 0<length<=4096-36 and hashlib.sha256(data[36:36+length]).digest()==data[4:36]:
                        report.setdefault("authority_diagnostics",{})[str(index)]=json.loads(data[36:36+length])
                        (output/("task-authority-"+str(index)+".json")).write_bytes(data[36:36+length])
                for slot in (path/"wait-evidence").glob("*.wait"):
                    data=slot.read_bytes();length=int.from_bytes(data[:4],"little")
                    if len(data)==16384 and 0<length<=16384-36 and hashlib.sha256(data[36:36+length]).digest()==data[4:36]:
                        (output/("task-wait-"+str(index)+"-"+slot.stem+".json")).write_bytes(data[36:36+length])
                status=path.parents[2]/record["volume_id"]/"root/work/workspace/stdout-status"
                if status.exists() and not status.is_symlink() and status.stat().st_size<128:
                    report["probe_stdout_status"]=status.read_text()
                if any(m["operation"]=="task_start" and m["acknowledged"] for m in record["mutations"]):report["workload_tasks_launched"]=True
                if record.get("outcome_sha256"):
                    data=(path/"outcome.bin").read_bytes();length=int.from_bytes(data[:4],"little");payload=data[36:36+length]
                    if len(data)!=1024**2 or not 0<length<=1024**2-36 or hashlib.sha256(payload).digest()!=data[4:36] or hashlib.sha256(payload).hexdigest()!=record["outcome_sha256"]:raise ValueError("Durable task outcome changed")
                    (output/("task-outcome-"+str(index)+".json")).write_bytes(payload)
            if args.missing_once:
                observed_path=output/"f51-missing-once-observation.json"
                observed=json.loads(observed_path.read_bytes())
                for name,key in (("f51-missing-once-slot-before.bin","before_sha256"),("f51-missing-once-slot.bin","sha256")):
                    raw=(output/name).read_bytes()
                    if len(raw)!=16384 or hashlib.sha256(raw).hexdigest()!=observed["diagnostic"][key]:raise ValueError("Immediate original diagnostic snapshot changed")
                if not observed["harness_completed"] or not observed["service_stop"]["reaped"] or observed["runner_executions_before"]!=0 or observed["runner_executions_after"]!=0 or observed["task_records_before_stop"]!=0 or observed["task_records_after_stop"]!=0 or report["task_records"] or report["workload_tasks_launched"]:raise ValueError("Missing-once did not retain its no-guest/process boundary")
                report["missing_once_observation"]={"sha256":hashlib.sha256(observed_path.read_bytes()).hexdigest(),"http_status":observed["http_status"],"classification":observed["classification"],"http200_component_validated":observed["http_status"]==200 and observed["response_valid"],"F51_closed":False}
            if args.controller_case == "delivery":
                from delivery_evidence import export_delivery
                export_delivery(mountpoint, output, report)
        devices = json.loads(call(["/usr/sbin/losetup", "--json", "--list", "--output", "NAME,BACK-FILE"]))["loopdevices"]
        owned = {}
        for device in devices:
            back = device.get("back-file")
            if not back or not Path(back).is_relative_to(root):
                continue
            path = Path(back)
            if path.is_symlink() or path.resolve(strict=True) != path:
                raise ValueError("Own loop backing path changed")
            m = path.stat()
            actual = info(device["name"])
            if actual is None:
                continue
            if actual["image_id"] != (m.st_dev, m.st_ino) or m.st_uid != 0 or not stat.S_ISREG(m.st_mode):
                raise ValueError("Own loop inode binding changed")
            if path == image:
                if (m.st_dev, m.st_ino) != root_image_id or (loop is not None and device["name"] != loop):
                    raise ValueError("Outer loop ownership changed")
            else:
                records=[]
                for name in ("volume.a", "volume.b"):
                    data=(path.parent/name).read_bytes()
                    length=int.from_bytes(data[:4],"little")
                    if 0<length<=8192-36 and hashlib.sha256(data[36:36+length]).digest()==data[4:36]:
                        records.append(json.loads(data[36:36+length]))
                record=max(records,key=lambda r:r["sequence"])
                if actual["image_id"] != (record["image"]["dev"], record["image"]["ino"]) or actual["nonce"] != record["nonce"]:
                    raise ValueError("Nested loop has no exact private ownership record")
            owned[actual["rdev"]] = (device["name"], actual)
        report["own_loops_at_cleanup"] = len(owned)

        def detach(device, expected):
            current = info(device)
            if current is None:
                return
            if current != expected:
                raise ValueError("Loop device changed before cleanup")
            call(["/usr/sbin/losetup", "--detach", device])
            if info(device) is not None:
                raise RuntimeError("Own loop still physically attached")

        mounts = []
        for line in Path("/proc/self/mountinfo").read_text().splitlines():
            fields = line.split()
            point = Path(fields[4].replace("\\040", " ").replace("\\134", "\\"))
            if point.is_relative_to(root):
                major, minor = map(int, fields[2].split(":"))
                mounts.append((point, os.makedev(major, minor)))
        remaining={device_id:sum(1 for _,d in mounts if d==device_id) for _,device_id in mounts}
        for point, device_id in sorted(mounts, key=lambda v: len(v[0].parts), reverse=True):
            if device_id not in owned:
                raise ValueError("Unowned filesystem beneath fixture; cleanup refused")
            call(["/usr/bin/umount", "--", str(point)])
            remaining[device_id]-=1
            if remaining[device_id]==0:detach(*owned[device_id])
        for device, expected in owned.values():
            detach(device, expected)
        for line in Path("/proc/self/mountinfo").read_text().splitlines():
            if Path(line.split()[4]).is_relative_to(root):
                raise RuntimeError("Own mount remains")
        report["mounts_absent"] = True
        for device, _ in owned.values():
            if info(device) is not None:
                raise RuntimeError("Own loop remains")
        report["loops_detached"] = True
        if image.exists():
            m = image.stat()
            if image.is_symlink() or (m.st_dev, m.st_ino) != root_image_id:
                raise ValueError("Refusing removal of a substituted backing image")
            image.unlink()
        if mountpoint.exists():
            mountpoint.rmdir()
        if private_binary.exists():
            m=private_binary.stat()
            if private_binary.is_symlink() or (m.st_dev,m.st_ino)!=binary_id:
                raise ValueError("Own test executable changed before cleanup")
            private_binary.unlink()
        if tools_directory is not None:
            if tools_directory.parent!=Path("/var/tmp") or not tools_directory.name.startswith("cowork-gvisor-tools-") or (tools_directory.stat().st_dev,tools_directory.stat().st_ino)!=tools_id:
                raise ValueError("Own public toolset root identity changed")
            for name,identity in tools_files.items():
                path=tools_directory/name
                m=path.lstat()
                if not stat.S_ISREG(m.st_mode) or (m.st_dev,m.st_ino)!=identity:
                    raise ValueError("Own toolset file replaced before removal")
                path.unlink()
            if (tools_directory/"gvisor-bin").exists():
                (tools_directory/"gvisor-bin").rmdir()
            tools_directory.rmdir()
            report["toolset_removed"]=True
        if private_manifest.exists():
            m=private_manifest.lstat()
            if not stat.S_ISREG(m.st_mode) or (m.st_dev,m.st_ino)!=manifest_id:
                raise ValueError("Own toolset manifest replaced before removal")
            private_manifest.unlink()
        if rootfs is not None:
            for path,identity in sorted(rootfs_entries.items(),key=lambda v:len(v[0].parts),reverse=True):
                m=path.lstat()
                if (m.st_dev,m.st_ino)!=identity or stat.S_ISLNK(m.st_mode):raise ValueError("Own rootfs replaced")
                if path.is_dir():path.rmdir()
                else:path.unlink()
        for path,identity in extras.items():
            m=path.lstat()
            if not stat.S_ISREG(m.st_mode) or (m.st_dev,m.st_ino)!=identity:raise ValueError("Own auxiliary artifact replaced")
            path.unlink()
        root.rmdir()
        report["fixture_removed"] = True

    try:
        if args.suite in ("task-components","actual-tasks"):
            if args.gvisor_directory is None or args.gvisor_manifest is None or args.gvisor_manifest_sha256 is None:
                raise ValueError("Exact independent gVisor toolset manifest/directory required")
            manifest_bytes=args.gvisor_manifest.resolve(strict=True).read_bytes()
            if len(manifest_bytes)>65536 or hashlib.sha256(manifest_bytes).hexdigest()!=args.gvisor_manifest_sha256:
                raise ValueError("Toolset manifest checksum changed")
            manifest=json.loads(manifest_bytes)
            if manifest.get("format_version")!=1 or not 3<=len(manifest["files"])<=32:
                raise ValueError("Unexpected toolset manifest")
            tools_directory=Path("/var/tmp")/("cowork-gvisor-tools-"+uuid.uuid4().hex)
            tools_directory.mkdir(mode=0o755)
            tools_id=(tools_directory.stat().st_dev,tools_directory.stat().st_ino)
            source_root=args.gvisor_directory.resolve(strict=True)
            total=0
            for name,expected in manifest["files"].items():
                path=PurePosixPath(name)
                if path.is_absolute() or any(p in (".","..") for p in path.parts) or not (name in ("runsc","containerd-shim-runsc-v1") or len(path.parts)==2 and path.parts[0]=="gvisor-bin"):
                    raise ValueError("Unexpected toolset path")
                source=source_root.joinpath(*path.parts)
                if source.is_symlink() or not source.is_file() or source.stat().st_size!=expected["bytes"] or not 4<=expected["bytes"]<=128*1024**2:
                    raise ValueError("Invalid toolset source")
                total+=expected["bytes"]
                if total>512*1024**2:
                    raise ValueError("Toolset exceeded its bounded size")
                data=source.read_bytes()
                if not data.startswith(b"\x7fELF") or hashlib.sha256(data).hexdigest()!=expected["sha256"]:
                    raise ValueError("Toolset executable checksum changed")
                target=tools_directory.joinpath(*path.parts)
                target.parent.mkdir(mode=0o755,parents=True,exist_ok=True)
                fd=os.open(target,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,0o555)
                with os.fdopen(fd,"wb") as sink:
                    m=os.fstat(sink.fileno());tools_files[name]=(m.st_dev,m.st_ino)
                    sink.write(data);sink.flush();os.fsync(sink.fileno())
            if set(manifest["files"])!={"runsc","containerd-shim-runsc-v1","gvisor-bin/checkpointgofer","gvisor-bin/gvisor-proc-umounter","gvisor-bin/gvisor-sentry-prewarmer","gvisor-bin/gvisor_sentry","gvisor-bin/runsc-fd-parking","gvisor-bin/runsc-metric-server"}:
                raise ValueError("Incomplete pinned gVisor release toolset")
            (tools_directory/"gvisor-bin").chmod(0o555)
            tools_directory.chmod(0o555)
            fd=os.open(private_manifest,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,0o400)
            with os.fdopen(fd,"wb") as sink:
                m=os.fstat(sink.fileno());manifest_id=(m.st_dev,m.st_ino)
                sink.write(manifest_bytes);sink.flush();os.fsync(sink.fileno())
            report["gvisor_manifest_sha256"]=args.gvisor_manifest_sha256
        if args.suite=="actual-tasks":
            def private_file(path,data,mode):
                fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,mode)
                with os.fdopen(fd,"wb") as sink:
                    m=os.fstat(sink.fileno());extras[path]=(m.st_dev,m.st_ino)
                    sink.write(data);sink.flush();os.fsync(sink.fileno())
            labels=("fence","driver","reaper","containerd","probe","pause") + (("service",) if args.service_binary else ()) + (("controller",) if args.controller_binary else ())
            for label in labels:
                source=getattr(args,label+"_binary")
                expected=getattr(args,label+"_sha256")
                if source is None or expected is None:raise ValueError("Missing exact auxiliary executable")
                data=source.resolve(strict=True).read_bytes()
                if not data.startswith(b"\x7fELF") or len(data)>128*1024**2 or hashlib.sha256(data).hexdigest()!=expected:raise ValueError("Auxiliary executable changed")
                private_file(root/label,data,0o500)
            rootfs=root/"rootfs";rootfs.mkdir(mode=0o755)
            rootfs_entries[rootfs]=(rootfs.stat().st_dev,rootfs.stat().st_ino)
            entries={}
            for name in ("bin","dev","etc","proc","run","sys","tmp","workspace"):
                path=rootfs/name;path.mkdir(mode=0o555);rootfs_entries[path]=(path.stat().st_dev,path.stat().st_ino);entries[name]={"kind":"directory"}
            for name,source in (("probe","probe"),("cowork-pause","pause")):
                data=(root/source).read_bytes();path=rootfs/"bin"/name
                private_file(path,data,0o555);rootfs_entries[path]=extras.pop(path)
                entries["bin/"+name]={"kind":"file","bytes":len(data),"sha256":hashlib.sha256(data).hexdigest(),"executable":True}
            rootfs.chmod(0o555)
            private_file(root/"rootfs-manifest.json",(json.dumps({"format_version":1,"entries":entries},sort_keys=True,separators=(",",":"))+"\n").encode(),0o400)
            private_file(root/"host-sentinel",b"never enter workload",0o600)
            generation=Path("/sys/fs/cgroup")/("cowork-proof-"+uuid.uuid4().hex)
            generation.mkdir(mode=0o755);generation_ids[generation]=(generation.stat().st_dev,generation.stat().st_ino)
            (generation/"cgroup.subtree_control").write_text("+cpu +memory +pids")
            (generation/"memory.max").write_text(str(2*1024**3));(generation/"pids.max").write_text("512")
            (generation/"cpu.max").write_text("400000 100000")
            control=generation/"control";control.mkdir(mode=0o755);generation_ids[control]=(control.stat().st_dev,control.stat().st_ino)
        descriptor=os.open(private_binary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,0o500)
        with os.fdopen(descriptor,"wb") as target:
            m=os.fstat(target.fileno());binary_id=(m.st_dev,m.st_ino)
            target.write(binary_bytes);target.flush();os.fsync(target.fileno())
        fd = os.open(image, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)
        try:
            m = os.fstat(fd)
            root_image_id = (m.st_dev, m.st_ino)
            os.posix_fallocate(fd, 0, report["outer_reserved_bytes"])
            os.fsync(fd)
        finally:
            os.close(fd)
        call(["/usr/sbin/mke2fs", "-q", "-F", "-t", "ext4", "-m", "0", "-N", "4096", "-E", "nodiscard,lazy_itable_init=0,lazy_journal_init=0", str(image)])
        loop = call(["/usr/sbin/losetup", "--find", "--show", str(image)]).strip()
        if info(loop)["image_id"] != root_image_id:
            raise ValueError("Own outer loop attachment differs")
        mountpoint.mkdir(mode=0o700)
        call(["/usr/bin/mount", "-t", "ext4", "-o", "nodev,nosuid,noexec", "--", loop, str(mountpoint)])
        mountpoint.chmod(0o700)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("COWORK_", "PG"))}
        env["COWORK_RUNTIME_VOLUME_PROOF_DIR"] = str(mountpoint)
        if args.suite in ("task-components","actual-tasks"):
            env["COWORK_TEST_GVISOR_DIRECTORY"]=str(tools_directory)
            env["COWORK_TEST_GVISOR_MANIFEST"]=str(private_manifest)
            env["COWORK_TEST_GVISOR_MANIFEST_SHA256"]=args.gvisor_manifest_sha256
        before_exec=None
        if args.suite=="actual-tasks":
            env.update(COWORK_TEST_DRIVER_BINARY=str(root/"driver"),COWORK_TEST_DRIVER_SHA256=args.driver_sha256,COWORK_TEST_REAPER_BINARY=str(root/"reaper"))
            env.update(COWORK_TEST_ROOTFS_DIRECTORY=str(rootfs),COWORK_TEST_ROOTFS_MANIFEST=str(root/"rootfs-manifest.json"),COWORK_TEST_FENCE_BINARY=str(root/"fence"),COWORK_TEST_FENCE_SHA256=args.fence_sha256,COWORK_TEST_CONTAINERD_BINARY=str(root/"containerd"),COWORK_TEST_CONTAINERD_SHA256=args.containerd_sha256,COWORK_TEST_HOST_SENTINEL=str(root/"host-sentinel"),COWORK_TEST_HOST_SECRET="fixture-secret",COWORK_RUNTIME_CGROUP_PARENT=str(generation))
            def before_exec():
                descriptor=os.open(control/"cgroup.procs",os.O_WRONLY|os.O_CLOEXEC)
                os.write(descriptor,str(os.getpid()).encode());os.close(descriptor)
        if args.service_binary:
            env.update(COWORK_TEST_SERVICE_BINARY=str(root/"service"),COWORK_TEST_SERVICE_SHA256=args.service_sha256)
        if args.controller_binary:
            env.update(COWORK_TEST_CONTROLLER_BINARY=str(root/"controller"),COWORK_TEST_CONTROLLER_SHA256=args.controller_sha256,COWORK_TEST_CONTROLLER_CASE=args.controller_case)
        if args.input_only: env["COWORK_TEST_INPUT_ONLY"]="1"
        if args.backend_only: env["COWORK_TEST_BACKEND_ONLY"]="reject" if args.reject_output else "1"
        test_arguments=[str(private_binary),"--ignored","--test-threads=1","--nocapture"]
        if args.service_binary:
            selected = "actual_https_service_begin_and_missing_once_without_start" if args.missing_once else "actual_https_service_transfer_result_and_rejected_export"
            test_arguments += ["--exact", "managed_fixture::"+selected]
        if args.suite=="receiver-claim": test_arguments += ["--exact","volumes::receiver_claim_tests::original_receiver_claim_excludes_duplicates_and_retained_resource_restart"]
        test_process=subprocess.Popen(test_arguments,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,preexec_fn=before_exec)
        stdout,stderr=test_process.communicate(timeout=420 if args.suite=="actual-tasks" else 240)
        (output / "volume-tests.stdout.log").write_bytes(stdout)
        (output / "volume-tests.stderr.log").write_bytes(stderr)
        report["test_exit_code"] = test_process.returncode
        expected_count=13 if args.suite=="volumes" else (5 if args.suite=="task-components" else 1)
        report["test_result"] = "passed" if test_process.returncode == 0 and ("test result: ok. "+str(expected_count)+" passed;").encode() in stdout else "failed"
        report["result"] = report["test_result"]
    except Exception as error:
        report.update(result="failed", failure=str(error))
    finally:
        try:
            cleanup()
        except Exception as error:
            report.update(result="failed", cleanup_failure=str(error))
        for name in ("volume-tests.stdout.log", "volume-tests.stderr.log"):
            if (output / name).exists():
                report[name + ".sha256"] = hashlib.sha256((output / name).read_bytes()).hexdigest()
        (output / "physical-volume-report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
