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
    const shelf = document.createElement('details'); shelf.id = 'merge-projects'; shelf.open = true;
    shelf.textContent = '历史合成图'; document.getElementById('strip').before(shelf);
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
        if (args.path === '/dataset') return {path:args.path,entries:[
          {name:'eer',path:'/dataset/eer',directory:true,size:0},
          {name:'mdocfolder',path:'/dataset/mdocfolder',directory:true,size:0},
          {name:'reference.gain',path:'/dataset/reference.gain',directory:false,supported:false,size:1024},
        ]};
        if (args.path === '/dataset/mdocfolder') return {path:args.path,entries:[{name:'series.mdoc',path:'/dataset/mdocfolder/series.mdoc',directory:false,supported:false,size:512,modified:1700000000}]};
        return { path: args.path, entries: [
          { name: "中文 folder", path: `${args.path}/中文 folder`, directory: true, size: 0, modified: 1700000000 },
          { ...image, path: `${args.path}/sample 01.mrc` },
          { ...image, name: "sample 02.mrc", path: `${args.path}/sample 02.mrc`, local: "/cache/sample 02.mrc" },
          { name: ".hidden", path: `${args.path}/.hidden`, directory: true, size: 0 },
          { name: "notes.txt", path: `${args.path}/notes.txt`, directory: false, supported: false, size: 32 },
          { name: 'process.py',path:`${args.path}/process.py`,directory:false,supported:false,size:32 },
          { name: 'run.sh',path:`${args.path}/run.sh`,directory:false,supported:false,size:32 },
        ] };
      }
      if (method === "download") {
        sshDownloads.push(args.path);
        return { path: image.local };
      }
      if (method === "thumbnail") return { url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j9S8AAAAASUVORK5CYII=" };
      if (method === 'readText') return {text:'[ZValue = 0]\nTiltAngle = 10\nSubFramePath = Z:\\old\\sample 01.mrc\n',truncated:false};
      if (method === 'resolveMovie') return {folder:'/data',path:'/data/sample 01.mrc'};
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
  // Dismiss only a genuine outside click; an inside-to-outside drag must stay open.
  const bounds = await page.locator('.ssh-dialog').boundingBox();
  await page.mouse.click(bounds.x + 12, bounds.y + 12);
  assert.equal(await page.locator('.ssh-dialog').evaluate(d => d.open), true);
  await page.mouse.move(bounds.x + 12, bounds.y + 12); await page.mouse.down();
  await page.mouse.move(2, 2); await page.mouse.up();
  assert.equal(await page.locator('.ssh-dialog').evaluate(d => d.open), true);
  await page.mouse.click(2, 2);
  assert.equal(await page.locator('.ssh-dialog').evaluate(d => d.open), false, 'outside click closes SSH dialog');
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
  await page.locator('.ssh-entry.image img').first().waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => sshDownloads.length), 0, 'preview does not download the original');
  assert.equal(await q("password").inputValue(), "");
  assert.equal(await page.evaluate(() => localStorage.getItem("plugin.ssh.recent").includes("fixture")), false);
  assert.equal(await page.locator(".ssh-entry").count(), 3);
  assert.equal(await page.getByRole('listitem',{name:'process.py',exact:true}).count(),0);
  assert.equal(await page.getByRole('listitem',{name:'run.sh',exact:true}).count(),0);
  await q("hidden").check(); assert.equal(await page.locator(".ssh-entry").count(), 4);
  await q("hidden").uncheck();
  await q("search").fill("中文"); assert.equal(await page.locator(".ssh-entry").count(), 1);
  await q("search").fill("");
  await page.getByRole("listitem", { name: "中文 folder，文件夹", exact: true }).dblclick();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="path"]').value === "/data/中文 folder");
  assert.equal(await page.evaluate(() => sshDownloads.length), 0, "directory browsing never downloads images");
  await q("up").click();
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="path"]').value === "/data");
  assert.equal(await page.getByRole("listitem", { name: "notes.txt", exact: true }).count(),0);
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
  await q('path').fill('/dataset'); await q('path').press('Enter');
  await page.waitForFunction(() => document.querySelector('.ssh-dialog [data-k="path"]').value === '/dataset');
  assert.equal(await q('load').isEnabled(), true, 'a directory with only subfolders/data must still load');
  await q('load').click();
  await page.locator('#strip .remote-entry').first().waitFor({state:'visible'});
  assert.equal(await page.locator('#strip .remote-folder svg').count(),2,'folder icons are SVGs');
  assert.equal(await page.locator('#merge-projects').isVisible(),false,'Pro project history must not obscure the active remote directory');
  assert.equal(await page.evaluate(() => sshDownloads.length), 0, 'mounting a directory never downloads its first image');
  assert.equal(await page.locator('#strip').getByRole('button', {name:'eer，文件夹',exact:true}).count(), 1);
  await page.locator('#strip').getByRole('button', {name:'eer，文件夹',exact:true}).click();
  await page.waitForFunction(() => S.folder.remotePath.endsWith('/dataset/eer'));
  await page.getByRole('button',{name:'↑ 上级目录',exact:true}).click();
  await page.locator('#strip').getByRole('button',{name:'mdocfolder，文件夹',exact:true}).click();
  await page.locator('#strip').getByRole('button',{name:'series.mdoc',exact:true}).click();
  await page.locator('.ssh-document pre').waitFor({state:'visible'});
  assert.match(await page.locator('.ssh-document pre').textContent(), /SubFramePath/);
  await page.getByRole('button',{name:'查找 sample 01.mrc',exact:true}).click();
  await page.waitForFunction(() => S.folder.remotePath === '/data' && S.folder.view.query === 'sample 01.mrc');
  assert.equal(await page.evaluate(() => sshDownloads.length),0,'MDOC lookup does not eagerly fetch movies');
  await page.locator('#plugin-buttons button[data-plugin="ssh"]').click();
  await q('path').fill('/data'); await q('path').press('Enter');
  await page.getByRole('listitem',{name:'sample 01.mrc',exact:true}).click();
  await q("load").click();
  await page.waitForFunction(() => sshOpened.length === 1);
  await page.locator('#strip .thumb .box img').waitFor({ state: 'visible' });
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
