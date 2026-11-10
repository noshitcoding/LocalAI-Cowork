#!/bin/bash
# Development-only physical boundary probe. Run as root only in a disposable
# Linux test host with containerd, runsc, busybox-static and ext4 tools installed.
# It creates its own daemon/socket, cgroup and reserved 64 MiB loop volume. It
# never configures Docker, WSL, a system service or an application runner profile.
# A passing result does not qualify a production runtime or signed package.
set -eu
python3 - <<'PY'
import json, os, pathlib, shutil, subprocess, time, uuid
root=pathlib.Path('/opt')/('cowork-proof-'+uuid.uuid4().hex)
root.mkdir(mode=0o700)
runtime=pathlib.Path('/run')/root.name
runtime.mkdir(mode=0o700)
(root/'containerd.toml').write_text('version = 3\ndisabled_plugins = ["io.containerd.cri.v1.images", "io.containerd.cri.v1.runtime"]\n')
(root/'debug').mkdir(mode=0o755)
(root/'runsc.toml').write_text('[runsc_config]\nplatform="systrap"\nnetwork="none"\nhost-uds="none"\nhost-fifo="none"\ndirectfs="false"\ndebug="true"\nstrace="true"\ndebug-log="'+str(root/'debug')+'/"\n')
log=(root/'containerd.log').open('wb')
daemon=subprocess.Popen(['containerd','--config',str(root/'containerd.toml'),'--address',str(runtime/'containerd.sock'),'--root',str(root/'containerd'),'--state',str(runtime/'state')],stdout=log,stderr=log)
ctr=['ctr','--address',str(runtime/'containerd.sock'),'--namespace','cowork-proof']
def call(args, **kwargs):
    result=subprocess.run(args,capture_output=True,text=True,timeout=60,**kwargs)
    if result.returncode:
        print('FAILED_EXIT', result.returncode, flush=True)
        for path in (root/'debug').glob('*'):
            text=path.read_text(errors='replace')
            if 'boot' in path.name: print(path.name, text[-24000:],flush=True)
        raise RuntimeError(str(args)+': '+result.stdout+result.stderr)
    return result.stdout
mounted=False
loop=None
cgroup=pathlib.Path('/sys/fs/cgroup')/('cowork-proof-'+uuid.uuid4().hex)
def stop(task):
    call(ctr+['tasks','kill','--signal','SIGKILL','--all',task])
    for _ in range(200):
        rows=[line.split() for line in call(ctr+['tasks','ls']).splitlines()[1:]]
        row=next((row for row in rows if row and row[0]==task),None)
        if row and row[-1]=='STOPPED':break
        time.sleep(.05)
    else:raise RuntimeError('Own task termination not confirmed')
    call(ctr+['tasks','delete',task])
