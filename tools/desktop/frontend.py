"""Build the frontend and bind its output to unchanged source bytes; no tests."""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
def manifest(paths: list[Path]) -> dict:
    return {p.relative_to(ROOT).as_posix(): {"size": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
            for p in sorted(set(paths))}
def sources() -> dict:
    paths = [ROOT / name for name in ("package.json", "package-lock.json", "tsconfig.json", "tools/desktop/frontend.py")]
    for name in ("apps/web", "packages", "contracts"):
        paths.extend(p for p in (ROOT / name).rglob("*") if p.is_file()
                     and not {"node_modules", "dist", "target", ".git"}.intersection(p.relative_to(ROOT).parts))
    return manifest(paths)
def main() -> None:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node",type=Path,required=True)
    parser.add_argument("--npm",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    if args.output.exists():raise ValueError("Choose a new output directory")
    args.output.mkdir(parents=True)
    before=sources()
    build=subprocess.run([str(args.node),str(args.npm),"run","build"],cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    (args.output/"build.log").write_bytes(build.stdout)
    after=sources()
    receipt={"format":"localai-cowork-frontend-build/1","created_at":datetime.now(timezone.utc).isoformat(),
             "version":json.loads((ROOT/"package.json").read_text())["version"],"exit_code":build.returncode,
             "sources_unchanged":before==after,"sources":before,"outputs":manifest([p for p in (ROOT/"apps/web/dist").rglob("*") if p.is_file()]),
             "tests_run":False,"validation_scope":"build_only","release_qualified":False}
    (args.output/"receipt.json").write_text(json.dumps(receipt,indent=2,sort_keys=True)+"\n",encoding="utf-8")
    print(json.dumps({"exit_code":build.returncode,"sources_unchanged":before==after,"receipt":str(args.output/"receipt.json")}))
    if build.returncode or before!=after:raise SystemExit(build.returncode or 1)
if __name__=="__main__":main()
