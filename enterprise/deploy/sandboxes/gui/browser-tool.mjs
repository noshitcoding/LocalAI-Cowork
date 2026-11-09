import { chromium } from 'playwright';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve, sep } from 'node:path';
import { spawn } from 'node:child_process';

const workspace = '/workspace';
const profile = `${workspace}/.cowork/browser-profile`;
const statePath = `${workspace}/.cowork/browser-state.json`;
const artifactRoot = `${workspace}/artifacts/browser`;

function insideWorkspace(path, fallback) {
  const candidate = resolve(workspace, path || fallback);
  if (candidate !== workspace && !candidate.startsWith(`${workspace}${sep}`)) {
    throw new Error('path must stay inside the run workspace');
  }
  return candidate;
}

function relativeWorkspace(path) {
  return path.slice(`${workspace}${sep}`.length).replaceAll('\\', '/');
}

function httpUrl(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('only HTTP(S) URLs are allowed');
  }
  return url.toString();
}

async function input() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const value = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('browser tool input must be an object');
  }
  return value;
}

async function previousState() {
  try {
    return JSON.parse(await readFile(statePath, 'utf8'));
  } catch {
    return {};
  }
}

const request = await input();
await mkdir(profile, { recursive: true });
await mkdir(artifactRoot, { recursive: true });
const timestamp = new Date().toISOString().replaceAll(/[:.]/g, '-');
const tracePath = `${artifactRoot}/${timestamp}-trace.zip`;
const logPath = `${artifactRoot}/${timestamp}-events.json`;
const videoDir = `${artifactRoot}/${timestamp}-video`;
const proxy = process.env.HTTPS_PROXY || process.env.https_proxy;
let browserConnection = null;
let closeContext = true;
const events = [];
const attach = (target) => {
  target.on('console', (message) => events.push({ type: 'console', level: message.type(), text: message.text() }));
  target.on('pageerror', (error) => events.push({ type: 'pageerror', text: error.message }));
  target.on('requestfailed', (entry) => events.push({ type: 'requestfailed', url: entry.url(), error: entry.failure()?.errorText }));
  target.on('response', (entry) => events.push({ type: 'response', url: entry.url(), status: entry.status() }));
};
let browser;
let page;
let tracingStarted = false;
let result = {};
let finalState = { url: '', title: '', updated_at: new Date().toISOString() };
let failure = null;
let traceWritten = false;
let logWritten = false;
let videos = [];
let videoPaths = [];

