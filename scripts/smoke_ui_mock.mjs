#!/usr/bin/env node
/**
 * smoke_ui_mock.mjs — scripted mock walkthrough (COLLECT-FINAL suggestion 1).
 *
 * Boots the vite dev server, opens frontend/mock.html in headless Chrome
 * (playwright-core, system Chrome channel), asserts the workbench anchors
 * render (session pill, terminal host, SFTP pane, toolbar), functionally
 * exercises the batch-send dialog and the global quick-commands CRUD against
 * the mock bridge, and saves screenshots for the docs.
 *
 * Dependency policy: playwright-core is installed OUTSIDE the repo
 * (/tmp/dbx-ui-mock, same pattern as the A-LDAP walkthrough) — the project
 * package.json stays dependency-frozen. If playwright-core or Chrome is
 * unavailable the script SKIPs (exit 0), matching the smoke SKIP semantics.
 *
 * Usage: node scripts/smoke_ui_mock.mjs [--port 5199]
 */
import { spawn } from "node:child_process";
import { mkdirSync, existsSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const ROOT = new URL("..", import.meta.url).pathname;
// No --strictPort / fixed port: vite picks a free one and prints the URL.
const URL_BASE = `mock.html`;
const SHOT_DIR = `${ROOT}docs/screenshots-ui-mock`;

function skip(reason) {
  console.log(`SKIP: ${reason}`);
  process.exit(0);
}

// --- dependency gate (outside the repo) ---
let chromium;
try {
  ({ chromium } = await import("file:///tmp/dbx-ui-mock/node_modules/playwright-core/index.mjs"));
} catch {
  skip("playwright-core not available at /tmp/dbx-ui-mock (npm install --prefix /tmp/dbx-ui-mock playwright-core)");
}
const hasChrome = existsSync("/Applications/Google Chrome.app") || existsSync("/Applications/Chromium.app");
if (!hasChrome) skip("no system Chrome/Chromium");

// --- vite dev server ---
console.log("==> starting vite dev server");
const vite = spawn("pnpm", ["--dir", "frontend", "exec", "vite"], {
  cwd: ROOT,
  stdio: ["ignore", "pipe", "pipe"],
});
vite.stderr.on("data", (d) => process.stderr.write(d));
let stdoutBuf = "";
vite.stdout.on("data", (d) => {
  stdoutBuf += String(d);
});
let baseUrl = "";
const upDeadline = Date.now() + 60_000;
while (Date.now() < upDeadline) {
  const match = /(https?:\/\/(?:localhost|127\.0\.0\.1|\[::1\]):\d+)\//.exec(stdoutBuf);
  if (match) {
    baseUrl = `${match[1]}/mock.html`;
    break;
  }
  await sleep(500);
}
if (!baseUrl) skip("vite dev server did not report a URL in time");
console.log(`==> dev server up: ${baseUrl}`);

const failures = [];
async function expect(page, selector, label) {
  const el = page.locator(selector).first();
  try {
    await el.waitFor({ state: "visible", timeout: 15_000 });
    console.log(`  ok  ${label} (${selector})`);
  } catch {
    failures.push(`${label} (${selector}) not visible`);
    console.log(`  FAIL ${label} (${selector})`);
  }
}

async function expectText(page, selector, text, label) {
  const el = page.locator(selector, { hasText: text }).first();
  try {
    await el.waitFor({ state: "visible", timeout: 15_000 });
    console.log(`  ok  ${label} (${selector} ~ "${text}")`);
  } catch {
    failures.push(`${label}: "${text}" not visible in ${selector}`);
    console.log(`  FAIL ${label}: "${text}" not visible in ${selector}`);
  }
}

async function check(label, condition, detail = "") {
  if (condition) {
    console.log(`  ok  ${label}`);
  } else {
    failures.push(`${label}${detail ? `: ${detail}` : ""}`);
    console.log(`  FAIL ${label}${detail ? `: ${detail}` : ""}`);
  }
}

const browser = await chromium.launch({ channel: "chrome", headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const pageError = [];
  page.on("pageerror", (err) => pageError.push(String(err)));
  await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: 30_000 });
  await sleep(2_500); // let the mock bridge wire the workbench

  console.log("==> workbench anchors");
  await expect(page, ".session-pill", "session status pill");
  await expect(page, ".terminal-host", "terminal host");
  await expect(page, ".terminal-pane", "terminal pane");
  await expect(page, ".toolbar-actions", "toolbar actions");
  await expect(page, ".toolbar-separator", "toolbar separator");

  mkdirSync(SHOT_DIR, { recursive: true });
  await page.screenshot({ path: `${SHOT_DIR}/01-workbench.png`, fullPage: false });
  console.log(`  screenshot: docs/screenshots-ui-mock/01-workbench.png`);

  // --- global quick commands: add via the toolbar popover -----------------
  console.log("==> quick commands: global store add");
  await page.click('button[title="Quick commands"]');
  await expect(page, ".quick-commands-popover", "quick commands popover");
  await expectText(page, ".quick-command-global-hint", "Stored globally", "global-store hint");
  await page.fill(".quick-command-editor input:not(.mono)", "ui-mock cmd");
  await page.fill(".quick-command-editor input.mono", "echo ui-mock-batch");
  await page.click(".quick-command-editor .primary-button");
  await expectText(page, ".quick-command-row strong", "ui-mock cmd", "quick command row");
  await page.screenshot({ path: `${SHOT_DIR}/02-quick-commands.png`, fullPage: false });
  console.log(`  screenshot: docs/screenshots-ui-mock/02-quick-commands.png`);

  // --- batch send: dialog, target inventory, quick pick, send -------------
  console.log("==> batch send: dialog walkthrough");
  await page.click('button[title="Batch send"]');
  await expect(page, ".batch-modal", "batch send modal");
  await expectText(page, ".batch-target-row", "demo@server.demo.internal", "target row user@host");
  await expectText(page, ".batch-target-row", "Current", "current-session badge");
  await page.selectOption(".batch-quick-pick", { label: "ui-mock cmd" });
  const draft = await page.inputValue(".batch-modal input.mono");
  await check("quick pick fills the command draft", draft === "echo ui-mock-batch", `draft="${draft}"`);
  await page.screenshot({ path: `${SHOT_DIR}/03-batch-send.png`, fullPage: false });
  console.log(`  screenshot: docs/screenshots-ui-mock/03-batch-send.png`);
  await page.click(".batch-modal footer .primary-button");
  await expectText(page, ".batch-summary", "Sent to 1 session(s)", "batch send summary");
  try {
    // The mock bridge echoes the command into the terminal (PTY semantics).
    await page.waitForFunction(
      () => document.querySelector(".terminal-host")?.textContent?.includes("echo ui-mock-batch"),
      null,
      { timeout: 10_000 },
    );
    console.log('  ok  terminal echo ("echo ui-mock-batch")');
  } catch {
    failures.push('terminal echo missing ("echo ui-mock-batch")');
    console.log('  FAIL terminal echo ("echo ui-mock-batch")');
  }
  await page.click(".batch-modal header .icon-button");

  // --- global quick commands: delete --------------------------------------
  console.log("==> quick commands: delete");
  await page.click('button[title="Quick commands"]');
  await page.click(".quick-command-row button.icon-button:last-child");
  await expectText(page, ".quick-commands-popover .empty.compact", "No quick commands yet", "quick commands empty after delete");

  if (pageError.length) {
    failures.push(`page errors: ${pageError.slice(0, 3).join(" | ")}`);
  }

  if (failures.length) {
    console.error(`\nsmoke_ui_mock: ${failures.length} failure(s)`);
    process.exitCode = 1;
  } else {
    console.log("\nmock walkthrough: all green");
  }
} finally {
  await browser.close();
  vite.kill("SIGTERM");
}
