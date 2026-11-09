// Compatibility entry point; Python is build tooling only, never an agent runtime.
import {execFileSync} from "node:child_process";
import {fileURLToPath} from "node:url";
execFileSync(process.platform==="win32"?"python":"python3",[fileURLToPath(new URL("./prepare.py",import.meta.url))],{stdio:"inherit"});