try {
  if (request.visible) {
    try {
      browserConnection = await chromium.connectOverCDP('http://127.0.0.1:9222');
    } catch {
      const args = [
        '--remote-debugging-address=127.0.0.1',
        '--remote-debugging-port=9222',
        `--user-data-dir=${profile}`,
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-dev-shm-usage',
        '--disable-background-networking',
        '--disable-component-update',
        '--disable-default-apps',
        '--disable-sync',
        '--metrics-recording-only',
        '--safebrowsing-disable-auto-update',
        '--disable-features=HttpsUpgrades,HttpsFirstModeV2ForTypicallySecureUsers,HttpsFirstBalancedModeAutoEnable,HttpsFirstModeV2ForEngagedSites',
        '--no-sandbox',
      ];
      if (proxy) {
        args.push(`--proxy-server=${proxy}`);
        args.push('--proxy-bypass-list=<-loopback>');
      }
      const chromiumProcess = spawn(chromium.executablePath(), args, { detached: true, stdio: 'ignore' });
      chromiumProcess.unref();
      let connected = false;
      for (let attempt = 0; attempt < 100; attempt += 1) {
        try {
          const response = await fetch('http://127.0.0.1:9222/json/version');
          if (response.ok) { connected = true; break; }
        } catch {}
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      if (!connected) throw new Error('visible Chromium did not expose its CDP endpoint');
      browserConnection = await chromium.connectOverCDP('http://127.0.0.1:9222');
    }
    browser = browserConnection.contexts()[0];
    if (!browser) throw new Error('visible Chromium did not provide a default context');
    closeContext = false;
  } else {
    browser = await chromium.launchPersistentContext(profile, {
      headless: true,
      acceptDownloads: true,
      proxy: proxy ? { server: proxy } : undefined,
      viewport: { width: request.width || 1440, height: request.height || 900 },
      recordVideo: request.record_video ? { dir: videoDir, size: { width: 1280, height: 720 } } : undefined,
    });
  }

  page = browser.pages()[0] || await browser.newPage();
  browser.pages().forEach(attach);
  browser.on('page', attach);
  await browser.tracing.start({ screenshots: true, snapshots: true, sources: true });
  tracingStarted = true;

  const oldState = await previousState();
  if (request.action !== 'navigate' && oldState.url && page.url() === 'about:blank') {
    await page.goto(httpUrl(oldState.url), { waitUntil: 'domcontentloaded', timeout: request.timeout_ms || 30_000 });
  }

  switch (request.action) {
    case 'navigate': {
      const response = await page.goto(httpUrl(request.url), {
        waitUntil: request.wait_until || 'domcontentloaded',
        timeout: request.timeout_ms || 30_000,
      });
      result = { status: response?.status() ?? null };
      break;
    }
    case 'click': {
      const locator = page.locator(request.selector).first();
      if (request.expect_download) {
        const [download] = await Promise.all([
          page.waitForEvent('download', { timeout: request.timeout_ms || 30_000 }),
          locator.click({ timeout: request.timeout_ms || 30_000 }),
        ]);
        const target = insideWorkspace(request.download_path, `artifacts/browser/${timestamp}-${download.suggestedFilename()}`);
        await mkdir(dirname(target), { recursive: true });
        await download.saveAs(target);
        result = { download: relativeWorkspace(target), suggested_filename: download.suggestedFilename() };
      } else {
        await locator.click({ timeout: request.timeout_ms || 30_000 });
      }
      break;
    }
    case 'fill':
      await page.locator(request.selector).first().fill(String(request.value ?? ''), { timeout: request.timeout_ms || 30_000 });
      break;
    case 'upload': {
      const paths = (Array.isArray(request.paths) ? request.paths : [request.path]).map((path) => insideWorkspace(path));
      await page.locator(request.selector).first().setInputFiles(paths, { timeout: request.timeout_ms || 30_000 });
      result = { uploaded: paths.map(relativeWorkspace) };
      break;
    }
    case 'screenshot': {
      const target = insideWorkspace(request.path, `artifacts/browser/${timestamp}-screenshot.png`);
      await mkdir(dirname(target), { recursive: true });
      await page.screenshot({ path: target, fullPage: request.full_page !== false });
      result = { screenshot: relativeWorkspace(target) };
      break;
    }
    case 'inspect': {
      const text = (await page.locator('body').innerText({ timeout: request.timeout_ms || 30_000 })).slice(0, request.max_chars || 100_000);
      const links = await page.locator('a').evaluateAll((items) => items.slice(0, 500).map((item) => ({ text: item.textContent?.trim() || '', href: item.href })));
      result = { text, links };
      break;
    }
    case 'tabs':
      result = { tabs: browser.pages().map((item, index) => ({ index, url: item.url() })), active: browser.pages().indexOf(page) };
      break;
    default:
      throw new Error(`unsupported browser action: ${request.action}`);
  }

  if (request.wait_ms) await page.waitForTimeout(Math.min(Number(request.wait_ms), 30_000));
} catch (error) {
  failure = {
    name: error instanceof Error ? error.name : 'Error',
    message: error instanceof Error ? error.message : String(error),
  };
  events.push({ type: 'toolerror', ...failure });
} finally {
  if (page) {
    try {
      finalState = { url: page.url(), title: await page.title(), updated_at: new Date().toISOString() };
      if (!failure) {
        await mkdir(dirname(statePath), { recursive: true });
        await writeFile(statePath, JSON.stringify(finalState));
      }
    } catch (error) {
      events.push({ type: 'finalization_error', stage: 'state', text: error instanceof Error ? error.message : String(error) });
    }
  }
  if (browser && tracingStarted) {
    try {
      await browser.tracing.stop({ path: tracePath });
      traceWritten = true;
    } catch (error) {
      events.push({ type: 'finalization_error', stage: 'trace', text: error instanceof Error ? error.message : String(error) });
    }
  }
  if (browser && request.record_video) videos = browser.pages().map((item) => item.video()).filter(Boolean);
  if (browser && closeContext) {
    try {
      await browser.close();
    } catch (error) {
      events.push({ type: 'finalization_error', stage: 'browser_close', text: error instanceof Error ? error.message : String(error) });
    }
  }
  for (const video of videos) {
    try {
      videoPaths.push(relativeWorkspace(await video.path()));
    } catch (error) {
      events.push({ type: 'finalization_error', stage: 'video', text: error instanceof Error ? error.message : String(error) });
    }
  }
  try {
    await writeFile(logPath, JSON.stringify(events));
    logWritten = true;
  } catch {}
}

const output = JSON.stringify({
  ok: !failure,
  ...result,
  ...finalState,
  error: failure,
  artifacts: [
    ...(traceWritten ? [relativeWorkspace(tracePath)] : []),
    ...(logWritten ? [relativeWorkspace(logPath)] : []),
    ...videoPaths,
  ],
});
if (browserConnection) {
  process.stdout.write(output, () => process.exit(failure ? 1 : 0));
} else {
  process.stdout.write(output);
  if (failure) process.exitCode = 1;
}
