// Builds and publishes a release from our own machines instead of CI, which
// takes about 45 minutes for a full release. Each machine builds its own
// platform; the Mac then assembles and uploads everything.
//
//   node scripts/local-release.mjs version
//   node scripts/local-release.mjs build [standard|pro|all] [--out DIR]
//   node scripts/local-release.mjs android [--out DIR]
//   node scripts/local-release.mjs publish --artifacts DIR [--dry-run]
//
// build:   macOS builds the universal .dmg/.app.tar.gz, Windows the NSIS
//          installer, Linux the .deb/.rpm/.AppImage, for each edition, into
//          DIR/<edition>/<platform>/ (default release-artifacts/<version>).
// android: the arm64 APK (standard edition), signed, into DIR/standard/android.
// publish: runs make-release.mjs for both editions and pack-plugins.mjs,
//          then uploads like CI does (version folder first, feeds last) and
//          purges the Cloudflare cache. Copy the other machines' DIR/<edition>/
//          folders into the same DIR first.
//
// All machines must build the same commit (the version is
// <major.minor>.<commit count>, as in CI). Secrets come from the environment
// or from scripts/local-release.env (gitignored, KEY=value lines):
//   TAURI_SIGNING_PRIVATE_KEY (key text or path), TAURI_SIGNING_PRIVATE_KEY_PASSWORD
//   ANDROID_KEYSTORE_FILE, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS
//   DEPLOY_HOST, DEPLOY_USER, DEPLOY_PATH, DEPLOY_KEY_FILE, CF_PURGE_TOKEN

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const UPDATE_BASE_URL = process.env.UPDATE_BASE_URL || "https://tifmrcview.com/tifmrcview";
const DIRECT_BASE_URL = process.env.DIRECT_BASE_URL || "https://dl.tifmrcview.com:9443/tifmrcview";
const CF_ZONE_ID = process.env.CF_ZONE_ID || "3dc3af1d7361be94d38608f6e7c642d9";

loadEnv(path.join(root, "scripts", "local-release.env"));

