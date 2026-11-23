"use strict";

const assert = require("node:assert/strict");

module.exports = async function checkLocalFolder(page) {
  await page.evaluate(() => {
    EDITION.pro = true;
    S.info = null;
    // Start with an SSH project mounted. The local toolbar must bypass its picker.
    S.folder = { dir: "ssh:test:/remote", remote: true, processing: {
      choose: () => { throw new Error("Local picker was sent to SSH"); },
    } };
    const native = T.core.invoke;
    window.folderCalls = [];
    window.localPick = "/local/project";
    T.core.invoke = async (command, args, options) => {
      folderCalls.push({ command, args });
      if (command === "pick_folder") return localPick;
      if (command === "list_directory") {
        if (args.path === "/local/project") return { parent: "/local", entries: [
          { name: "TS_01", path: "/local/project/TS_01", directory: true, supported: false, size: 0 },
          { name: "series.mdoc", path: "/local/project/series.mdoc", directory: false, supported: false, size: 512 },
        ] };
        if (args.path === "/local/project/TS_01") return { parent: "/local/project", entries: [] };
      }
      if (command === "tomo_session") {
        if (window.holdProjectScan && args.folder === "/local/project") {
          await new Promise(resolve => { window.releaseProjectScan = resolve; });
        }
        return {
          root: args.folder,
          series: args.folder === "/local/project" ? [{ name: "Tilt01", stage: "movies", movies: 6, done: [] }] : [],
          warnings: [], gains: [],
        };
      }
      return native(command, args, options);
    };
    const panel = document.createElement("section");
    panel.id = "cryoet-workspace";
    panel.innerHTML = '<div class="al-head"></div><div id="cryo-session"></div>';
    document.body.append(panel);
    window.TifMrcViewPro = { cryoet: { panel, busy: false, status() {}, name: basename } };
  });
  for (const file of ["rules.js", "session.js"]) {
    await page.addScriptTag({ url: new URL("/pro/cryoet/" + file, page.url()).href });
  }

  await page.locator("#btn-open").click();
  await page.getByText("打开本地文件夹…", { exact: true }).click({ timeout: 2500 });
  await page.locator("#strip").getByRole("button", { name: "TS_01，文件夹" }).waitFor();
  assert.match(await page.locator("#strip").textContent(), /series\.mdoc/);
  await page.waitForFunction(() => folderCalls.some(({ command, args }) => command === "tomo_session" && args.folder === "/local/project"));
  await page.locator(".cs-tile").filter({ hasText: "Tilt01" }).waitFor();
  assert.equal(await page.locator(".cs-path").getAttribute("title"), "/local/project");
  assert.equal(await page.evaluate(() => folderCalls.some(({ command }) => command === "open_image")), false);
  await page.evaluate(() => { window.holdProjectScan = true; });
  await page.locator("#strip").getByRole("button", { name: "刷新目录" }).click();
  await page.waitForFunction(() => window.releaseProjectScan);
  await page.locator("#strip").getByRole("button", { name: "TS_01，文件夹" }).click();
  await page.waitForFunction(() => folderCalls.some(({ command, args }) => command === "tomo_session" && args.folder === "/local/project/TS_01"));
  await page.evaluate(() => { window.holdProjectScan = false; window.releaseProjectScan(); });
  await page.locator(".cs-empty").filter({ hasText: /没有/ }).waitFor();
  assert.equal(await page.locator(".cs-tile").count(), 0);
  assert.equal(await page.locator(".cs-path").getAttribute("title"), "/local/project/TS_01");
  await page.locator("#strip").getByRole("button", { name: "↑ 上级目录" }).click();
  await page.locator("#strip").getByRole("button", { name: "TS_01，文件夹" }).waitFor();
  await page.evaluate(() => { localPick = null; });
  await page.locator("#btn-open").click();
  await page.getByText("打开本地文件夹…", { exact: true }).click();
  assert.equal(await page.locator("#folder-name").getAttribute("title"), "/local/project");
  console.log("PASS: Pro local toolbar bypasses SSH, browses a metadata-only project, and drives Cryo-ET discovery");
};
