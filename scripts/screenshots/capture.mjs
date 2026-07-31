#!/usr/bin/env node
// Deterministic screenshot harness. Boots the Vue UI headlessly (the same
// `src/dev/browserMock.ts` shim `pnpm dev` uses), seeds an exact EngineState
// via `window.__RSR_SEED_DATA__` (read by browserMock's install() before the
// fake tournament ticker can ever start), and writes PNGs to
// docs/screenshots/.
//
// Determinism, end to end:
// - Fixed 1000x800 viewport, fixed timezone (UTC) and locale so clock-based
//   text renders the same on any machine.
// - Playwright's clock API pins Date.now()/new Date() to one fixed instant
//   (fixtures.mjs's FAKE_NOW_MS) -- no wall-clock drift between runs.
// - The seeded EngineState replaces the mock's default state before the app
//   ever calls get_state(), so nothing is left to chance from the scripted
//   fake tournament (see browserMock.ts's `seeded` flag).
// - CSS animations/transitions are force-disabled after load (infinite
//   animations like the background bloom and the live-set pulse dot would
//   otherwise land on a different frame every run).
// - document.fonts.ready is awaited before every screenshot.
//
// Usage: `pnpm screenshots` (starts the dev server if one isn't already
// listening on :1420, captures, and shuts down anything it started).

import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import fs from 'node:fs';
import {
  FAKE_NOW_MS,
  stationIdle,
  stationLive,
  stationFinished,
  operatorConsole,
  settingsBase,
  availableSets,
} from './fixtures.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..', '..');
const PORT = 1420;
// Matches vite.config.ts's own default (server.host: false binds to plain
// "localhost", which on this machine resolves to the IPv6 loopback) -- using
// the same hostname the dev server itself binds to avoids a probe that
// checks one address family while the server listens on the other.
const BASE_URL = `http://localhost:${PORT}/`;
const OUT_DIR = path.join(ROOT, 'docs', 'screenshots');
const TMP_DIR = path.join(__dirname, '.tmp');

const VIEWPORT = { width: 1000, height: 800 };

// Belt and suspenders on top of the fixture seeding: infinite CSS animations
// (background bloom drift, the live-set pulse dot) and hover/focus
// transitions would otherwise be mid-cycle at a different point every run.
const DISABLE_MOTION_CSS = `
  *, *::before, *::after {
    animation: none !important;
    transition: none !important;
    caret-color: transparent !important;
  }
`;

async function isServerUp() {
  // A single fast probe was flaky here: Node's fetch occasionally timed out
  // against a server that curl and a plain retry reached immediately after
  // (Windows localhost resolution jitter). A couple of slightly longer
  // attempts costs at most a second when nothing is listening, and avoids
  // spawning a second dev server on top of one that's simply slow to answer
  // the very first probe.
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const res = await fetch(BASE_URL, { signal: AbortSignal.timeout(1500) });
      return res.status < 500;
    } catch {
      // try again
    }
  }
  return false;
}

async function killTree(child) {
  if (child.exitCode !== null || child.killed) return;
  await new Promise((resolve) => {
    if (process.platform === 'win32') {
      const k = spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
      k.on('close', resolve);
      k.on('error', resolve);
    } else {
      try {
        process.kill(-child.pid, 'SIGTERM');
      } catch {
        try { child.kill('SIGTERM'); } catch { /* already gone */ }
      }
      resolve();
    }
  });
}

async function startDevServer() {
  fs.mkdirSync(TMP_DIR, { recursive: true });
  const logPath = path.join(TMP_DIR, 'dev-server.log');
  const logFd = fs.openSync(logPath, 'w');
  const viteBin = path.join(ROOT, 'node_modules', 'vite', 'bin', 'vite.js');
  const child = spawn(process.execPath, [viteBin, '--port', String(PORT), '--strictPort'], {
    cwd: ROOT,
    stdio: ['ignore', logFd, logFd],
    detached: process.platform !== 'win32',
  });
  child.on('error', (e) => {
    throw new Error(`failed to start vite: ${e.message}`);
  });

  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (await isServerUp()) return child;
    if (child.exitCode !== null) {
      // Most likely EADDRINUSE: something is already listening on :1420 that
      // our own isServerUp() probe just hadn't managed to reach yet (seen in
      // practice as Windows localhost-resolution jitter on the very first
      // probe). Give it one more, more patient look before giving up --
      // and if it answers now, treat it as the pre-existing server this
      // script should never manage the lifecycle of, rather than failing.
      if (await isServerUp()) return null;
      throw new Error(`vite exited early (code ${child.exitCode}); see ${logPath}`);
    }
    await sleep(200);
  }
  await killTree(child);
  throw new Error(`dev server never became ready on :${PORT}; see ${logPath}`);
}

