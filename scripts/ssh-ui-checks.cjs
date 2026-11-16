"use strict";
// Runs inside the existing offline browser harness; real plugin and sidebar UI.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

module.exports = async function checkSsh(page) {
  await page.evaluate(() => {
    document.getElementById("plugin-dialog").hidden = true;
    window.sshCalls = [];
    window.sshDownloads = [];
    window.sshOpened = [];
    window.sshFailList = false;
    const image = { name: "sample 01.mrc", path: "/data/sample 01.mrc", local: "/cache/sample 01.mrc", directory: false, supported: true, size: 2048, modified: 1700000000 };
    window.mockSshCall = async (method, args) => {
      sshCalls.push({ method, args });
      if (method === "probe") return { connection: "test-connection", fingerprint: "SHA256:testFingerprintForOfflineUI", needsTrust: true };
      if (method === "authenticate") {
        if (args.authMethod === "keyFile") {
          if (args.keyPath !== "/fixtures/private_key" || args.passphrase !== "fixture-key-passphrase" || "password" in args) throw new Error("密钥登录参数错误");
        } else if (args.password !== "fixture") throw new Error("身份验证失败");
        return { path: "/data", fingerprint: "SHA256:testFingerprintForOfflineUI" };
      }
      if (method === "list") {
        if (window.sshFailList) throw new Error("无法访问路径：权限不足");
        return { path: args.path, entries: [
          { name: "中文 folder", path: `${args.path}/中文 folder`, directory: true, size: 0, modified: 1700000000 },
          { ...image, path: `${args.path}/sample 01.mrc` },
          { name: ".hidden", path: `${args.path}/.hidden`, directory: true, size: 0 },
          { name: "notes.txt", path: `${args.path}/notes.txt`, directory: false, supported: false, size: 32 },
        ] };
      }
      if (method === "download") {
        sshDownloads.push(args.path);
        return { path: image.local };
      }
      return null;
    };
    window.mockSshOpen = async ({ path }) => {
      sshOpened.push(path);
      throw new Error("decoder sentinel: browser transport fixture has no image decoder");
    };
    pluginHost.native.push("ssh");
    const plugin = pluginHost.plugins.get("ssh");
    deactivatePlugin(plugin); activatePlugin(plugin);
  });
  const q = (key) => page.locator(`.ssh-dialog [data-k="${key}"]`);
  const checkTheme = async (expected) => {
    const colors = await page.evaluate(() => ({
      button: getComputedStyle(document.querySelector('.ssh-dialog [data-k="load"]')).backgroundColor,
      icon: getComputedStyle(document.querySelector('.ssh-dialog .ssh-symbol')).color,
      toolbar: getComputedStyle(document.querySelector('.ssh-toolbar svg')).color,
      image: document.querySelector('.ssh-entry.image svg') && getComputedStyle(document.querySelector('.ssh-entry.image svg')).color,
      progress: getComputedStyle(document.querySelector('.ssh-transfer progress')).accentColor,
    }));
    for (const [part, color] of Object.entries(colors)) if (color) assert.equal(color, expected, `${part} must inherit the edition theme`);
  };
  await page.locator('#plugin-buttons button[data-plugin="ssh"]').click();
  await q("host").fill("127.0.0.1"); await q("username").fill("test"); await q("password").fill("fixture");
  await checkTheme("rgb(9, 105, 218)");
  if (process.env.TMV_SSH_SCREENSHOT_DIR) {
    fs.mkdirSync(process.env.TMV_SSH_SCREENSHOT_DIR, { recursive: true });
    await page.locator(".ssh-dialog").screenshot({ path: path.join(process.env.TMV_SSH_SCREENSHOT_DIR, "connect.png") });
  }
  await q("connect").click();
  await q("trust").waitFor({ state: "visible" });
  assert.equal(await page.evaluate(() => sshCalls.filter((c) => c.method === "authenticate").length), 0, "no password sent before trust approval");
  await q("trust-no").click();
  await q("connect").waitFor({ state: "visible" });
  await page.waitForFunction(() => !document.querySelector('.ssh-dialog [data-k="connect"]').disabled);
  await q("password").fill("fixture"); await q("connect").click(); await q("trust-yes").click();
  await q("list-wrap").waitFor({ state: "visible" });
  assert.equal(await q("password").inputValue(), "");
  assert.equal(await page.evaluate(() => localStorage.getItem("plugin.ssh.recent").includes("fixture")), false);
  assert.equal(await page.locator(".ssh-entry").count(), 3);
  await q("hidden").check(); assert.equal(await page.locator(".ssh-entry").count(), 4);
  await q("hidden").uncheck();
  await q("search").fill("中文"); assert.equal(await page.locator(".ssh-entry").count(), 1);
  await q("search").fill("");
  await page.getByRole("listitem", { name: "中文 folder，文件夹", exact: true }).dblclick();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="path"]').value === "/data/中文 folder");
  assert.equal(await page.evaluate(() => sshDownloads.length), 0, "directory browsing never downloads images");
  await q("up").click();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="path"]').value === "/data");
  await page.getByRole("listitem", { name: "notes.txt", exact: true }).click();
  assert.equal(await q("load").isDisabled(), true);
  await q("search").fill(""); // clear selection
  await page.evaluate(() => { window.sshFailList = true; });
  await q("path").fill("/denied"); await q("path").press("Enter");
  await q("error").waitFor({ state: "visible" });
  assert.equal(await q("path").inputValue(), "/data");
  await page.evaluate(() => { window.sshFailList = false; });
  await q("refresh").click();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="error"]').hidden);
  if (process.env.TMV_SSH_SCREENSHOT_DIR) await page.locator(".ssh-dialog").screenshot({ path: path.join(process.env.TMV_SSH_SCREENSHOT_DIR, "browser.png") });
  // Use the real edition stylesheet without bootstrapping mocked Pro backends.
  await page.addStyleTag({ url: new URL("/pro/pro.css", page.url()).href });
  await page.evaluate(() => { document.documentElement.dataset.edition = "pro"; });
  await checkTheme("rgb(231, 19, 22)");
  if (process.env.TMV_SSH_SCREENSHOT_DIR) await page.locator(".ssh-dialog").screenshot({ path: path.join(process.env.TMV_SSH_SCREENSHOT_DIR, "browser-pro.png") });
  await page.setViewportSize({ width: 680, height: 720 });
  const loadBounds = await q("load").boundingBox();
  const modalBounds = await page.locator(".ssh-dialog").boundingBox();
  assert(loadBounds.x >= modalBounds.x && loadBounds.x + loadBounds.width <= modalBounds.x + modalBounds.width, "load action remains within the narrow dialog");
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.evaluate(() => { delete document.documentElement.dataset.edition; });
  await q("load").click();
  await page.waitForFunction(() => sshOpened.length === 1);
  assert.deepEqual(await page.evaluate(() => sshDownloads), ["/data/sample 01.mrc"]);
  assert.deepEqual(await page.evaluate(() => sshOpened), ["/cache/sample 01.mrc"]);
  assert.equal(await page.evaluate(() => S.folder.remote), true);
  assert.equal(await page.evaluate(() => ext.remoteFiles.get("/cache/sample 01.mrc").ready), true);
  await page.locator('#plugin-buttons button[data-plugin="ssh"]').click();
  await q("disconnect").click();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="connected"]').hidden);
  await q("auth-method").selectOption("keyFile");
  assert.equal(await q("password").isVisible(), false);
  await q("pick-key").click();
  assert.equal(await q("key-path").inputValue(), "/fixtures/private_key");
  await q("passphrase").fill("fixture-key-passphrase");
  await q("connect").click(); await q("trust-yes").click();
  await q("list-wrap").waitFor({ state: "visible" });
  assert.equal(await q("passphrase").inputValue(), "");
  assert.equal(await page.evaluate(() => localStorage.getItem("plugin.ssh.recent").includes("fixture-key-passphrase")), false);
  await q("disconnect").click();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="connected"]').hidden);
  await page.evaluate(() => deactivatePlugin(pluginHost.plugins.get("ssh")));
  assert.equal(await page.locator(".ssh-dialog").count(), 0);
  assert.equal(await page.locator('#plugin-buttons button[data-plugin="ssh"]').count(), 0);
  assert.equal(await page.evaluate(() => ext.remoteFiles.size), 0);
  console.log("PASS: SSH trust gate, no password persistence, directory navigation/filtering, read errors, lazy sidebar loading, blue/red edition themes and plugin cleanup");
};
