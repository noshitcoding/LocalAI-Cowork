"""Verify the built preview in Chrome with a private disposable daemon profile.

Requires Playwright in the selected Python environment. Never installs an app,
opens an existing profile, launches a terminal or dispatches a model request.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--fault-chunk", action="store_true", help="Abort the first optional JS download and verify local recovery")
parser.add_argument("--expected-version", default=json.loads((ROOT / "package.json").read_text())["version"])
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=False)
data = Path(tempfile.mkdtemp(prefix="cowork-preview-"))
if os.name == "nt":
    sid = re.search(r"S-1-5-[0-9-]+", subprocess.check_output(["whoami", "/user", "/fo", "csv", "/nh"], text=True)).group()
    subprocess.run(["icacls", str(data), "/inheritance:r", "/grant:r", f"*{sid}:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"], check=True, capture_output=True)
    subprocess.run(["icacls", str(data), "/setowner", "*" + sid], check=True, capture_output=True)
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
base = f"http://127.0.0.1:{port}"
env = {key: value for key, value in os.environ.items() if not key.startswith("COWORK_")}
env.update(COWORK_DATA_DIR=str(data), COWORK_PORT=str(port), COWORK_WEB_DIR=str(ROOT / "apps/web/dist"), RUST_LOG="error")
token = ""
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def call(method, path, body=None):
    payload = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(base + "/api/v2" + path, data=payload, method=method,
                                     headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
    with opener.open(request, timeout=10) as response:
        return json.load(response)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


report = {"passed": False, "scope": "Built web preview and lazy workspace navigation on a disposable Windows/Chrome profile", "expected_version": args.expected_version, "checks": [], "installed_app_accepted": False, "model_requests": 0}
page = None
with (output / "daemon.log").open("wb") as log:
    daemon = subprocess.Popen([str(binary)], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                              **({"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}))
    try:
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if daemon.poll() is not None:
                raise RuntimeError("Disposable daemon exited before readiness")
            try:
                token = (data / "daemon.token").read_text().strip()
                health = call("GET", "/health")
                break
            except (OSError, ValueError):
                time.sleep(.1)
        else:
            raise RuntimeError("Disposable daemon readiness timed out")
        assert health["version"] == args.expected_version, health["version"]
        project = call("POST", "/projects", {"name": "V2 Preview"})
        order = call("POST", "/orders", {"project_id": project["id"], "title": "Release-Prüfung"})
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(channel="chrome", headless=True)
            context = browser.new_context(viewport={"width": 1440, "height": 1000})
            page = context.new_page()
            errors, requests = [], []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on("request", lambda request: requests.append(request.url))
            page.add_init_script("sessionStorage.setItem('cowork-v2-token'," + json.dumps(token) + ");sessionStorage.setItem('cowork-v2-endpoint'," + json.dumps(base) + ");sessionStorage.setItem('cowork-v2-connection-kind','server');")
            page.goto(base)
            page.get_by_role("button", name=project["name"], exact=True).click()
            page.get_by_role("button", name=order["title"], exact=True).click()
            page.locator("#prompt").wait_for()
            report["checks"].append("Current chat opens with the built frontend")
            assert not any("/assets/TerminalDock-" in url or "/assets/DeveloperWorkspace-" in url or "/assets/OfficeWorkbench-" in url or "/assets/SettingsWorkspace-" in url or "/assets/WorkflowLibrary-" in url or "/assets/TeamPanel-" in url for url in requests)
            report["checks"].append("Optional workspace chunks are absent from the initial requests")
            draft = page.locator("#prompt")
            draft.fill("Dieser Entwurf bleibt beim Wechsel erhalten.")
            def open_tool(name):
                if not page.locator("#app-menu").is_visible():
                    page.locator('button[aria-controls="app-menu"]').click()
                group = page.get_by_role("button", name="Werkzeuge", exact=True)
                if group.get_attribute("aria-expanded") != "true":
                    group.click()
                page.get_by_role("button", name=name, exact=True).click()
            if args.fault_chunk:
                failed = []
                def first_chunk_failure(route):
                    if not failed:
                        failed.append(route.request.url)
                        route.abort()
                    else:
                        route.continue_()
                page.route("**/OfficeWorkbench-*.js", first_chunk_failure)
            open_tool("Dokumente")
            if args.fault_chunk:
                failure = page.locator(".deferred-workspace-error")
                failure.wait_for()
                assert page.locator("#prompt").count() == 1
                page.get_by_role("button", name="Zurück zum Chat", exact=True).click()
                assert draft.input_value() == "Dieser Entwurf bleibt beim Wechsel erhalten."
                open_tool("Dokumente")
                failure.get_by_role("button", name="Erneut versuchen", exact=True).click()
                report["checks"].append("One failed optional chunk preserves chat and draft; explicit retry recovers")
            try:
                page.get_by_role("heading", name="Dokumente und Bilder", exact=True).wait_for(timeout=10000)
            except Exception:
                page.screenshot(path=str(output / "workspace-failure.png"), full_page=True)
                debug = page.locator(".office-workbench").evaluate("el => {const result=[];for(let p=el;p;p=p.parentElement){const s=getComputedStyle(p),r=p.getBoundingClientRect();result.push({tag:p.tagName,class:p.className,hidden:p.hidden,display:s.display,visibility:s.visibility,rect:{width:r.width,height:r.height}});}return result;}")
                (output / "workspace-dom.json").write_text(json.dumps(debug,indent=2),encoding="utf-8")
                raise
            open_tool("Browser & GitHub")
            page.get_by_role("heading", name="Entwicklungswerkzeuge", exact=True).wait_for()
            open_tool("Terminal")
            page.locator(".terminal-dock").wait_for()
            report["checks"].append("Documents, Browser/GitHub and Terminal load on demand")
            page.get_by_role("button", name="Einstellungen", exact=True).click()
            page.locator(".settings-workspace").wait_for()
            report["checks"].append("Settings load on demand")
            page.get_by_role("button", name=order["title"], exact=True).click()
            # Switching views must retain the existing chat and unsent draft.
            page.get_by_role("button", name="Zurück zum Chat", exact=True).click()
            assert draft.input_value() == "Dieser Entwurf bleibt beim Wechsel erhalten."
            report["checks"].append("Unsent chat draft survives all workspace switches")
            page.locator('button[aria-controls="app-menu"]').click()
            group = page.get_by_role("button", name="Aufgaben & Abläufe", exact=True)
            if group.get_attribute("aria-expanded") != "true":
                group.click()
            page.get_by_role("button", name="Workflows", exact=True).click()
            page.locator('.workflow-dialog [role="tablist"]').first.wait_for()
            page.locator(".workflow-dialog > header button").click()
            report["checks"].append("Workflow library loads on demand and closes without a run")
            assert draft.input_value() == "Dieser Entwurf bleibt beim Wechsel erhalten."
            page.screenshot(path=str(output / "desktop.png"), full_page=True, animations="disabled")
            page.set_viewport_size({"width": 390, "height": 844})
            assert not page.evaluate("document.documentElement.scrollWidth > innerWidth + 1")
            page.screenshot(path=str(output / "mobile.png"), full_page=True, animations="disabled")
            report["checks"].append("390px viewport has no document horizontal overflow")
            assert call("GET", f"/orders/{order['id']}/runs") == []
            assert call("GET", "/terminals/status")["active_sessions"] == 0
            assert errors == [], errors
            report["checks"].append("No model runs, terminal processes or browser exceptions")
            report["browser"] = browser.version
            browser.close()
        report.update(passed=True, daemon_sha256=digest(binary), frontend={str(path.relative_to(ROOT)): digest(path) for path in (ROOT / "apps/web/dist").rglob("*") if path.is_file()}, source_sha256={name: digest(ROOT / name) for name in ["packages/ui/src/App.tsx", "packages/ui/src/DeferredWorkspaces.tsx", "packages/ui/src/SettingsWorkspace.tsx", "packages/client/src/validators.js", "tools/contracts/typescript.mjs"]})
    except BaseException as error:
        report["error"] = str(error)
        if page is not None:
            try:
                page.screenshot(path=str(output / "failure.png"), full_page=True)
            except Exception:
                pass
        raise
    finally:
        if daemon.poll() is None:
            daemon.kill()
        daemon.wait(timeout=10)
        report["own_daemon_stopped"] = daemon.poll() is not None
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        # Preserve only the path to this disposable profile for inspection.
        (output / "fixture-path.txt").write_text(str(data), encoding="utf-8")
print(json.dumps({"passed": report["passed"], "checks": len(report["checks"]), "output": str(output)}))