function loadEnv(file) {
  if (!fs.existsSync(file)) return;
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    const m = line.match(/^\s*([A-Z_][A-Z0-9_]*)\s*=\s*(.*?)\s*$/);
    if (m && process.env[m[1]] === undefined) process.env[m[1]] = m[2].replace(/^(["'])(.*)\1$/, "$2");
  }
}

// Windows needs a shell to run npx.cmd; quote the arguments for it.
function run(cmd, args, opts = {}) {
  console.log(`$ ${cmd} ${args.join(" ")}`);
  if (process.platform !== "win32") return execFileSync(cmd, args, { cwd: root, stdio: "inherit", ...opts });
  const q = (a) => (/[\s"]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a);
  return execFileSync([cmd, ...args].map(q).join(" "), { cwd: root, stdio: "inherit", shell: true, ...opts });
}

// The universal Quick Look extension links Swift compatibility libraries that
// the Command Line Tools ship for arm64 only, so use Xcode when it is there.
function useXcode() {
  if (process.platform !== "darwin" || process.env.DEVELOPER_DIR) return;
  const selected = execFileSync("xcode-select", ["-p"], { encoding: "utf8" }).trim();
  const xcode = "/Applications/Xcode.app/Contents/Developer";
  if (selected.includes("CommandLineTools") && fs.existsSync(xcode)) {
    console.log(`using ${xcode} (xcode-select points at ${selected})`);
    process.env.DEVELOPER_DIR = xcode;
  }
}

function git(...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

function version() {
  const conf = JSON.parse(fs.readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8"));
  return `${conf.version.split(".").slice(0, 2).join(".")}.${git("rev-list", "--count", "HEAD")}`;
}

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
}

const platform = { darwin: "macos-universal", win32: "windows-x64", linux: "linux-x64" }[process.platform];

// Same overlay as the CI "Build config" step.
function writeConfig(edition, v) {
  const pro = edition === "pro";
  const feed = pro ? "latest-pro" : "latest";
  const c = pro ? JSON.parse(fs.readFileSync(path.join(root, "src-tauri/tauri.pro.conf.json"), "utf8")) : {};
  c.version = v;
  c.plugins = { updater: { endpoints: [`${DIRECT_BASE_URL}/${feed}-direct.json`, `${UPDATE_BASE_URL}/${feed}.json`] } };
  if (process.env.TAURI_SIGNING_PRIVATE_KEY) c.bundle = { ...c.bundle, createUpdaterArtifacts: true };
  fs.writeFileSync(path.join(root, "src-tauri/tauri.ci.conf.json"), JSON.stringify(c, null, 2));
}

function walk(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
}

// The files CI uploads as bundle artifacts.
const bundleFile = /(\.dmg|\.app\.tar\.gz(\.sig)?|-setup\.exe(\.sig)?|\.deb(\.sig)?|\.rpm(\.sig)?|\.AppImage(\.sig)?)$/;

function clean() {
  const status = git("status", "--porcelain", "--untracked-files=no");
  if (status) console.warn(`warning: uncommitted changes are built too:\n${status}`);
}

function build(which, out) {
  if (!platform) throw new Error(`no desktop bundle for ${process.platform}`);
  if (!process.env.TAURI_SIGNING_PRIVATE_KEY) console.warn("warning: TAURI_SIGNING_PRIVATE_KEY not set; no updater packages, so installed apps won't see this release");
  clean();
  useXcode();
  const v = version();
  const editions = which === "all" ? ["standard", "pro"] : [which];
  const target = process.platform === "darwin" ? ["--target", "universal-apple-darwin"] : [];
  const bundleRoot = path.join(root, "src-tauri", "target");
  for (const edition of editions) {
    writeConfig(edition, v);
    // Old bundles of the other edition or version would be picked up too.
    for (const f of walk(bundleRoot).filter((f) => f.includes(`${path.sep}bundle${path.sep}`) && bundleFile.test(f))) fs.rmSync(f);
    const started = Date.now();
    run("npx", ["tauri", "build", ...target, "--config", "src-tauri/tauri.ci.conf.json"]);
    const dest = path.join(out, edition, platform);
    fs.mkdirSync(dest, { recursive: true });
    const found = walk(bundleRoot).filter((f) => f.includes(`${path.sep}bundle${path.sep}`) && bundleFile.test(f));
    if (!found.length) throw new Error("no bundles found");
    for (const f of found) fs.copyFileSync(f, path.join(dest, path.basename(f)));
    console.log(`${edition} ${platform} ${v}: ${found.length} files in ${((Date.now() - started) / 60000).toFixed(1)} min -> ${dest}`);
  }
}

function android(out) {
  const home = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (!home) throw new Error("ANDROID_HOME is not set");
  if (!process.env.NDK_HOME) {
    const ndk = path.join(home, "ndk");
    const versions = fs.existsSync(ndk) ? fs.readdirSync(ndk).sort() : [];
    if (!versions.length) throw new Error("no NDK under ANDROID_HOME/ndk");
    process.env.NDK_HOME = path.join(ndk, versions.at(-1));
  }
  clean();
  const v = version();
  fs.writeFileSync(path.join(root, "src-tauri/tauri.ci.conf.json"), JSON.stringify({ version: v }));
  run("npx", ["tauri", "android", "build", "--apk", "--target", "aarch64", "--config", "src-tauri/tauri.ci.conf.json"]);
  const apkDir = path.join(root, "src-tauri/gen/android/app/build/outputs/apk/universal/release");
  const apk = walk(apkDir).find((f) => f.endsWith(".apk"));
  if (!apk) throw new Error(`no APK in ${apkDir}`);
  const { ANDROID_KEYSTORE_FILE: ks, ANDROID_KEYSTORE_PASSWORD: pass, ANDROID_KEY_ALIAS: alias } = process.env;
  if (!ks || !pass || !alias) throw new Error("ANDROID_KEYSTORE_FILE, ANDROID_KEYSTORE_PASSWORD and ANDROID_KEY_ALIAS are needed to sign the APK");
  const tools = path.join(home, "build-tools", fs.readdirSync(path.join(home, "build-tools")).sort().at(-1));
  const dest = path.join(out, "standard", "android");
  fs.mkdirSync(dest, { recursive: true });
  const aligned = path.join(os.tmpdir(), `tmv-aligned-${process.pid}.apk`);
  run(path.join(tools, "zipalign"), ["-f", "4", apk, aligned]);
  run(path.join(tools, "apksigner"), ["sign", "--ks", ks, "--ks-pass", "env:ANDROID_KEYSTORE_PASSWORD", "--ks-key-alias", alias,
    "--out", path.join(dest, `TifMrcView_${v}_arm64.apk`), aligned]);
  fs.rmSync(aligned);
  console.log(`android ${v} -> ${dest}`);
}

function publish(artifacts, dryRun) {
  const v = version();
  const notes = git("log", "-1", "--pretty=%s").slice(0, 300);
  const release = path.join(artifacts, "release");
  fs.rmSync(release, { recursive: true, force: true });
  // Every platform must be there, or its users would lose the update.
  const missing = [];
  for (const edition of ["standard", "pro"]) {
    for (const p of ["macos-universal", "windows-x64", "linux-x64"]) {
      if (!walk(path.join(artifacts, edition, p)).length) missing.push(`${edition}/${p}`);
    }
  }
  if (!walk(path.join(artifacts, "standard", "android")).some((f) => f.endsWith("_arm64.apk"))) missing.push("standard/android");
  if (missing.length) throw new Error(`missing builds: ${missing.join(", ")}`);
  for (const f of walk(artifacts)) {
    const m = path.basename(f).match(/_(\d+\.\d+\.\d+)_/);
    if (m && m[1] !== v) throw new Error(`${f} is version ${m[1]}, this checkout is ${v}`);
  }
  run("node", ["scripts/make-release.mjs", path.join(artifacts, "standard"), release, v, UPDATE_BASE_URL, notes, DIRECT_BASE_URL]);
  run("node", ["scripts/make-release.mjs", path.join(artifacts, "pro"), release, v, UPDATE_BASE_URL, notes, DIRECT_BASE_URL], {
    env: { ...process.env, TMV_EDITION: "pro" },
  });
  run("node", ["scripts/pack-plugins.mjs", release]);
  if (dryRun) {
    console.log(`dry run: release ${v} assembled in ${release}, nothing uploaded`);
    return;
  }
  const { DEPLOY_HOST: host, DEPLOY_USER: user = "tifdeploy", DEPLOY_PATH: dir = "/var/www/tifmrcview", DEPLOY_KEY_FILE: key } = process.env;
  if (!host) throw new Error("DEPLOY_HOST is not set");
  const ssh = `ssh -o StrictHostKeyChecking=yes${key ? ` -i ${key}` : ""}`;
  const dest = `${user}@${host}:${dir.replace(/\/+$/, "")}`;
  const rsync = (from, to) => run("rsync", ["-av", "--chmod=D755,F644", "-e", ssh, from, to]);
  // Version folder first, feeds last, so clients never see a half-uploaded release.
  rsync(path.join(release, v), `${dest}/`);
  rsync(path.join(release, "plugins") + "/", `${dest}/plugins/`);
  for (const f of ["latest.json", "latest-direct.json", "downloads.json", "latest-pro.json", "latest-pro-direct.json", "downloads-pro.json"]) {
    if (fs.existsSync(path.join(release, f))) rsync(path.join(release, f), `${dest}/${f}`);
  }
  if (process.env.CF_PURGE_TOKEN) {
    run("curl", ["-fsS", "-X", "POST", `https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/purge_cache`,
      "-H", `Authorization: Bearer ${process.env.CF_PURGE_TOKEN}`, "-H", "Content-Type: application/json",
      "--data", '{"purge_everything":true}'], { stdio: ["ignore", "ignore", "inherit"] });
  } else {
    console.warn("warning: CF_PURGE_TOKEN not set; Cloudflare may serve old copies of rebuilt files");
  }
  console.log(`published ${v}`);
}

const [cmd, sub] = process.argv.slice(2);
const out = path.resolve(arg("--out", path.join(root, "release-artifacts", version())));
try {
  if (cmd === "version") console.log(version());
  else if (cmd === "build") build(sub && !sub.startsWith("--") ? sub : "all", out);
  else if (cmd === "android") android(out);
  else if (cmd === "publish") publish(path.resolve(arg("--artifacts", out)), process.argv.includes("--dry-run"));
  else {
    console.error("usage: local-release.mjs version | build [standard|pro|all] [--out DIR] | android [--out DIR] | publish --artifacts DIR [--dry-run]");
    process.exit(1);
  }
} catch (e) {
  console.error(`error: ${e.message}`);
  process.exit(1);
}
