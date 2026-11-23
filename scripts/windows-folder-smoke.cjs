const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

(async () => {
  const directory = process.argv[2];
  const appId = Number(process.argv[3]);
  assert.ok(appId > 0);
  const browser = await chromium.connectOverCDP("http://127.0.0.1:19231", { timeout: 35000 });
  try {
    const page = browser.contexts().flatMap(c => c.pages()).find(page => /tauri\.localhost/.test(page.url()));
    assert.ok(page, "Packaged WebView2 page is running");
    await page.waitForFunction(() => window.TifMrcViewPro?.cryoet?.reconFolder);
    assert.equal(await page.evaluate(() => T.app.getVersion()), "0.7.331");
    await page.evaluate(path => { S.folder.dir = path; }, directory);
    await page.locator("#btn-open").click();
    await page.locator("#dropmenu").getByText(/打开本地文件夹|Open local folder/).click();
    execFileSync("powershell.exe", [
      "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
      "scripts/select-test-folder.ps1", String(appId),
    ], { stdio: "inherit", timeout: 22000 });
    await page.locator("#strip").getByText("Records", { exact: true }).waitFor({ timeout: 12000 });
    assert.equal(await page.locator("#folder-name").getAttribute("title"), directory);
    await page.locator("#btn-cryoet").click();
    await page.locator(".cs-tile").filter({ hasText: "TS_native_smoke" }).waitFor();
    await page.screenshot({ path: "windows-qa-series.png" });
    console.log("PASS: Windows packaged Pro 0.7.331 folder picker, local browser and real Cryo-ET series discovery");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
