// Collects CI bundle artifacts into a release folder for the download site.
//
//   node scripts/make-release.mjs <artifacts-dir> <out-dir> <version> <base-url> [notes] [direct-base-url]
//
// Produces <out-dir>/<version>/ with the installers and updater packages,
// plus <out-dir>/latest.json (Tauri updater feed) and
// <out-dir>/downloads.json (read by the download page). DIRECT_BASE_URL or
// the final argument also writes latest-direct.json for the direct host.
// TMV_EDITION=pro names the files TifMrcViewPro_* and writes latest-pro.json,
// latest-pro-direct.json and downloads-pro.json instead, in the same folder.

import fs from "node:fs";
import path from "node:path";
import { zip } from "./plugin-zip.mjs";

const [src, out, version, baseUrl, notes = "", directBaseUrl = process.env.DIRECT_BASE_URL] = process.argv.slice(2);
if (!src || !out || !version || !baseUrl) {
  console.error("usage: make-release.mjs <artifacts-dir> <out-dir> <version> <base-url> [notes] [direct-base-url]");
  process.exit(1);
}
const base = baseUrl.replace(/\/+$/, "");
const directBase = directBaseUrl?.replace(/\/+$/, "");
const pro = process.env.TMV_EDITION === "pro";
const product = pro ? "TifMrcViewPro" : "TifMrcView";
const feedName = (name) => (pro ? name.replace(/^(latest|downloads)/, "$1-pro") : name);

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
}

// Each kind: how to recognise the bundle, its published name, and where it
// goes in latest.json / downloads.json.
const kinds = [
  { id: "dmg", re: /\.dmg$/, name: `${product}_${version}_universal.dmg`, os: "mac", label: "下载 .dmg（通用）", note: "Apple 芯片与 Intel 通用，macOS 10.15+" },
  { id: "app", re: /\.app\.tar\.gz$/, name: `${product}_${version}_universal.app.tar.gz`, updater: ["darwin-aarch64", "darwin-x86_64", "darwin-aarch64-app", "darwin-x86_64-app"] },
  { id: "nsis", re: /-setup\.exe$/, name: `${product}_${version}_x64-setup.exe`, os: "win", label: "下载安装程序", note: "Windows 10 / 11，64 位", updater: ["windows-x86_64", "windows-x86_64-nsis"] },
  { id: "deb", re: /\.deb$/, name: `${product}_${version}_amd64.deb`, os: "linux", label: ".deb", note: "Debian / Ubuntu", updater: ["linux-x86_64-deb"] },
  { id: "rpm", re: /\.rpm$/, name: `${product}-${version}-1.x86_64.rpm`, os: "linux", label: ".rpm", note: "Fedora / openSUSE", updater: ["linux-x86_64-rpm"] },
  { id: "apk", re: /_arm64\.apk$/, name: `${product}_${version}_arm64.apk`, os: "android", label: "下载 APK", note: "Android 7 及以上，arm64 手机和平板" },
  { id: "appimage", re: /\.AppImage$/, name: `${product}_${version}_amd64.AppImage`, os: "linux", label: ".AppImage", note: "其它发行版，免安装", updater: ["linux-x86_64", "linux-x86_64-appimage"] },
];

const files = walk(src);
const verDir = path.join(out, version);
fs.mkdirSync(verDir, { recursive: true });

const platforms = {};
const downloads = { mac: [], win: [], linux: [], android: [] };
for (const k of kinds) {
  const f = files.find((p) => k.re.test(p));
  if (!f) {
    console.warn(`missing ${k.id}`);
    continue;
  }
  fs.copyFileSync(f, path.join(verDir, k.name));
  const rel = `${version}/${k.name}`;
  const url = `${base}/${rel}`;
  console.log(`${k.id}: ${path.basename(f)} -> ${rel}`);
  if (k.os) downloads[k.os].push({ label: k.label, file: rel, note: k.note, size: fs.statSync(f).size });
  // Some browsers and mail filters block a bare unsigned .exe; offer the same
  // installer inside a .zip as a second Windows download.
  if (k.id === "nsis") {
    const zipName = k.name.replace(/\.exe$/, ".zip");
    fs.writeFileSync(path.join(verDir, zipName), zip([{ name: k.name, data: fs.readFileSync(f) }]));
    const size = fs.statSync(path.join(verDir, zipName)).size;
    downloads.win.push({ label: "下载 .zip（内含安装程序）", file: `${version}/${zipName}`, note: "浏览器拦截 .exe 时用这个", size });
  }
  const sig = f + ".sig";
  if (k.updater && fs.existsSync(sig)) {
    fs.copyFileSync(sig, path.join(verDir, k.name + ".sig"));
    const signature = fs.readFileSync(sig, "utf8").trim();
    for (const p of k.updater) platforms[p] = { signature, url };
  }
}

const date = new Date().toISOString();
fs.writeFileSync(path.join(out, feedName("downloads.json")), JSON.stringify({ version, date: date.slice(0, 10), files: downloads }, null, 2) + "\n");
if (Object.keys(platforms).length) {
  const feed = { version, notes, pub_date: date.replace(/\.\d+Z$/, "Z"), platforms };
  fs.writeFileSync(path.join(out, feedName("latest.json")), JSON.stringify(feed, null, 2) + "\n");
  if (directBase) {
    const directPlatforms = Object.fromEntries(Object.entries(platforms).map(([key, entry]) => [
      key, { ...entry, url: directBase + entry.url.slice(base.length) },
    ]));
    fs.writeFileSync(path.join(out, feedName("latest-direct.json")), JSON.stringify({ ...feed, platforms: directPlatforms }, null, 2) + "\n");
  }
} else {
  console.warn("no .sig files: updater feed not written (is TAURI_SIGNING_PRIVATE_KEY set?)");
}
