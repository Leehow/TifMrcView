"use strict";

// No installs or browser downloads. Run with an existing require('playwright').
// TMV_TEST_CHROMIUM may select an already-cached binary if revisions differ.
const assert = require("node:assert/strict");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const vm = require("node:vm");
const root = path.resolve(__dirname, "..");
const ui = path.join(root, "ui");
const read = (file) => fs.readFileSync(file, "utf8");
const sample = JSON.parse(read(path.join(ui, "sandbox/test/sample/plugin.json")));
const legacy = fs.readdirSync(path.join(root, "plugins")).filter((id) => fs.existsSync(path.join(root, "plugins", id, "plugin.json")))
  .map((id) => JSON.parse(read(path.join(root, "plugins", id, "plugin.json"))));
const packages = [...legacy, sample].map((manifest) => ({ id: manifest.id, main: manifest.main || "index.js", manifest }));
const conf = JSON.parse(read(path.join(root, "src-tauri/tauri.conf.json")));
const translations = vm.runInNewContext(read(path.join(ui, "plugins/i18n.js")) + "; ({WORDS, LANGS})", { TifMrcView: { register() {} } });
let origin, browser;
const requests = [];
const server = http.createServer((req, res) => {
  requests.push(req.url);
  const url = new URL(req.url, origin);
  let file;
  if (url.pathname === "/site/plugins/permission-test.html") {
    res.setHeader("Content-Type", "text/html");
    res.end('<!doctype html><meta charset="utf-8"><div id="plugins"></div><script src="/site/plugins/market.js"></script>');
    return;
  }
  if (url.pathname.startsWith("/site/")) file = path.join(root, url.pathname);
  else if (url.pathname === "/tmvplugin/__sandbox/runtime.js") file = path.join(ui, "sandbox/tmv-plugin-api.js");
  else if (url.pathname.startsWith("/tmvplugin/__sandbox/")) {
    res.writeHead(404).end(); return;
  } else if (url.pathname.startsWith("/tmvplugin/")) {
    const rel = url.pathname.slice("/tmvplugin/".length);
    file = rel.startsWith(sample.id + "/") ? path.join(ui, "sandbox/test/sample", rel.slice(sample.id.length + 1)) : path.join(root, "plugins", rel);
  } else file = path.join(ui, url.pathname === "/" ? "index.html" : url.pathname);
  if (!file.startsWith(root + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) { res.writeHead(404).end(); return; }
  const ext = path.extname(file);
  res.setHeader("Content-Type", ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json" })[ext] || "application/octet-stream");
  if (ext === ".html") {
    // Same policy as production, with custom-protocol URLs mapped to the test
    // server. In particular, do NOT add unsafe-inline to script-src.
    res.setHeader("Content-Security-Policy", conf.app.security.csp
      .replaceAll("tmvplugin:", origin).replaceAll("http://tmvplugin.localhost", origin));
  }
  let body = fs.readFileSync(file);
  if (file === path.join(ui, "plugin-host.js")) {
    // Only transport is replaced; registry, makeApi, lifecycle and UI are real.
    body = body.toString().replace('return scheme + path.split("/")', `return ${JSON.stringify(origin + "/tmvplugin/")} + path.split("/")`);
  }
  res.end(body);
});

(async () => {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch(process.env.TMV_TEST_CHROMIUM ? { executablePath: process.env.TMV_TEST_CHROMIUM } : undefined);
  const page = await browser.newPage();
  const errors = [], external = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  // Fail any attempted outbound request without ever touching the Internet.
  await page.route("**/*", (route) => {
    if (!route.request().url().startsWith(origin + "/")) {
      external.push(route.request().url()); return route.abort();
    }
    return route.continue();
  });
  await page.addInitScript(({ packages }) => {
    if (window.top !== window) return;
    localStorage.setItem("update.auto", "false");
    localStorage.setItem("plugin.i18n.lang", '"zh"');
    window.mockWrites = [];
    window.mockSavePaths = ["/chosen/sandbox.png"];
    window.mockCalls = [];
    window.mockInstalls = [];
    window.mockZip = { id: "confirm-v2", name: "Permission demo", version: "2.1.0", apiVersion: 2,
      permissions: ["state", "ui", "metadata", "export", "settings"], description: "Confirm before install" };
    window.mockMarket = [
      { entry: window.mockZip, state: "install" },
      { entry: { id: "confirm-none", name: "No permissions", version: "1.0.0", apiVersion: 2, permissions: [] }, state: "install" },
      { entry: { id: "recorder", name: "Official legacy", version: "1.0.0" }, state: "update", installed: "0.9.0" },
    ];
    const listen = async () => () => {};
    window.__TAURI__ = {
      core: { invoke: async (command, args, options) => {
        window.mockCalls.push(command);
        switch (command) {
          case "plugin_scan":
            S.info = { path: "/test/image.tif", name: "image.tif", width: 8, height: 8, sizeZ: 3, sizeT: 2, sizeC: 1, dtype: "u8", format: "TIFF", metadata: [] };
            S.chans = [{ visible: true, color: [255, 255, 255], min: 0, max: 255 }];
            return { native: ["recorder"], dir: "/mock/plugins", packages, user: [] };
          case "plugin_market_list": return window.mockMarket;
          case "take_install_request": { const req = window.mockRequest; window.mockRequest = null; return req; }
          case "pick_plugin_package": return "/test/demo.zip";
          case "plugin_inspect_package": return { manifest: window.mockZip, sha256: "approved-digest" };
          case "plugin_install":
          case "plugin_install_from_market": window.mockInstalls.push({ command, args }); return window.mockZip;
          case "histogram": return { bins: new Array(256).fill(0), min: 0, max: 255 };
          case "pick_save_path": return window.mockSavePaths.shift() ?? null;
          case "save_file": window.mockWrites.push({ path: decodeURIComponent(options.headers["x-path"]), bytes: Array.from(args) }); return;
          case "render_view": {
            const { outW: w, outH: h } = args.req;
            const buffer = new ArrayBuffer(28 + w * h * 4);
            const view = new DataView(buffer);
            view.setUint32(0, w, true); view.setUint32(4, h, true); view.setUint32(8, 1, true);
            view.setFloat32(16, 255, true); view.setFloat32(24, 255, true);
            new Uint8Array(buffer, 28).fill(255);
            return buffer;
          }
          default: return null;
        }
      } },
      app: { getVersion: async () => "0.5.0" },
      event: { listen },
      window: { getCurrentWindow: () => ({ setTitle: async () => {} }) },
      webview: { getCurrentWebview: () => ({ onDragDropEvent: listen }) },
      webviewWindow: { getCurrentWebviewWindow: () => ({ label: "main", listen }) },
    };
    window.__TAURI_INTERNALS__ = { invoke: window.__TAURI__.core.invoke };
  }, { packages });
  await page.goto(origin);
  await page.waitForFunction(() => {
    const rec = pluginHost.plugins.get("sandbox-sample");
    return rec && (!rec.active || S.pluginMeta.some((g) => g.name === "probe:permission")) && window.mockWrites.length;
  });
  await page.waitForFunction(() => pluginHost.plugins.get("sandbox-sample")?.active && !pluginHost.plugins.get("sandbox-sample")?.error);
  const state = await page.evaluate(() => ({
    plugins: [...pluginHost.plugins.values()].map((r) => ({ id: r.def.id, active: r.active, error: r.error })),
    meta: S.pluginMeta, writes: window.mockWrites,
  }));
  assert.equal(state.plugins.length, packages.length + 1); // includes built-in i18n
  assert(state.plugins.every((p) => p.active && !p.error), JSON.stringify(state.plugins));
  const probes = state.meta.filter((g) => g.name.startsWith("probe:"));
  assert.equal(probes.length, 10);
  assert(probes.every((g) => g.items[0][1] === "true"), JSON.stringify(probes));
  assert(state.meta.some((g) => g.name === "metadata-write"));
  assert.equal(state.writes.length, 1);
  assert.equal(state.writes[0].path, "/chosen/sandbox.png");
  assert.deepEqual(state.writes[0].bytes.slice(0, 8), [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(await page.locator('[data-plugin-sandbox="sandbox-sample"]').getAttribute("sandbox"), "allow-scripts");
  assert.equal(await page.locator('#plugin-buttons [data-plugin="sandbox-sample"]').count(), 1);
  const panel = page.locator('#tools section[data-plugin="sandbox-sample"]');
  assert.equal(await panel.locator("h3").textContent(), "Sandbox panel");
  await page.getByRole("button", { name: "Sandbox test", exact: true }).click();
  await panel.getByRole("button", { name: "Panel action" }).click();
  await panel.locator('input[type="number"]').fill("5");
  await panel.locator('input[type="number"]').dispatchEvent("change");
  await panel.locator("select").selectOption("b");
  await panel.locator('input[type="checkbox"]').uncheck();
  await page.evaluate(() => TifMrcView.emit("slice", { z: 2, t: 1 }));
  await page.waitForFunction(() => ["toolbar-click", "panel-click", "number-change", "select-change", "checkbox-change", "slice-event", "open-event"].every((name) => S.pluginMeta.some((g) => g.name === name)));
  await page.locator("#btn-plugins").click();
  await page.waitForFunction(() => market.list !== null);
  assert.equal(await page.locator("#plugin-dialog").isVisible(), true);
  await page.locator("#update-check").click();
  await page.waitForFunction(() => document.getElementById("update-status").textContent === "已是最新版本");
  await page.locator("#plugin-close").click();
  const frame = await page.locator('[data-plugin-sandbox="sandbox-sample"]').elementHandle().then((el) => el.contentFrame());
  assert.equal(await frame.evaluate(() => location.href), "about:srcdoc", "sandbox document must have a non-local URL");
  assert.equal(await frame.evaluate(() => location.origin), "null");
  assert.equal(await page.locator('[data-plugin-sandbox="sandbox-sample"]').getAttribute("src"), null);
  const pluginReadsBefore = requests.filter((url) => url.startsWith("/tmvplugin/")).length;
  const crossPlugin = await frame.evaluate(async (origin) => {
    const blocked = async (url) => { try { await fetch(url); return false; } catch (_) { return true; } };
    const ownFetch = await blocked(origin + "/tmvplugin/sandbox-sample/index.js");
    const otherFetch = await blocked(origin + "/tmvplugin/recorder/index.js");
    const otherScript = await new Promise((resolve) => {
      const script = document.createElement("script");
      script.src = origin + "/tmvplugin/recorder/index.js";
      script.onerror = () => resolve(true);
      script.onload = () => resolve(false);
      document.head.append(script);
    });
    return { ownFetch, otherFetch, otherScript };
  }, origin);
  assert.deepEqual(crossPlugin, { ownFetch: true, otherFetch: true, otherScript: true });
  assert.equal(requests.filter((url) => url.startsWith("/tmvplugin/")).length, pluginReadsBefore, "narrowed child CSP must block reads/scripts before transport");
  const escaped = await page.evaluate(() => {
    const html = sandboxDocument('demo"<&', 'a"<&.js');
    const doc = new DOMParser().parseFromString(html, "text/html");
    return { id: doc.documentElement.dataset.plugin, main: doc.documentElement.dataset.main, scripts: doc.scripts.length, policy: doc.querySelector('meta[http-equiv="Content-Security-Policy"]').content };
  });
  assert.equal(escaped.id, 'demo"<&');
  assert(escaped.main.endsWith("/demo%22%3C%26/a%22%3C%26.js"));
  assert.equal(escaped.scripts, 1);
  assert(escaped.policy.startsWith("default-src 'none'; script-src "));
  const failures = await frame.evaluate(async () => {
    const reject = async (fn) => { try { await fn(); return "ALLOWED"; } catch (e) { return e.message; } };
    return {
      arbitrary: await reject(() => testApp.fs.write("/arbitrary", new Uint8Array([1]))),
      reuse: await reject(() => testApp.fs.write("/chosen/sandbox.png", new Uint8Array([1]))),
      oversized: await reject(() => testApp.ui.toast("x".repeat(1024 * 1024))),
      invalid: await reject(() => testApp.renderRegion([0, 0, 1, 1], { size: [99999, 99999] })),
    };
  });
  assert(Object.values(failures).every((e) => e !== "ALLOWED"), JSON.stringify(failures));
  // Test raw adversarial RPCs too, independent of the friendly client wrapper.
  const rpcErrors = await frame.evaluate(() => new Promise((resolve) => {
    const replies = [];
    const listener = ({ data }) => {
      if (![900001, 900002, 900003, 900004].includes(data.id)) return;
      replies.push(data);
      if (replies.length === 4) { testPort.removeEventListener("message", listener); resolve(replies); }
    };
    testPort.addEventListener("message", listener);
    testPort.postMessage({ id: 900001, method: "native", args: ["begin"] });
    testPort.postMessage({ id: 900002, method: "settings.set", args: ["x", true] });
    testPort.postMessage({ id: 900003, method: "state", args: {} });
    testPort.postMessage({ id: 900004, method: "__proto__", args: [] });
  }));
  assert(rpcErrors.some((r) => /Unknown method/.test(r.error)));
  assert(rpcErrors.some((r) => /Undeclared permission/.test(r.error)));
  assert(rpcErrors.every((r) => typeof r.error === "string"));
  // Cancellation and a later dialog revoke the previous path; a different
  // plugin's selected path must never authorize this frame to write.
  await page.evaluate(() => { window.mockSavePaths = ["/chosen/first.png", "/chosen/second.png", null]; });
  const saveChecks = await frame.evaluate(async () => {
    const rejected = async (path) => { try { await testApp.fs.write(path, new Uint8Array([1])); return false; } catch (_) { return true; } };
    const first = await testApp.dialog.save({ name: "first.png" });
    const second = await testApp.dialog.save({ name: "second.png" });
    const old = await rejected(first);
    await testApp.dialog.save({ name: "cancel.png" });
    return { old, cancelled: await rejected(second) };
  });
  assert.deepEqual(saveChecks, { old: true, cancelled: true });
  // Namespacing: grant settings in the test only, then exercise the real map.
  await page.evaluate(async () => {
    const rec = pluginHost.plugins.get("sandbox-sample");
    deactivatePlugin(rec);
    rec.def.permissions.push("settings");
  });
  assert.equal(await page.locator('[data-plugin-sandbox="sandbox-sample"]').count(), 0);
  assert.equal(await page.locator('[data-plugin="sandbox-sample"]').count(), 0);
  await page.evaluate(async () => { await activatePlugin(pluginHost.plugins.get("sandbox-sample")); });
  const restarted = await page.locator('[data-plugin-sandbox="sandbox-sample"]').elementHandle().then((el) => el.contentFrame());
  await restarted.evaluate(async () => { await testApp.settings.set("answer", 42); });
  assert.equal(await restarted.evaluate(() => testApp.settings.get("answer", 0)), 42);
  assert.equal(await page.evaluate(() => localStorage.getItem("plugin.sandbox-sample.answer")), "42");
  assert.equal(await page.evaluate(() => localStorage.getItem("answer")), null);
  // No calls may arrive through the window message listener, even with valid RPC shape.
  await restarted.evaluate(() => parent.postMessage({ id: 9999, method: "ui.toast", args: ["forged"] }, "*"));
  await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 50)));
  assert.notEqual(await page.locator("#toast").textContent(), "forged");
  // Parent frame-src also blocks a plugin navigating itself to another origin
  // or to the app's privileged document (frame-src forbids URL navigation).
  await restarted.evaluate(() => { location.href = "https://example.com/self-navigation"; });
  await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 100)));
  const rootsBefore = requests.filter((url) => url === "/").length;
  await restarted.evaluate((url) => { location.href = url; }, origin + "/");
  await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 100)));
  assert.equal(requests.filter((url) => url === "/").length, rootsBefore, "Parent frame-src must block privileged app navigation");
  assert.deepEqual(external, [], "CSP must block outbound requests before transport");
  // A disabled API 2 package must not execute until explicitly enabled.
  await page.evaluate(() => setPluginEnabled("sandbox-sample", false));
  await page.reload();
  await page.waitForFunction((count) => pluginHost.plugins.size === count, packages.length + 1);
  assert.equal(await page.locator("[data-plugin-sandbox]").count(), 0);
  assert.equal(await page.evaluate(() => pluginHost.plugins.get("sandbox-sample").active), false);
  await page.evaluate(async () => {
    const rec = pluginHost.plugins.get("sandbox-sample");
    setPluginEnabled(rec.def.id, true);
    await activatePlugin(rec);
  });
  assert.equal(await page.locator('[data-plugin-sandbox="sandbox-sample"]').count(), 1);
  assert.equal(await page.evaluate(() => pluginHost.plugins.get("sandbox-sample").error), null);
  const mainsBefore = requests.filter((url) => url === "/tmvplugin/sandbox-sample/index.js").length;
  // Inject only into subsequent frames, after the normal isolation checks.
  // Chromium mocks the rejection; it cannot validate native Tauri's decision.
  await page.addInitScript(() => {
    if (window.top === window) return;
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: Object.freeze({
      invoke: async () => { throw new Error("IPC denied: no remote capability"); },
    }) });
    window.__TAURI__ = { core: window.__TAURI_INTERNALS__ };
  });
  await page.evaluate(async () => {
    const rec = pluginHost.plugins.get("sandbox-sample");
    deactivatePlugin(rec);
    await activatePlugin(rec);
  });
  assert.equal(await page.evaluate(() => pluginHost.plugins.get("sandbox-sample").error), null);
  assert.equal(requests.filter((url) => url === "/tmvplugin/sandbox-sample/index.js").length, mainsBefore + 1);
  assert.equal(await page.locator('[data-plugin-sandbox="sandbox-sample"]').count(), 1);
  const injected = await page.locator('[data-plugin-sandbox="sandbox-sample"]').elementHandle().then((el) => el.contentFrame());
  assert.equal(await injected.evaluate(() => window.__TAURI__ === undefined), true, "configurable globals are removed");
  assert.equal(await injected.evaluate(async () => {
    try { await window.__TAURI_INTERNALS__.invoke("plugin_market_list"); return false; }
    catch (e) { return /IPC denied/.test(String(e)); }
  }), true, "surviving nonconfigurable globals must not prevent activation");
  console.log("PASS: simulated WebView2 injection permits API 2 activation with surviving native APIs denied by mock backend");
  // Exercise the real confirmation UI with mocked native transport only.
  // Plugin loading itself was exercised above; record successful installs here
  // to avoid reloading the page when an installed fixture is updated.
  await page.evaluate(() => {
    window.confirmedInstalls = [];
    afterInstall = async (m) => window.confirmedInstalls.push(m.id);
    togglePluginDialog(true);
  });
  await page.waitForFunction(() => market.list?.length === 3);
  const permissionTexts = ["读取当前图像的信息并切换层和帧", "添加工具栏按钮和面板", "给图像添加元数据并设置像素尺寸和格式", "导出图片到你选择的位置", "保存自己的设置"];
  assert.deepEqual(await page.locator("#market-list .market-row").first().locator(".pl-permissions li").allTextContents(), permissionTexts);
  // In-app market install buttons also pass through confirmation.
  await page.locator("#market-list .market-row").first().getByRole("button", { name: "安装", exact: true }).click();
  await page.waitForFunction(() => !document.getElementById("mc-install").disabled);
  assert.equal(await page.locator("#mc-api").textContent(), "API 2");
  assert.deepEqual(await page.locator("#mc-permissions li").allTextContents(), permissionTexts);
  assert.equal(await page.evaluate(() => mockInstalls.length), 0);
  await page.locator("#mc-cancel").click();
  assert.equal(await page.locator("#market-confirm").isHidden(), true);
  assert.equal(await page.evaluate(() => mockInstalls.length), 0);
  // tifmrcview:// uses the same card; nothing installs while it is open.
  await page.evaluate(async () => {
    mockRequest = { id: "confirm-v2", version: "2.1.0" };
    await takeInstallRequest();
  });
  await page.waitForFunction(() => !document.getElementById("mc-install").disabled);
  assert.match(await page.locator("#mc-title").textContent(), /Permission demo.*2\.1\.0/);
  assert.deepEqual(await page.locator("#mc-permissions li").allTextContents(), permissionTexts);
  await page.locator("#mc-install").click();
  await page.waitForFunction(() => mockInstalls.length === 1 && document.getElementById("market-confirm").hidden);
  const marketInstall = await page.evaluate(() => mockInstalls[0]);
  assert.equal(marketInstall.command, "plugin_install_from_market");
  assert.equal(marketInstall.args.approval.apiVersion, 2);
  assert.deepEqual(marketInstall.args.approval.permissions, ["state", "ui", "metadata", "export", "settings"]);
  await page.evaluate(() => confirmInstall({ id: "confirm-none" }));
  assert.deepEqual(await page.locator("#mc-permissions li").allTextContents(), ["无额外权限"]);
  await page.evaluate(() => confirmInstall({ id: "recorder" }));
  assert.equal(await page.locator("#mc-api").textContent(), "API 1");
  assert.equal(await page.locator("#mc-permissions").textContent(), "官方 API 1 插件使用完整权限");
  await page.locator("#mc-cancel").click();
  // Zip selection calls inspect first, then waits for a separate Install click.
  await page.locator("#plugin-install").click();
  await page.waitForFunction(() => !document.getElementById("mc-install").disabled);
  assert.equal(await page.locator("#mc-title").textContent(), "Permission demo  v2.1.0");
  const cardBounds = await page.locator("#market-confirm").boundingBox();
  const dialogBounds = await page.locator("#plugin-dialog").boundingBox();
  assert(cardBounds.y >= dialogBounds.y && cardBounds.y + cardBounds.height <= dialogBounds.y + dialogBounds.height,
    "Confirmation must be visible after selecting a zip in a scrolled plugin dialog");
  if (process.env.TMV_TEST_SCREENSHOT) await page.locator("#plugin-dialog").screenshot({ path: process.env.TMV_TEST_SCREENSHOT });
  assert.deepEqual(await page.locator("#mc-permissions li").allTextContents(), permissionTexts);
  assert.equal(await page.evaluate(() => mockInstalls.length), 1);
  await page.locator("#mc-cancel").click();
  assert.equal(await page.evaluate(() => mockInstalls.length), 1);
  await page.locator("#plugin-install").click();
  await page.waitForFunction(() => !document.getElementById("mc-install").disabled);
  await page.locator("#mc-install").click();
  await page.waitForFunction(() => mockInstalls.length === 2 && document.getElementById("market-confirm").hidden);
  assert.deepEqual(await page.evaluate(() => mockInstalls[1]), {
    command: "plugin_install", args: { path: "/test/demo.zip", sha256: "approved-digest" },
  });
  // Drive all seven languages through i18n's real selector and DOM observer.
  await page.evaluate(() => confirmInstall({ id: "confirm-v2" }));
  const languages = ["en", "de", "ja", "es", "fr", "ru", "zh"];
  for (const lang of languages) {
    await page.evaluate((lang) => {
      const select = document.querySelector('#plugin-buttons select[data-plugin="i18n"]');
      select.value = lang;
      select.dispatchEvent(new Event("change"));
    }, lang);
    await page.waitForFunction((lang) => document.documentElement.lang.startsWith(lang), lang);
    const column = translations.LANGS.findIndex(([code]) => code === lang);
    const expected = permissionTexts.map((text) => translations.WORDS.find((r) => r[0] === text)[column]);
    await page.waitForFunction((expected) => {
      const texts = [...document.querySelectorAll("#mc-permissions li")].map((e) => e.textContent);
      return JSON.stringify(texts) === JSON.stringify(expected);
    }, expected);
  }
  await page.locator("#mc-cancel").click();
  console.log("PASS: market/deep-link/zip permission confirmation, cancel/install boundaries, API 1 warning, empty permissions, seven languages");
  // The website card uses Chinese/English descriptions and English fallback.
  const website = await browser.newPage();
  website.on("pageerror", (e) => errors.push(String(e)));
  await website.addInitScript(() => {
    window.siteLang = "zh";
    window.TMV = { t: (key) => key, lang: () => window.siteLang, onLang: (fn) => { window.renderSite = fn; } };
  });
  await website.route("**/*", (route) => {
    const url = route.request().url();
    if (!url.startsWith(origin + "/")) { external.push(url); return route.abort(); }
    if (url.endsWith("/site/plugins/plugins.json")) return route.fulfill({ json: { plugins: [
      { id: "web-v2", name: "Web demo", version: "1.0.0", apiVersion: 2, permissions: ["state", "ui", "metadata", "export", "settings"] },
      { id: "web-empty", name: "Web empty", version: "1.0.0", apiVersion: 2, permissions: [] },
      { id: "web-legacy", name: "Web legacy", version: "1.0.0" },
    ] } });
    return route.continue();
  });
  await website.goto(origin + "/site/plugins/permission-test.html");
  await website.waitForFunction(() => document.querySelectorAll("#plugins article").length === 3);
  assert.deepEqual(await website.locator(".permissions").first().locator("li").allTextContents(), permissionTexts);
  assert.equal(await website.locator("#plugins article").nth(1).locator(".permissions").textContent(), "无额外权限");
  assert.equal(await website.locator("#plugins article").nth(2).locator(".permissions").count(), 0);
  let english;
  for (const lang of ["en", "ja", "de", "es", "fr", "ru"]) {
    await website.evaluate((lang) => { siteLang = lang; renderSite(); }, lang);
    const texts = await website.locator(".permissions").first().locator("li").allTextContents();
    if (lang === "en") {
      english = texts;
      assert.equal(texts[0], "Read image information and change slices and frames");
      assert.equal(texts[4], "Save its own settings");
    } else assert.deepEqual(texts, english);
    assert.equal(await website.locator("#plugins article").nth(1).locator(".permissions").textContent(), "No additional permissions");
  }
  await website.close();
  assert.deepEqual(external, [], "All browser tests must stay offline");
  console.log("PASS: website API 2 permissions in Chinese/English with English fallback");
  assert.deepEqual(errors, [], "Unexpected uncaught errors");
  assert(requests.includes("/tmvplugin/__sandbox/runtime.js"));
  assert(!requests.some((url) => url.startsWith("/tmvplugin/__sandbox/") && url !== "/tmvplugin/__sandbox/runtime.js"), "no protocol-served sandbox documents");
  console.log("PASS: opaque srcdoc, CSP intersection, cross-plugin read/script denial, escaped document attributes and blocked self-navigation");
  console.log(`PASS: API 2 UI/metadata/PNG export, 10 isolation probes, permissions/limits/settings/lifecycle; ${legacy.length + 1} API 1 plugins loaded`);
})().catch((e) => { console.error(e); process.exitCode = 1; }).finally(async () => {
  if (browser) await browser.close();
  server.close();
});
