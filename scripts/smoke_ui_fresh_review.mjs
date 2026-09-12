#!/usr/bin/env node
// Round4: exercise failed reads, alert retries and keyboard routing in the real
// Vue/xterm page. Uses the same optional browser dependency as smoke_ui_mock.mjs.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const root = new URL("..", import.meta.url).pathname;
let chromium;
try {
  ({ chromium } = await import("file:///tmp/dbx-ui-mock/node_modules/playwright-core/index.mjs"));
} catch {
  console.log("SKIP: existing /tmp/dbx-ui-mock playwright-core is unavailable");
  process.exit(0);
}
if (!existsSync("/Applications/Google Chrome.app")) {
  console.log("SKIP: system Chrome is unavailable");
  process.exit(0);
}

let vite;
let browser;
try {
  let url = process.env.DBX_SSH_MOCK_URL;
  if (!url) {
    vite = spawn("pnpm", ["--dir", "frontend", "exec", "vite", "--host", "127.0.0.1"], { cwd: root });
    let output = "";
    vite.stdout.on("data", data => { output += data; });
    vite.stderr.on("data", data => process.stderr.write(data));
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline && !url) {
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+\//);
      if (match) url = `${match[0]}mock.html`;
      else await sleep(100);
    }
  }
  assert.ok(url, "Vite must start");
  assert.match(url, /^http:\/\/(127\.0\.0\.1|localhost):\d+\/mock\.html$/, "mock page only");
  browser = await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  const shots = `${root}docs/screenshots-ui-mock`;
  mkdirSync(shots, { recursive: true });
  const reload = async () => {
    await page.goto(url);
    await page.waitForFunction(() => document.querySelector(".session-pill")?.textContent?.includes("Connected"));
  };

  await reload();
  await page.evaluate(() => {
    const base = window.dbxPlugin.invoke;
    window.__freshReview = { writes: 0, pending: true };
    window.dbxPlugin.invoke = (method, ...args) => {
      if (method === "ssh/settings/get" && window.__freshReview.pending) {
        return new Promise((_resolve, reject) => { window.__freshReview.reject = reject; });
      }
      if (method === "ssh/settings/set") window.__freshReview.writes++;
      return base(method, ...args);
    };
  });
  await page.locator('button[title="SSH settings"]').click();
  const save = page.locator(".settings-modal > footer .primary-button");
  assert.equal(await save.isDisabled(), true, "pending settings cannot be saved");
  await page.evaluate(() => window.__freshReview.reject(new Error("fixture read failure")));
  await page.locator('.settings-modal [role="alert"]').waitFor();
  assert.equal(await save.isDisabled(), true, "failed settings cannot be saved");
  assert.equal(await page.locator(".settings-modal .credential-source-row").count(), 0, "failed read must not expose stale connection settings");
  await save.dispatchEvent("click");
  assert.equal(await page.evaluate(() => window.__freshReview.writes), 0, "handler guards writes even on synthetic clicks");
  await page.screenshot({ path: `${shots}/round4-settings-failed.png` });
  await page.evaluate(() => { window.__freshReview.pending = false; });
  await page.locator('.settings-modal [role="alert"] button').click();
  await page.locator(".settings-modal .credential-source-row").waitFor();
  assert.equal(await save.isDisabled(), false);
  await save.click();
  assert.equal(await page.evaluate(() => window.__freshReview.writes), 1, "retry restores normal saving");
  console.log("PASS settings: loading / failure block writes; retry restores editing and saving");

  await reload();
  await page.locator('button[title="Alert triage"]').click();
  const payload = page.locator(".alert-triage-payload");
  const analyze = page.locator(".alert-triage-modal .primary-button");
  await payload.fill("Disk space is full");
  await analyze.click();
  await page.locator(".alert-triage-result").waitFor();
  await page.evaluate(() => {
    const base = window.dbxPlugin.invoke;
    window.__freshReview = { pending: true };
    window.dbxPlugin.invoke = (method, ...args) => {
      if (method === "ssh/alert/triage" && window.__freshReview.pending) {
        return new Promise((_resolve, reject) => { window.__freshReview.reject = reject; });
      }
      return base(method, ...args);
    };
  });
  await payload.fill("CPU load is high");
  assert.equal(await page.locator(".alert-triage-result").count(), 0, "editing invalidates the old result");
  await analyze.click();
  await page.locator('.alert-triage-modal [role="status"]').waitFor();
  assert.equal(await payload.isDisabled(), true, "in-flight payload stays paired with its response");
  await page.evaluate(() => window.__freshReview.reject(new Error("fixture analysis failure")));
  await page.locator('.alert-triage-modal [role="alert"]').waitFor();
  assert.equal(await page.locator(".alert-triage-result").count(), 0);
  assert.match(await page.locator('.alert-triage-modal [role="alert"]').innerText(), /Could not analyze this alert/);
  assert.equal(await payload.isDisabled(), false);
  assert.equal(await analyze.isDisabled(), false);
  await page.screenshot({ path: `${shots}/round4-alert-failed.png` });
  await page.evaluate(() => { window.__freshReview.pending = false; });
  await analyze.click();
  await page.locator(".alert-triage-result").waitFor();
  assert.equal(await page.locator(".alert-category").innerText(), "CPU");
  assert.equal(await page.locator('.alert-triage-modal [role="alert"]').count(), 0);
  console.log("PASS alerts: editing clears stale results; pending / failure / retry stay in the dialog");

  await reload();
  for (const [key, modifiers] of [["f", { ctrlKey: true }], ["Escape", {}], ["f", { metaKey: true }], ["Escape", {}], ["0", { ctrlKey: true }], ["v", { metaKey: true }], ["V", { ctrlKey: true, shiftKey: true }]]) {
    const result = await page.evaluate(({ key, modifiers }) => {
      let bubbled = false;
      const listener = () => { bubbled = true; };
      document.addEventListener("keydown", listener);
      const event = new KeyboardEvent("keydown", { key, ...modifiers, bubbles: true, cancelable: true });
      document.querySelector(".xterm-helper-textarea").dispatchEvent(event);
      document.removeEventListener("keydown", listener);
      return { cancelled: event.defaultPrevented, bubbled };
    }, { key, modifiers });
    assert.deepEqual(result, { cancelled: true, bubbled: false }, `owned shortcut ${key}`);
  }
  await page.evaluate(() => {
    const base = window.dbxPlugin.sendBinary;
    window.__freshReview = { input: [] };
    window.dbxPlugin.sendBinary = (channel, data) => {
      if (channel.startsWith("ssh/terminal/in/")) {
        const bytes = typeof data === "string" ? window.dbxPlugin.decodeBase64(data) : new Uint8Array(data);
        window.__freshReview.input.push(...bytes.slice(8));
      }
      return base(channel, data);
    };
  });
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Control+c");
  await page.keyboard.press("Tab");
  await page.waitForFunction(() => window.__freshReview.input.includes(3) && window.__freshReview.input.includes(9));
  console.log("PASS keyboard: owned shortcuts cancel defaults and bubbling; Ctrl+C / Tab still reach PTY");
  assert.deepEqual(errors, [], "no unhandled page errors");
  console.log("PASS round4 fresh review UI smoke");
} finally {
  await browser?.close();
  vite?.kill("SIGTERM");
}
