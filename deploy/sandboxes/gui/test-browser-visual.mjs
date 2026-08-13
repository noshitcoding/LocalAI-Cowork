#!/usr/bin/env node
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const workspace = '/workspace'
const tool = '/opt/cowork/browser-tool.mjs'
const port = 18081
const origin = `http://127.0.0.1:${port}`

const html = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Open Cowork browser visual baseline</title></head>
<body style="margin:0;font-family:sans-serif;background:#121b2a;color:#f3f4f6">
  <header style="padding:28px;background:#166b8f"><h1>Open Cowork Browser Baseline</h1></header>
  <main style="padding:28px">
    <label>Name <input id="name" value="" /></label>
    <input id="file" type="file" />
    <button id="save">Save</button>
    <button id="popup">Open review tab</button>
    <a id="download" href="/download">Download result</a>
    <p id="status">Saved: pending</p>
  </main>
  <script>
    const name = document.querySelector('#name');
    const status = document.querySelector('#status');
    name.value = localStorage.getItem('cowork-name') || '';
    const render = () => { status.textContent = 'Saved: ' + (localStorage.getItem('cowork-name') || 'pending'); };
    name.addEventListener('input', () => localStorage.setItem('cowork-name', name.value));
    document.querySelector('#save').addEventListener('click', () => { localStorage.setItem('cowork-name', name.value); render(); });
    document.querySelector('#popup').addEventListener('click', () => window.open('/review', '_blank'));
    render();
    console.log('cowork-browser-baseline-ready');
  </script>
