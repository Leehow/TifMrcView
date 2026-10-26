// One command: node build.mjs. Requires Rust's wasm32 target and Node 22+.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const tools = process.env.TMV_PLUGIN_TOOLS
  ? path.resolve(process.env.TMV_PLUGIN_TOOLS)
  : [path.join(dir, "tools"), path.resolve(dir, "../../scripts")].find((p) => fs.existsSync(path.join(p, "tmv-plugin.mjs")));
if (!tools) throw new Error("Checker not found: copy the shared scripts to tools/ (see README), or set TMV_PLUGIN_TOOLS");
const { checkPlugin } = await import(pathToFileURL(path.join(tools, "tmv-plugin.mjs")));
const { zip } = await import(pathToFileURL(path.join(tools, "plugin-zip.mjs")));
function check(input) {
  const result = checkPlugin(input);
  for (const warning of result.warnings) console.warn(`warning: ${warning}`);
  if (result.errors.length) throw new Error(result.errors.join("; "));
  return result.manifest;
}

const manifest = check(dir);
if (typeof manifest.wasm !== "string") throw new Error("plugin.json must name a wasm module");
const toml = fs.readFileSync(path.join(dir, "Cargo.toml"), "utf8");
const name = (toml.match(/^\s*name\s*=\s*"([^"]+)"/m) || [])[1];
if (!name) throw new Error("Cargo.toml has no package name");
const target = path.join(dir, "target");
execFileSync(process.env.CARGO || "cargo", [
  "build", "--target", "wasm32-unknown-unknown", "--release",
  "--manifest-path", path.join(dir, "Cargo.toml"), "--target-dir", target,
], { cwd: dir, stdio: "inherit" });

const staging = fs.mkdtempSync(path.join(os.tmpdir(), "tmv-format-plugin-"));
try {
  const pkg = path.join(staging, manifest.id);
  fs.mkdirSync(pkg);
  const entries = [
    { name: "plugin.json", data: fs.readFileSync(path.join(dir, "plugin.json")) },
    { name: manifest.main, data: fs.readFileSync(path.join(dir, manifest.main)) },
    { name: manifest.wasm, data: fs.readFileSync(path.join(target, "wasm32-unknown-unknown/release", `${name.replace(/-/g, "_")}.wasm`)) },
  ];
  for (const entry of entries) {
    const dest = path.join(pkg, entry.name);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, entry.data);
  }
  check(pkg);
  const dist = path.join(dir, "dist");
  fs.mkdirSync(dist, { recursive: true });
  const out = path.join(dist, `${manifest.id}-${manifest.version}.zip`);
  // Same writer and root layout as scripts/pack-plugins.mjs; no Rust sources.
  fs.writeFileSync(out, zip([entries[0], ...entries.slice(1).sort((a, b) => a.name.localeCompare(b.name))]));
  check(out);
  console.log(out);
} finally {
  fs.rmSync(staging, { recursive: true, force: true });
}
