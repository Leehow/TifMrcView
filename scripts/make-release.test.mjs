import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const script = fileURLToPath(new URL("./make-release.mjs", import.meta.url));
const cloudflare = "https://tifmrcview.com/tifmrcview";
const direct = "https://dl.tifmrcview.com:9443/tifmrcview";
const artifacts = ["test.dmg", "test.app.tar.gz", "test-setup.exe", "test.deb", "test.rpm", "test.AppImage"];

function release(t, { arg, env, signed = true, edition } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tmv-release-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const src = path.join(dir, "artifacts"), out = path.join(dir, "release");
  fs.mkdirSync(src);
  for (const file of artifacts) {
    fs.writeFileSync(path.join(src, file), `payload of ${file}`);
    if (signed) fs.writeFileSync(path.join(src, file + ".sig"), `signature of ${file}\n`);
  }
  const childEnv = { ...process.env };
  delete childEnv.DIRECT_BASE_URL;
  if (env !== undefined) childEnv.DIRECT_BASE_URL = env;
  delete childEnv.TMV_EDITION;
  if (edition) childEnv.TMV_EDITION = edition;
  execFileSync(process.execPath, [script, src, out, "1.2.3", cloudflare + "/", "Release notes\non two lines", ...(arg === undefined ? [] : [arg])], { env: childEnv, stdio: "pipe" });
  return { out, read: (name) => JSON.parse(fs.readFileSync(path.join(out, name), "utf8")) };
}

for (const source of ["argument", "environment"]) {
  test(`direct feed from ${source} keeps metadata, signatures and artifact paths`, (t) => {
    const { out, read } = release(t, source === "argument" ? { arg: direct + "///", env: "https://unused.test" } : { env: direct + "/" });
    const normal = read("latest.json"), fast = read("latest-direct.json");
    assert.equal(Object.keys(normal.platforms).length, 10);
    const platforms = {};
    for (const [key, entry] of Object.entries(normal.platforms)) {
      assert.ok(entry.url.startsWith(cloudflare + "/1.2.3/"));
      const rel = entry.url.slice(cloudflare.length + 1);
      assert.ok(fs.existsSync(path.join(out, rel)));
      assert.equal(fs.readFileSync(path.join(out, rel + ".sig"), "utf8").trim(), entry.signature);
      platforms[key] = { ...entry, url: direct + "/" + rel };
    }
    assert.deepEqual(fast, { ...normal, platforms });
    for (const file of Object.values(read("downloads.json").files).flat()) {
      assert.ok(file.file.startsWith("1.2.3/"), "download page paths stay relative");
      assert.equal(fs.statSync(path.join(out, file.file)).size, file.size);
    }
  });
}

test("without DIRECT_BASE_URL only the original feeds are written", (t) => {
  const { out, read } = release(t);
  assert.ok(read("latest.json").platforms["darwin-aarch64"]);
  assert.ok(read("downloads.json").files.mac.length);
  assert.equal(fs.existsSync(path.join(out, "latest-direct.json")), false);
});

test("unsigned releases do not publish either updater feed", (t) => {
  const { out, read } = release(t, { env: direct, signed: false });
  assert.ok(read("downloads.json").files.win.length);
  assert.equal(fs.existsSync(path.join(out, "latest.json")), false);
  assert.equal(fs.existsSync(path.join(out, "latest-direct.json")), false);
});

test("Pro writes its own feeds and file names next to the standard ones", (t) => {
  const { out, read } = release(t, { env: direct, edition: "pro" });
  assert.equal(fs.existsSync(path.join(out, "latest.json")), false);
  assert.equal(fs.existsSync(path.join(out, "downloads.json")), false);
  const feed = read("latest-pro.json");
  assert.equal(feed.platforms["windows-x86_64"].url, cloudflare + "/1.2.3/TifMrcViewPro_1.2.3_x64-setup.exe");
  assert.equal(read("latest-pro-direct.json").platforms["windows-x86_64"].url, direct + "/1.2.3/TifMrcViewPro_1.2.3_x64-setup.exe");
  assert.deepEqual(read("downloads-pro.json").files.linux.map((f) => f.file), [
    "1.2.3/TifMrcViewPro_1.2.3_amd64.deb", "1.2.3/TifMrcViewPro-1.2.3-1.x86_64.rpm", "1.2.3/TifMrcViewPro_1.2.3_amd64.AppImage",
  ]);
});

test("the Windows installer is also offered inside a .zip", (t) => {
  const { out, read } = release(t);
  const win = read("downloads.json").files.win;
  assert.deepEqual(win.map((f) => f.file), ["1.2.3/TifMrcView_1.2.3_x64-setup.exe", "1.2.3/TifMrcView_1.2.3_x64-setup.zip"]);
  const listing = execFileSync("unzip", ["-l", path.join(out, win[1].file)], { encoding: "utf8" });
  assert.match(listing, /TifMrcView_1\.2\.3_x64-setup\.exe/);
  const exe = execFileSync("unzip", ["-p", path.join(out, win[1].file)]);
  assert.equal(exe.toString(), "payload of test-setup.exe");
  assert.equal(fs.statSync(path.join(out, win[1].file)).size, win[1].size);
});
