"""Actual final Docker COPY filesystem import graph, without a Docker daemon."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
DOCKERFILE = Path(__file__).with_name("Dockerfile")


def final_copy_layout(destination):
    commands = []
    text = DOCKERFILE.read_text(encoding="utf-8").replace("\\\n", " ")
    for line in text.splitlines():
        fields = shlex.split(line, comments=True)
        if not fields:
            continue
        if fields[0].upper() == "FROM":
            commands = []
        else:
            commands.append(line)
    workdir, entrypoint = destination, None
    copied = []
    for line in commands:
        fields = shlex.split(line, comments=True)
        if fields[0].upper() == "WORKDIR":
            workdir = destination / fields[1].lstrip("/")
            workdir.mkdir(parents=True, exist_ok=True)
        elif fields[0].upper() == "COPY" and not fields[1].startswith("--from="):
            sources, target = fields[1:-1], fields[-1]
            directory = destination / target.lstrip("/") if target.startswith("/") else workdir / target
            directory.mkdir(parents=True, exist_ok=True)
            for relative in sources:
                source = ROOT / relative
                assert source.is_file() and source.resolve().is_relative_to(ROOT.resolve())
                output = directory / source.name
                shutil.copy2(source, output)
                copied.append(output)
        elif fields[0].upper() == "ENTRYPOINT":
            entrypoint = json.loads(line.strip().split(None, 1)[1])
    return workdir, copied, entrypoint


def import_layout(workdir):
    script = """
from pathlib import Path
import importlib,json,sys
root=Path.cwd().resolve()
sys.path.insert(0,str(root))
names=['local_entrypoint','remote_entrypoint','provisioner','instance_backups','instance_imports','instance_import_target','operation_journal','instance_updates','execution_enrollment']
for name in names:
 module=importlib.import_module(name)
 assert Path(module.__file__).resolve().parent==root, 'ambient module substitution'
print(json.dumps(names))
"""
    return subprocess.run([sys.executable, "-I", "-c", script], cwd=workdir,
                          env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}, capture_output=True, text=True, timeout=15)


class ProvisionerPackageTests(unittest.TestCase):
    def test_final_copy_layout_imports_actual_controller_module_graph(self):
        with tempfile.TemporaryDirectory(prefix="cowork-package-import-") as temporary:
            workdir, copied, entrypoint = final_copy_layout(Path(temporary))
            self.assertEqual(entrypoint, ["python3", "local_entrypoint.py"])
            self.assertTrue((workdir / entrypoint[1]).is_file())
            self.assertGreater(len(copied), 15)
            result = import_layout(workdir)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("instance_imports", json.loads(result.stdout))

    def test_missing_import_module_cannot_fall_back_to_checkout_or_pythonpath(self):
        with tempfile.TemporaryDirectory(prefix="cowork-package-missing-") as temporary:
            workdir, _, _ = final_copy_layout(Path(temporary))
            (workdir / "instance_imports.py").unlink()
            result = import_layout(workdir)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("ModuleNotFoundError", result.stderr)
            self.assertIn("instance_imports", result.stderr)


if __name__ == "__main__":
    unittest.main()
