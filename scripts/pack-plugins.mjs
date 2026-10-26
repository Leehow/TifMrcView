// Packs plugins/<id>/ into installable zips and writes the market index.
//
//   node scripts/pack-plugins.mjs <out-dir>
//
// Produces <out-dir>/plugins/<id>-<version>.zip, <id>.zip (latest alias) and
// <out-dir>/plugins/plugins.json (read by the plugin market page).
// plugin.json sits at the zip root. No dependencies: a minimal zip writer.
// Set PLUGINS_DIR to pack from somewhere other than ./plugins (for tests).
//
// A plugin folder with a Cargo.toml is a WebAssembly plugin crate (see
// docs/format-plugins.md): it is built with
//   cargo build --release --target wasm32-unknown-unknown
// and the .wasm goes into the zip under the name plugin.json gives in
// "wasm"; the Rust sources and Cargo files stay out. Needs Rust with the
// wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown).
// PLUGINS_TARGET_DIR sets the cargo target folder (default target/wasm-plugins).

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pluginsDir = path.resolve(root, process.env.PLUGINS_DIR || "plugins");
const targetDir = path.resolve(root, process.env.PLUGINS_TARGET_DIR || "target/wasm-plugins");
const outArg = process.argv[2];
if (!outArg) {
  console.error("usage: pack-plugins.mjs <out-dir>");
  process.exit(1);
}
const outDir = path.join(path.resolve(outArg), "plugins");

const CRC_TABLE = Uint32Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// Fixed timestamp (2020-01-01 00:00) keeps the archives reproducible.
const DOS_TIME = 0;
const DOS_DATE = ((2020 - 1980) << 9) | (1 << 5) | 1;

/** entries: [{name, data}] -> Buffer */
function zip(entries) {
  const parts = [];
  const central = [];
  let offset = 0;
  for (const { name, data } of entries) {
    const nameBuf = Buffer.from(name, "utf8");
    const crc = crc32(data);
    let method = 8;
    let body = zlib.deflateRawSync(data, { level: 9 });
    if (body.length >= data.length) {
      method = 0;
      body = data;
    }
    const flags = 0x0800; // UTF-8 names
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(flags, 6);
    local.writeUInt16LE(method, 8);
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    local.writeUInt16LE(0, 28);
    parts.push(local, nameBuf, body);

    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0);
    c.writeUInt16LE((3 << 8) | 20, 4); // made by Unix
    c.writeUInt16LE(20, 6);
    c.writeUInt16LE(flags, 8);
    c.writeUInt16LE(method, 10);
    c.writeUInt16LE(DOS_TIME, 12);
    c.writeUInt16LE(DOS_DATE, 14);
    c.writeUInt32LE(crc, 16);
    c.writeUInt32LE(body.length, 20);
    c.writeUInt32LE(data.length, 24);
    c.writeUInt16LE(nameBuf.length, 28);
    c.writeUInt32LE((0o100644 << 16) >>> 0, 38); // external attrs: regular file 0644
    c.writeUInt32LE(offset, 42);
    central.push(c, nameBuf);
    offset += local.length + nameBuf.length + body.length;
  }
  const cdSize = central.reduce((n, b) => n + b.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cdSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...parts, ...central, end]);
}

/** Files under dir, relative posix paths, sorted, dotfiles skipped. */
function listFiles(dir, rel = "") {
  const out = [];
  const names = fs.readdirSync(path.join(dir, rel)).filter((n) => !n.startsWith(".")).sort();
  for (const n of names) {
    const r = rel ? `${rel}/${n}` : n;
    const st = fs.statSync(path.join(dir, r));
    if (st.isDirectory()) out.push(...listFiles(dir, r));
    else if (st.isFile()) out.push(r);
  }
  return out;
}