async function captureOne(browser, name, seedState, { onReady } = {}) {
  const context = await browser.newContext({
    viewport: VIEWPORT,
    deviceScaleFactor: 1,
    timezoneId: 'UTC',
    locale: 'en-US',
    colorScheme: 'dark',
    reducedMotion: 'reduce',
  });
  const page = await context.newPage();

  // Freeze Date.now()/new Date() before anything on the page runs. Timers
  // keep firing normally (setFixedTime doesn't fake them) -- only the wall
  // clock reading is pinned, which is all the app's status/clock text needs.
  await page.clock.setFixedTime(FAKE_NOW_MS);

  // Stashed before browserMock.ts's module code runs, so install() applies
  // it synchronously -- the app's very first get_state() already returns
  // this fixture, with the fake-tournament ticker permanently disabled.
  await page.addInitScript((state) => {
    window.__RSR_SEED_DATA__ = state;
  }, seedState);

  await page.goto(BASE_URL, { waitUntil: 'domcontentloaded' });

  // Applied once up front; animation:none/transition:none make injection
  // timing irrelevant (an element with no animation has no "current frame"
  // to be caught mid-cycle).
  await page.addStyleTag({ content: DISABLE_MOTION_CSS });

  if (onReady) await onReady(page);

  await page.evaluate(() => document.fonts.ready);

  const outPath = path.join(OUT_DIR, `${name}.png`);
  await page.screenshot({ path: outPath });
  await context.close();
  return outPath;
}

async function main() {
  fs.mkdirSync(OUT_DIR, { recursive: true });

  let devServer = null;
  const alreadyUp = await isServerUp();
  if (!alreadyUp) {
    devServer = await startDevServer();
  }

  const browser = await chromium.launch();
  try {
    const written = [];

    // Operator console first: it's the headline screenshot (the multi-station
    // view the README leads with), so it's also the first one captured.
    written.push(
      await captureOne(browser, 'operator-console', operatorConsole, {
        onReady: (page) =>
          page.locator('.oc-row', { hasText: 'Winners Quarter-Final' }).waitFor({ state: 'visible' }),
      }),
    );

    written.push(
      await captureOne(browser, 'available-sets', availableSets, {
        // The panel is open by default now, no click needed to expand it
        // (and clicking the summary would toggle it closed).
        onReady: (page) =>
          page.locator('.as-row', { hasText: 'Winners Round 1' }).waitFor({ state: 'visible' }),
      }),
    );

    written.push(
      await captureOne(browser, 'station-idle', stationIdle, {
        onReady: (page) => page.locator('.ls-idle-text').waitFor({ state: 'visible' }),
      }),
    );

    written.push(
      await captureOne(browser, 'station-live', stationLive, {
        onReady: (page) => page.locator('.ls-label', { hasText: 'Now playing' }).waitFor({ state: 'visible' }),
      }),
    );

    written.push(
      await captureOne(browser, 'station-finished', stationFinished, {
        onReady: (page) => page.locator('.ss-row', { hasText: 'LOOM' }).waitFor({ state: 'visible' }),
      }),
    );

    written.push(
      await captureOne(browser, 'settings', settingsBase, {
        onReady: async (page) => {
          await page.locator('.mv-actions button.linkish', { hasText: 'Settings' }).click();
          await page.locator('.sd-title', { hasText: 'Settings' }).waitFor({ state: 'visible' });
          // The drawer auto-fires an update check on first open (see
          // SettingsDrawer.vue's onMounted); wait for it to settle so the
          // hint text is never caught mid-"Checking...".
          await page
            .locator('.sd-hint', { hasText: /latest version/i })
            .waitFor({ state: 'visible', timeout: 10_000 });
        },
      }),
    );

    for (const p of written) console.log(`wrote ${path.relative(ROOT, p)}`);
  } finally {
    await browser.close();
    if (devServer) await killTree(devServer);
  }
}

main().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