try:
    for _ in range(200):
        if (runtime/'containerd.sock').exists(): break
        if daemon.poll() is not None: raise RuntimeError('Own containerd stopped')
        time.sleep(.05)
    print(call(ctr+['version']),flush=True)
    bundle=root/'bundle';bundle.mkdir()
    call(['runsc','spec'],cwd=bundle)
    rootfs=bundle/'rootfs';rootfs.mkdir()
    for name in ['bin','proc','sys','dev','tmp','workspace','etc']:(rootfs/name).mkdir()
    shutil.copy2('/bin/busybox',rootfs/'bin/busybox')
    (rootfs/'bin/sh').symlink_to('busybox')
    workspace=root/'workspace';workspace.mkdir()
    volume=root/'workspace.ext4'
    call(['fallocate','-l','67108864',str(volume)])
    call(['mkfs.ext4','-q','-m','0','-N','2048',str(volume)])
    loop=call(['losetup','--find','--show',str(volume)]).strip()
    call(['mount','-o','nodev,nosuid,noexec',loop,str(workspace)]);mounted=True
    os.chown(workspace,1000,1000)
    (root/'host-secret').write_text('synthetic-host-secret-must-stay-outside')
    cgroup.mkdir()
    (cgroup/'cgroup.subtree_control').write_text('+cpu +memory +pids')
    spec=json.loads((bundle/'config.json').read_text())
    spec['root']={'path':str(rootfs),'readonly':True}
    spec['process'].update(terminal=False,user={'uid':1000,'gid':1000},cwd='/workspace',env=['PATH=/bin','HOME=/workspace'],noNewPrivileges=True,
        capabilities={'bounding':[],'effective':[],'inheritable':[],'permitted':[],'ambient':[]},
        rlimits=[{'type':'RLIMIT_NOFILE','hard':128,'soft':128},{'type':'RLIMIT_NPROC','hard':64,'soft':64},{'type':'RLIMIT_FSIZE','hard':67108864,'soft':67108864}])
    spec['linux']['cgroupsPath']=str(cgroup.relative_to('/sys/fs/cgroup'))+'/smoke'
    spec['linux']['resources']={'memory':{'limit':536870912,'swap':536870912},'cpu':{'quota':100000,'period':100000},'pids':{'limit':256}}
    spec['mounts']=[mount for mount in spec['mounts'] if mount['destination'] not in ['/tmp','/dev/shm']]
    spec['mounts'] += [{'destination':'/tmp','type':'tmpfs','source':'tmpfs','options':['nodev','nosuid','noexec','size=16m']},
        {'destination':'/workspace','type':'bind','source':str(workspace),'options':['rbind','rw','nodev','nosuid','noexec']}]
    spec['process']['args']=['/bin/sh','-ec', '''
      exec >/workspace/stdout.log 2>/workspace/stderr.log
      /bin/busybox uname -a
      /bin/busybox dmesg | /bin/busybox head -3
      for path in /mnt/c /mnt/wsl /opt/cowork-proof/host-secret /run/cowork-proof/containerd.sock /var/run/docker.sock; do
        test ! -e "$path" || exit 31
      done
      if /bin/busybox wget -T 2 -O /workspace/network http://1.1.1.1/; then exit 32; fi
      if /bin/busybox wget -T 2 -O /workspace/network http://169.254.169.254/; then exit 33; fi
      printf 'verified isolated output\n' > /workspace/result.txt
      printf 'SMOKE_COMPLETE\n'
    ''']
    (bundle/'config.json').write_text(json.dumps(spec))
    output=call(ctr+['run','--null-io','--runtime','io.containerd.runsc.v1','--runtime-config-path',str(root/'runsc.toml'),'--config',str(bundle/'config.json'),'smoke'])
    print(output,flush=True)
    print((workspace/'stdout.log').read_text(),flush=True)
    print((workspace/'stderr.log').read_text(),flush=True)
    info=json.loads(call(ctr+['containers','info','smoke']))
    assert info['Runtime']['Name']=='io.containerd.runsc.v1'
    print(call(ctr+['tasks','ls']),flush=True)
    assert (workspace/'result.txt').read_text()=='verified isolated output\n'
    print('MANAGED_RUNTIME_SMOKE_PASSED',flush=True)
    (root/'runsc.toml').write_text('[runsc_config]\nplatform="systrap"\nnetwork="none"\nhost-uds="none"\nhost-fifo="none"\ndirectfs="false"\n')
    def probe(name, shell, detached=False):
        spec['linux']['cgroupsPath']=str(cgroup.relative_to('/sys/fs/cgroup'))+'/'+name
        spec['process']['args']=['/bin/sh','-ec','exec >/workspace/stdout.log 2>/workspace/stderr.log\n'+shell]
        path=bundle/(name+'.json');path.write_text(json.dumps(spec))
        argv=ctr+['run','--null-io','--runtime','io.containerd.runsc.v1','--runtime-config-path',str(root/'runsc.toml'),'--config',str(path)]
        if detached:argv+=['--detach']
        return subprocess.run(argv+[name],capture_output=True,text=True,timeout=30)
    disk=probe('disk','/bin/busybox dd if=/dev/zero of=/workspace/full.bin bs=1048576 count=128')
    print('DISK_RESULT',disk.returncode,disk.stderr,(workspace/'stderr.log').read_text(),(workspace/'full.bin').stat().st_size,flush=True)
    assert disk.returncode!=0
    assert (workspace/'full.bin').stat().st_size<=67108864
    free=os.statvfs(workspace)
    assert free.f_bavail*free.f_frsize<1048576
    print('DISK_BOUND', (workspace/'full.bin').stat().st_size, flush=True)
    (workspace/'full.bin').unlink()
    inodes=probe('inodes','i=0; while test "$i" -lt 4000; do : > /workspace/file-$i; i=$((i+1)); done')
    assert inodes.returncode!=0
    count=sum(1 for path in workspace.glob('file-*'))
    assert count<2048
    assert os.statvfs(workspace).f_favail==0
    for path in workspace.glob('file-*'):path.unlink()
    print('INODE_BOUND',count,flush=True)
    cpu=probe('cpu','for n in 1 2 3 4; do (while :; do :; done) & done; wait',True)
    assert cpu.returncode==0,cpu.stderr
    child=cgroup/'cpu'
    assert (child/'cpu.max').read_text().strip()=='100000 100000'
    assert (child/'memory.max').read_text().strip()=='536870912'
    assert (child/'pids.max').read_text().strip()=='256'
    time.sleep(2)
    stats=dict(line.split() for line in (child/'cpu.stat').read_text().splitlines())
    assert int(stats['nr_throttled'])>0
    stop('cpu')
    assert not child.exists() or 'populated 0' in (child/'cgroup.events').read_text()
    print('CPU_THROTTLE_AND_PHYSICAL_STOP',stats,flush=True)
    fork=probe('fork','i=0; while test "$i" -lt 256; do /bin/busybox sleep 60 & i=$((i+1)); done; wait')
    assert fork.returncode!=0
    assert 'fork' in (workspace/'stderr.log').read_text().lower()
    assert 'populated 0' in (cgroup/'cgroup.events').read_text()
    print('FORK_BOUND_AND_NO_CHILDREN',flush=True)
    before=dict(line.split() for line in (cgroup/'memory.events').read_text().splitlines())
    memory=probe('memory',"/bin/busybox awk 'BEGIN {s=\"x\";for(i=0;i<31;i++)s=s s; print length(s)}'")
    assert memory.returncode!=0
    after=dict(line.split() for line in (cgroup/'memory.events').read_text().splitlines())
    assert int(after['oom_kill'])>int(before['oom_kill'])
    assert 'populated 0' in (cgroup/'cgroup.events').read_text()
    print('MEMORY_HARD_BOUND',after,flush=True)
    print('MANAGED_RUNTIME_RESOURCE_PROBES_PASSED',flush=True)
finally:
    try:
        active=call(ctr+['tasks','ls'])
        print(active,flush=True)
        for line in active.splitlines()[1:]:
            parts=line.split()
            if not parts:continue
            task=parts[0]
            assert task in {'smoke','cpu','fork','memory','disk','inodes'}
            if parts[-1]=='RUNNING':
                stop(task)
            else:call(ctr+['tasks','delete',task])
        if cgroup.exists():assert 'populated 0' in (cgroup/'cgroup.events').read_text()
    finally:
        daemon.terminate();daemon.wait(timeout=15);log.close()
    if mounted:call(['umount',str(root/'workspace')])
    if loop:call(['losetup','-d',loop])
    assert not call(['losetup','-j',str(root/'workspace.ext4')]).strip()
    if cgroup.exists():
        for child in sorted((path for path in cgroup.rglob('*') if path.is_dir()),key=lambda path:len(path.parts),reverse=True):
            assert 'populated 0' in (child/'cgroup.events').read_text()
            child.rmdir()
        cgroup.rmdir()
    print('OWN_CONTAINERD_STOPPED',flush=True)
PY
