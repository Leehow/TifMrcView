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
import { zip } from "./plugin-zip.mjs";
import { validateManifest } from "./plugin-manifest.mjs";
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
  const errs = validateManifest(m, d, files);
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