// Crate files that are not part of the installed plugin.
const CRATE_FILES = new Set(["Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain", "rust-toolchain.toml"]);
const CRATE_DIRS = ["src/", "target/", "tests/", "benches/", "examples/"];
const isCrateFile = (f) => CRATE_FILES.has(f) || CRATE_DIRS.some((d) => f.startsWith(d));

const EXT = /^[a-z0-9]{1,16}$/;
const extList = (v) => Array.isArray(v) && v.length > 0 && v.every((e) => typeof e === "string" && EXT.test(e));

/** Builds the plugin crate in `dir` for wasm32; returns the .wasm path. */
function buildWasm(dir) {
  const toml = fs.readFileSync(path.join(dir, "Cargo.toml"), "utf8");
  const name = (toml.match(/^\s*name\s*=\s*"([^"]+)"/m) || [])[1];
  if (!name) throw new Error("Cargo.toml has no package name");
  const args = ["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", path.join(dir, "Cargo.toml"), "--target-dir", targetDir];
  console.log(`cargo ${args.join(" ")}`);
  execFileSync(process.env.CARGO || "cargo", args, { stdio: "inherit" });
  return path.join(targetDir, "wasm32-unknown-unknown", "release", `${name.replace(/-/g, "_")}.wasm`);
}

const REQUIRED = ["id", "name", "version", "description", "main"];
function validate(m, dirName, files) {
  const errs = [];
  for (const k of REQUIRED) if (typeof m[k] !== "string" || !m[k]) errs.push(`missing "${k}"`);
  if (m.id !== dirName) errs.push(`id "${m.id}" does not match folder "${dirName}"`);
  if (typeof m.id === "string" && !/^[a-z0-9][a-z0-9._-]*$/.test(m.id)) errs.push("id must be lowercase letters, digits, . _ -");
  if (typeof m.version === "string" && !/^\d+\.\d+\.\d+([-+][\w.]+)?$/.test(m.version)) errs.push("version must look like 1.2.3");
  if (typeof m.main === "string" && !files.includes(m.main)) errs.push(`main "${m.main}" not found in package`);
  if (m.apiVersion != null && !Number.isInteger(m.apiVersion)) errs.push("apiVersion must be an integer");
  if (m.native != null && typeof m.native !== "boolean") errs.push("native must be a boolean");
  if (m.wasm != null && (typeof m.wasm !== "string" || !/^[\w.-]+(\/[\w.-]+)*\.wasm$/.test(m.wasm))) errs.push("wasm must be a .wasm path");
  if (m.wasm != null && !files.includes(m.wasm)) errs.push(`wasm "${m.wasm}" not found in package`);
  if (m.formats != null && !(Array.isArray(m.formats) && m.formats.every((f) => f && extList(f.ext)))) {
    errs.push('formats must be [{"ext": ["abc"], "name": "…"}] with lower-case extensions');
  }
  if (m.metadata != null && !(m.metadata && extList(m.metadata.ext))) errs.push('metadata must be {"ext": ["tif"]}');
  if ((m.formats || m.metadata) && m.wasm == null) errs.push("formats / metadata need a wasm module");
  return errs;
}