</body>
</html>`

const server = createServer((request, response) => {
  if (request.url === '/download') {
    response.writeHead(200, {
      'content-type': 'text/plain; charset=utf-8',
      'content-disposition': 'attachment; filename="cowork-result.txt"',
    })
    response.end('browser-download-ok')
    return
  }
  if (request.url === '/review') {
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    response.end('<!doctype html><title>Review tab</title><h1>Visible review tab</h1>')
    return
  }
  response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
  response.end(html)
})

function runTool(payload) {
  return new Promise((resolveOutput, reject) => {
    const child = spawn('node', [tool], { stdio: ['pipe', 'pipe', 'pipe'] })
    const stdout = []
    const stderr = []
    child.stdout.on('data', (chunk) => stdout.push(chunk))
    child.stderr.on('data', (chunk) => stderr.push(chunk))
    child.on('error', reject)
    child.on('exit', (code) => {
      const output = Buffer.concat(stdout).toString('utf8')
      if (code !== 0) {
        reject(new Error(`browser-tool failed (${code}): ${Buffer.concat(stderr).toString('utf8')}\n${output}`))
        return
      }
      try {
        const parsed = JSON.parse(output)
        if (!parsed.ok) throw new Error(`browser-tool reported failure: ${output}`)
        resolveOutput(parsed)
      } catch (error) {
        reject(error)
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

function workspacePath(relativePath) {
  const path = resolve(workspace, relativePath)
  if (path !== workspace && !path.startsWith(`${workspace}/`)) {
    throw new Error(`artifact escaped the workspace: ${relativePath}`)
  }
  return path
}

async function verifyArtifacts(output) {
  if (!Array.isArray(output.artifacts) || output.artifacts.length < 2) {
    throw new Error(`browser diagnostics are incomplete: ${JSON.stringify(output)}`)
  }
  for (const artifact of output.artifacts) {
    if (!artifact.startsWith('artifacts/browser/')) throw new Error(`invalid browser artifact path: ${artifact}`)
    if ((await stat(workspacePath(artifact))).size === 0) throw new Error(`empty browser artifact: ${artifact}`)
  }
}

await mkdir(workspace, { recursive: true })
await writeFile(`${workspace}/upload.txt`, 'browser-upload-ok')
await new Promise((resolveReady, reject) => {
  server.once('error', reject)
  server.listen(port, '127.0.0.1', resolveReady)
})

try {
  const navigate = await runTool({ action: 'navigate', url: origin, width: 1280, height: 720 })
  if (navigate.status !== 200 || navigate.title !== 'Open Cowork browser visual baseline') {
    throw new Error(`headless navigation regressed: ${JSON.stringify(navigate)}`)
  }
  await verifyArtifacts(navigate)

  await runTool({ action: 'fill', selector: '#name', value: 'distributed-runtime' })
  await runTool({ action: 'click', selector: '#save', wait_ms: 50 })
  const inspected = await runTool({ action: 'inspect', max_chars: 10_000 })
  if (!inspected.text.includes('Saved: distributed-runtime') || !inspected.links.some((link) => link.href === `${origin}/download`)) {
    throw new Error(`browser form state or link inspection regressed: ${JSON.stringify(inspected)}`)
  }

  const upload = await runTool({ action: 'upload', selector: '#file', path: 'upload.txt' })
  if (JSON.stringify(upload.uploaded) !== JSON.stringify(['upload.txt'])) {
    throw new Error(`browser upload regressed: ${JSON.stringify(upload)}`)
  }
  const download = await runTool({
    action: 'click',
    selector: '#download',
    expect_download: true,
    download_path: 'artifacts/browser/downloaded-result.txt',
  })
  if ((await readFile(workspacePath(download.download), 'utf8')) !== 'browser-download-ok') {
    throw new Error(`browser download content regressed: ${JSON.stringify(download)}`)
  }

  const screenshot = await runTool({ action: 'screenshot', path: 'artifacts/browser/headless-baseline.png' })
  const png = await readFile(workspacePath(screenshot.screenshot))
  if (png.length < 10_000 || png.readUInt32BE(16) !== 1440 || png.readUInt32BE(20) !== 900) {
    throw new Error(`headless screenshot geometry regressed: bytes=${png.length}`)
  }

  const visible = await runTool({ action: 'navigate', url: origin, visible: true })
  if (visible.status !== 200) throw new Error(`visible Chromium navigation regressed: ${JSON.stringify(visible)}`)
  await runTool({ action: 'click', selector: '#popup', visible: true, wait_ms: 250 })
  const tabs = await runTool({ action: 'tabs', visible: true })
  if (!Array.isArray(tabs.tabs) || tabs.tabs.length !== 2 || !tabs.tabs.some((tab) => tab.url === `${origin}/review`)) {
    throw new Error(`visible Chromium multi-tab state regressed: ${JSON.stringify(tabs)}`)
  }
  const visibleInspected = await runTool({ action: 'inspect', visible: true, max_chars: 10_000 })
  if (!visibleInspected.text.includes('Open Cowork Browser Baseline')) {
    throw new Error(`visible Chromium rendered content regressed: ${JSON.stringify(visibleInspected)}`)
  }
  const visibleScreenshot = await runTool({ action: 'screenshot', visible: true, path: 'artifacts/browser/visible-baseline.png' })
  const visiblePng = await readFile(workspacePath(visibleScreenshot.screenshot))
  const visibleWidth = visiblePng.length >= 24 ? visiblePng.readUInt32BE(16) : 0
  const visibleHeight = visiblePng.length >= 24 ? visiblePng.readUInt32BE(20) : 0
  if (visiblePng.length < 1_024
      || !visiblePng.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
      || visibleWidth < 640
      || visibleHeight < 480) {
    throw new Error(`visible Chromium screenshot regressed: bytes=${visiblePng.length}, geometry=${visibleWidth}x${visibleHeight}`)
  }

  const eventLogs = navigate.artifacts.filter((artifact) => artifact.endsWith('-events.json'))
  const events = JSON.parse(await readFile(workspacePath(eventLogs[0]), 'utf8'))
  if (!events.some((event) => event.type === 'console' && event.text === 'cowork-browser-baseline-ready')
      || !events.some((event) => event.type === 'response' && event.status === 200)) {
    throw new Error(`browser console/network diagnostics regressed: ${JSON.stringify(events)}`)
  }

  console.log(JSON.stringify({
    browser_visual_acceptance: 'ok',
    headless_screenshot_bytes: png.length,
    visible_screenshot_bytes: visiblePng.length,
    visible_screenshot_geometry: `${visibleWidth}x${visibleHeight}`,
    visible_tabs: tabs.tabs.length,
  }))
} finally {
  await new Promise((resolveClosed) => server.close(resolveClosed))
}
