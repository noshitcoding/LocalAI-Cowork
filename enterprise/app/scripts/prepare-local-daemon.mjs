import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile, writeFile, chmod } from "node:fs/promises";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(appRoot, "..");
const configuredTarget = process.env.TAURI_BUILD_TARGET?.trim();

function bundleTarget() {
  const target = configuredTarget ?? "";
  if ((target.includes("windows") || process.platform === "win32") && process.arch === "x64") {
    return { directory: "windows-x64", binary: "cowork-local-daemon.exe" };
  }
  if ((target.includes("linux") || process.platform === "linux") && process.arch === "x64") {
    return { directory: "linux-x64", binary: "cowork-local-daemon" };
  }
  throw new Error(`Unsupported local daemon release target: ${configuredTarget ?? `${process.platform}-${process.arch}`}`);
}

function run(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: repoRoot, env: process.env, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} failed (${signal ?? code})`));
    });
  });
}

const target = bundleTarget();
const cargoArgs = ["build", "--locked", "--release", "--package", "cowork-local-daemon"];
if (configuredTarget) cargoArgs.push("--target", configuredTarget);
await run(process.platform === "win32" ? "cargo.exe" : "cargo", cargoArgs);

const targetRoot = path.resolve(process.env.CARGO_TARGET_DIR || path.join(repoRoot, "target"));
const buildRoot = configuredTarget
  ? path.join(targetRoot, configuredTarget, "release")
  : path.join(targetRoot, "release");
const source = path.join(buildRoot, target.binary);
const destinationRoot = path.join(appRoot, "src-tauri", "resources", "daemon", target.directory);
const destination = path.join(destinationRoot, target.binary);
await mkdir(destinationRoot, { recursive: true });
await copyFile(source, destination);
if (process.platform !== "win32") await chmod(destination, 0o755);

const bytes = await readFile(destination);
const sha256 = createHash("sha256").update(bytes).digest("hex");
const files = [];
if (target.directory === "windows-x64") {
  const pdfiumSource = path.join(appRoot, "src-tauri", "resources", "pdfium", "bin", "pdfium.dll");
  const pdfiumDestination = path.join(destinationRoot, "pdfium.dll");
  await copyFile(pdfiumSource, pdfiumDestination);
  const pdfiumBytes = await readFile(pdfiumDestination);
  files.push({
    name: "pdfium.dll",
    sha256: createHash("sha256").update(pdfiumBytes).digest("hex"),
  });
}
const workspaceManifest = await readFile(path.join(repoRoot, "Cargo.toml"), "utf8");
const version = workspaceManifest.match(/\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)?.[1];
if (!version) throw new Error("Could not resolve workspace package version");

await writeFile(
  path.join(destinationRoot, "manifest.json"),
  `${JSON.stringify({ schemaVersion: 2, target: target.directory, binary: target.binary, sha256, version, files }, null, 2)}\n`,
  "utf8",
);
console.log(`Prepared ${target.directory}/${target.binary} (${sha256.slice(0, 16)})`);