// The only plugin compiled into the app: listed on the market, no download.
const builtin = [
  {
    id: "i18n",
    name: "多语言",
    version: "1.0.0",
    description: "把界面翻译成中文、English、Deutsch、日本語、Español、Français、Русский。",
    author: "TifMrcView",
    builtin: true,
    i18n: {
      en: { name: "Languages", description: "Translates the interface into 中文, English, Deutsch, 日本語, Español, Français and Русский." },
      de: { name: "Sprachen", description: "Übersetzt die Oberfläche ins Chinesische, Englische, Deutsche, Japanische, Spanische, Französische und Russische." },
      ja: { name: "多言語", description: "画面を中国語・英語・ドイツ語・日本語・スペイン語・フランス語・ロシア語に翻訳します。" },
      es: { name: "Idiomas", description: "Traduce la interfaz a chino, inglés, alemán, japonés, español, francés y ruso." },
      fr: { name: "Langues", description: "Traduit l'interface en chinois, anglais, allemand, japonais, espagnol, français et russe." },
      ru: { name: "Языки", description: "Переводит интерфейс на китайский, английский, немецкий, японский, испанский, французский и русский." },
    },
    details: {
      zh: ["内置在应用里，无需下载。可在「插件」窗口里随时开关。"],
      en: ["Built into the app, nothing to download. Switch it on or off in the Plugins window."],
      de: ["In die App integriert, kein Download erforderlich. Lässt sich im Fenster „Plugins“ jederzeit ein- oder ausschalten."],
      ja: ["アプリに内蔵されているため、ダウンロードは不要です。「プラグイン」ウィンドウでいつでも有効・無効を切り替えられます。"],
      es: ["Integrado en la aplicación, no requiere descarga. Se puede activar o desactivar en la ventana de complementos."],
      fr: ["Intégré à l'application, sans téléchargement. Vous pouvez l'activer ou le désactiver dans la fenêtre des extensions."],
      ru: ["Встроен в приложение, скачивать ничего не нужно. Его можно включить или выключить в окне плагинов."],
    },
  },
];

if (!fs.existsSync(pluginsDir)) {
  console.error(`no plugins directory: ${pluginsDir}`);
  process.exit(1);
}
fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });

let failed = false;
const list = [];
const dirs = fs
  .readdirSync(pluginsDir, { withFileTypes: true })
  .filter((e) => e.isDirectory() && !e.name.startsWith("."))
  .map((e) => e.name)
  .sort();
for (const d of dirs) {
  const dir = path.join(pluginsDir, d);
  const mfile = path.join(dir, "plugin.json");
  if (!fs.existsSync(mfile)) continue;
  let m;
  try {
    m = JSON.parse(fs.readFileSync(mfile, "utf8"));
  } catch (e) {
    console.error(`${d}: invalid plugin.json: ${e.message}`);
    failed = true;
    continue;
  }
  // name in the zip -> file on disk
  const sources = new Map();
  const crate = fs.existsSync(path.join(dir, "Cargo.toml"));
  for (const f of listFiles(dir)) if (!crate || !isCrateFile(f)) sources.set(f, path.join(dir, f));
  if (crate) {
    if (typeof m.wasm !== "string") {
      console.error(`${d}: has a Cargo.toml but plugin.json names no "wasm" module`);
      failed = true;
      continue;
    }
    try {
      const wasm = buildWasm(dir);
      if (!fs.existsSync(wasm)) throw new Error(`${wasm} was not built`);
      sources.set(m.wasm, wasm);
    } catch (e) {
      console.error(`${d}: building the WebAssembly module failed: ${e.message}`);
      failed = true;
      continue;
    }
  }
  const files = [...sources.keys()].sort();
  const errs = validate(m, d, files);
  if (errs.length) {
    console.error(`${d}: ${errs.join("; ")}`);
    failed = true;
    continue;
  }
  // plugin.json first, then the rest in sorted order.
  const ordered = ["plugin.json", ...files.filter((f) => f !== "plugin.json")];
  const buf = zip(ordered.map((name) => ({ name, data: fs.readFileSync(sources.get(name)) })));
  const file = `${m.id}-${m.version}.zip`;
  fs.writeFileSync(path.join(outDir, file), buf);
  fs.writeFileSync(path.join(outDir, `${m.id}.zip`), buf);
  list.push({ ...m, file, size: buf.length, sha256: crypto.createHash("sha256").update(buf).digest("hex") });
  console.log(`${m.id} ${m.version}: ${file} (${buf.length} bytes, ${ordered.length} files)`);
}
if (failed) process.exit(1);

const index = { generated: new Date().toISOString(), plugins: [...builtin, ...list] };
fs.writeFileSync(path.join(outDir, "plugins.json"), JSON.stringify(index, null, 2) + "\n");
console.log(`plugins.json: ${list.length} packages + ${builtin.length} built-in`);
