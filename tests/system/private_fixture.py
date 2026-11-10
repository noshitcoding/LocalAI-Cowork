"""Private permissions for newly created empty system-test fixtures only."""
import csv
import os
import pathlib
import subprocess


def protect_empty_fixture(root):
    """Bind a newly created, still empty test root to the current Windows user."""
    root=pathlib.Path(root).resolve()
    assert root.is_dir() and not any(root.iterdir()), 'Only a new empty fixture may receive private ACLs'
    if os.name!='nt':
        return
    options={'creationflags':subprocess.CREATE_NO_WINDOW}
    identity=subprocess.check_output(['whoami','/user','/fo','csv','/nh'],text=True,**options)
    sid=next(csv.reader(identity.strip().splitlines()))[1]
    assert sid.startswith('S-1-5-')
    subprocess.run(['icacls',str(root),'/inheritance:r','/grant:r',f'*{sid}:(OI)(CI)F',
        '*S-1-5-18:(OI)(CI)F','*S-1-5-32-544:(OI)(CI)F'],check=True,stdout=subprocess.DEVNULL,**options)
    subprocess.run(['icacls',str(root),'/setowner','*'+sid],check=True,stdout=subprocess.DEVNULL,**options)
