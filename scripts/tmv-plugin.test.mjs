import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import vm from "node:vm";
import { checkPlugin, checkWasm } from "./tmv-plugin.mjs";
import { validateManifest, SANDBOX_PERMISSION_NAMES } from "./plugin-manifest.mjs";
import { zip } from "./plugin-zip.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = {
  id: "r16", name: "R16", version: "1.0.0", description: "test",
  main: "index.js", apiVersion: 1, minAppVersion: "0.5.0",
  wasm: "r16.wasm", formats: [{ ext: ["r16"], name: "R16" }],
};
const signatures = {
  tmv_abi: [[], [0x7f]], tmv_alloc: [[0x7f], [0x7f]], tmv_free: [[0x7f, 0x7f], []],
  tmv_open: [[0x7f, 0x7f], [0x7f]], tmv_info: [[0x7f], [0x7e]],
  tmv_page: [[0x7f, 0x7f], [0x7e]], tmv_close: [[0x7f], []],
};
function u32(n) {
  const out = [];
  do { const b = n & 127; n >>>= 7; out.push(b | (n ? 128 : 0)); } while (n);
  return out;
}
const vector = (items) => [...u32(items.length), ...items.flat()];
const name = (s) => [...u32(Buffer.byteLength(s)), ...Buffer.from(s)];
const section = (id, data) => [id, ...u32(data.length), ...data];

// Tiny real binaries, without a Rust toolchain or downloaded WAT compiler.
function wasm({ exports = signatures, imports = [], memory = true } = {}) {
  const funcs = Object.entries(exports);
  const types = [...imports.map((i) => i.type), ...funcs.map(([, type]) => type)];
  const parts = [section(1, vector(types.map(([p, r]) => [0x60, ...vector(p), ...vector(r)])))];
  if (imports.length) parts.push(section(2, vector(imports.map((i, n) => [...name(i.module), ...name(i.name), 0, ...u32(n)]))));
  parts.push(section(3, vector(funcs.map((_, n) => u32(imports.length + n)))));
  parts.push(section(5, [1, 0, 1]));
  parts.push(section(7, vector([
    ...(memory ? [[...name("memory"), 2, 0]] : []),
    ...funcs.map(([n], i) => [...name(n), 0, ...u32(imports.length + i)]),
  ])));
  parts.push(section(10, vector(funcs.map(([, [, results]]) => {
    const body = [0, ...results.flatMap((t) => [t === 0x7e ? 0x42 : 0x41, 0]), 0x0b];
    return [...u32(body.length), ...body];
  }))));
  return Buffer.from([0, 97, 115, 109, 1, 0, 0, 0, ...parts.flat()]);
}

function temp(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tmv-plugin-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("format/metadata ABI, optional series, imports and signatures", () => {
  const imports = [
    { module: "tmv", name: "file_size", type: [[], [0x7e]] },
    { module: "tmv", name: "read", type: [[0x7e, 0x7f, 0x7f], [0x7f]] },
    { module: "tmv", name: "decompress", type: [[0x7f, 0x7f, 0x7f, 0x7f, 0x7f], [0x7f]] },
    ...["log", "error"].map((name) => ({ module: "tmv", name, type: [[0x7f, 0x7f], []] })),
  ];
  assert.deepEqual(checkWasm(wasm({ imports }), manifest), { errors: [], warnings: [] });
  const both = { ...signatures, tmv_metadata: [[0x7f, 0x7f], [0x7e]], tmv_open_series: [[0x7f, 0x7f, 0x7f], [0x7f]] };
  assert.deepEqual(checkWasm(wasm({ exports: both }), { ...manifest, metadata: { ext: ["tif"] } }).errors, []);
  const metadata = Object.fromEntries(Object.entries(both).filter(([n]) => ["tmv_abi", "tmv_alloc", "tmv_free", "tmv_metadata"].includes(n)));
  assert.deepEqual(checkWasm(wasm({ exports: metadata }), { metadata: { ext: ["tif"] } }).errors, []);
  for (const n of Object.keys(signatures)) {
    const missing = { ...signatures };
    delete missing[n];
    assert.ok(checkWasm(wasm({ exports: missing }), manifest).errors.includes(`missing export ${n}`));
    assert.ok(checkWasm(wasm({ exports: { ...signatures, [n]: [[], []] } }), manifest).errors.some((e) => e.includes(`export ${n} has the wrong`)));
  }
  assert.ok(checkWasm(wasm({ memory: false }), manifest).errors.includes("missing export memory"));
  assert.ok(checkWasm(wasm(), { ...manifest, metadata: { ext: ["tif"] } }).errors.includes("missing export tmv_metadata"));
  const badImport = [{ module: "tmv", name: "read", type: [[], []] }];
  assert.match(checkWasm(wasm({ imports: badImport }), manifest).errors[0], /wrong ABI signature/);
  const unknown = [{ module: "wasi_snapshot_preview1", name: "fd_write", type: [[], []] }];
  assert.match(checkWasm(wasm({ imports: unknown }), manifest).warnings[0], /unknown import.*app will refuse/);
  assert.match(checkWasm(Buffer.from("not wasm"), manifest).errors[0], /not a WebAssembly/);
  assert.match(checkWasm(wasm().subarray(0, 12), manifest).errors[0], /invalid WebAssembly/);
});

test("permission vocabulary matches Rust and the runtime; API 2 is strict", () => {
  const rust = fs.readFileSync(path.join(root, "src-tauri/src/plugins/package.rs"), "utf8");
  const names = JSON.parse("[" + rust.match(/pub const SANDBOX_PERMISSIONS:.*?= &\[([\s\S]*?)\];/)[1] + "]");
  const runtime = fs.readFileSync(path.join(root, "ui/plugin-sandbox.js"), "utf8");
  const methods = vm.runInNewContext(runtime + "; SANDBOX_PERMISSIONS");
  assert.deepEqual([...new Set(Object.values(methods))].sort(), [...names].sort());
  assert.deepEqual([...SANDBOX_PERMISSION_NAMES].sort(), [...names].sort());
  const m = { ...manifest, apiVersion: 2, permissions: [...names] };
  const check = (value) => validateManifest(value, "r16", ["index.js", "r16.wasm"]);
  assert.deepEqual(check(m), []);
  assert.deepEqual(check({ ...m, permissions: [] }), []);
  const { permissions, ...missing } = m;
  assert.ok(check(missing).some((e) => e.includes("permissions")));
  for (const permissions of [null, "state", {}, ["unknown"], [1]]) {
    assert.ok(check({ ...m, permissions }).some((e) => e.includes("permissions")));
  }
  for (const native of [null, false, true]) assert.ok(check({ ...m, native }).some((e) => e.includes("native")));
  const official = JSON.parse("[" + fs.readFileSync(path.join(root, "src-tauri/src/plugins/market.rs"), "utf8")
    .match(/pub const OFFICIAL_API1_IDS:.*?= &\[([\s\S]*?)\];/)[1] + "]");
  const shipped = fs.readdirSync(path.join(root, "plugins")).filter((id) => fs.existsSync(path.join(root, "plugins", id, "plugin.json")));
  assert.deepEqual(official.sort(), shipped.sort());
});

test("install permissions and errors have all seven i18n columns", () => {
  const { WORDS } = vm.runInNewContext(fs.readFileSync(path.join(root, "ui/plugins/i18n.js"), "utf8") + "; ({WORDS})", { TifMrcView: { register() {} } });
  // The install-time rows are grouped at the start of WORDS.
  for (const row of WORDS.slice(0, 15)) {
    assert.equal(row.length, 7, row[0]);
    assert.ok(row.every((s) => typeof s === "string" && s.trim()), row[0]);
  }
  const rust = ["src/desktop.rs", "src/plugins/desktop.rs", "src/plugins/market.rs", "src/plugins/package.rs"]
    .map((p) => fs.readFileSync(path.join(root, "src-tauri", p), "utf8")).join("\n");
  for (const row of WORDS.slice(9, 15)) assert.ok(rust.includes(row[0]), `UI error must match backend: ${row[0]}`);
});

test("source checks share manifest rules and distinguish a missing build", (t) => {
  const dir = path.join(temp(t), "r16");
  fs.mkdirSync(dir);
  const write = (m) => fs.writeFileSync(path.join(dir, "plugin.json"), JSON.stringify(m));
  write(manifest);
  fs.writeFileSync(path.join(dir, "index.js"), "");
  assert.ok(checkPlugin(dir).errors.some((e) => e.includes("wasm")));
  fs.writeFileSync(path.join(dir, "Cargo.toml"), "");
  assert.deepEqual(checkPlugin(dir).errors, []);
  assert.match(checkPlugin(dir).notes[0], /manifest only/);
  for (const [field, value] of [["id", "r_16"], ["version", "bad"], ["apiVersion", 3], ["permissions", ["fs"]], ["apiVersion", null], ["minAppVersion", "0.6.0"], ["minAppVersion", "bad"], ["main", "../index.js"]]) {
    write({ ...manifest, [field]: value });
    assert.ok(checkPlugin(dir).errors.some((e) => e.includes(field)), `${field}: ${value}`);
  }
  write({ ...manifest, minAppVersion: "0.10.0" });
  assert.deepEqual(checkPlugin(dir, { appVersion: "0.10.1" }).errors, []);
  write({ ...manifest, formats: [{ ext: ["tif"] }] });
  assert.ok(checkPlugin(dir).errors.some((e) => e.includes("core extension")));
  write({ ...manifest, description: "" });
  const shared = validateManifest({ ...manifest, description: "" }, "r16", ["index.js", "r16.wasm"]);
  assert.deepEqual(checkPlugin(dir).errors, shared);
  write(manifest);
  fs.writeFileSync(path.join(dir, "r16.wasm"), wasm());
  assert.deepEqual(checkPlugin(dir).errors, []);
  assert.match(checkPlugin(dir).notes[0], /static ABI checked/);
});

test("root and wrapped zips, missing wasm, unsafe paths and CLI exit status", (t) => {
  const dir = temp(t), archive = path.join(dir, "plugin.zip");
  const entries = [
    { name: "plugin.json", data: Buffer.from(JSON.stringify(manifest)) },
    { name: "index.js", data: Buffer.from("") },
    { name: "r16.wasm", data: wasm() },
  ];
  for (const prefix of ["", "r16/"]) {
    fs.writeFileSync(archive, zip(entries.map((e) => ({ ...e, name: prefix + e.name }))));
    assert.deepEqual(checkPlugin(archive).errors, []);
  }
  assert.equal(spawnSync(process.execPath, ["scripts/tmv-plugin.mjs", "check", archive], { cwd: root }).status, 0);
  assert.deepEqual(zip(entries), zip(entries));
  fs.writeFileSync(archive, zip(entries.slice(0, 2)));
  assert.ok(checkPlugin(archive).errors.some((e) => e.includes("wasm")));
  assert.equal(spawnSync(process.execPath, ["scripts/tmv-plugin.mjs", "check", archive], { cwd: root }).status, 1);
  fs.writeFileSync(archive, zip([...entries, { name: "../escape", data: Buffer.from("") }]));
  assert.ok(checkPlugin(archive).errors.includes("unsafe path in zip"));
  fs.writeFileSync(archive, zip(entries.map((e) => ({ ...e, name: "../" + e.name }))));
  assert.ok(checkPlugin(archive).errors.includes("unsafe path in zip"));
  fs.writeFileSync(archive, zip([...entries, entries[0]]));
  assert.ok(checkPlugin(archive).errors.includes("duplicate path in zip"));
});

test("market packer validates and produces the same package layout", (t) => {
  const dir = temp(t), plugins = path.join(dir, "plugins"), source = path.join(plugins, "example");
  fs.mkdirSync(source, { recursive: true });
  const m = { apiVersion: 2, permissions: ["state", "ui"], id: "example", name: "Example", version: "1.0.0", description: "test", main: "index.js" };
  fs.writeFileSync(path.join(source, "plugin.json"), JSON.stringify(m));
  fs.writeFileSync(path.join(source, "index.js"), "// test\n");
  const args = ["scripts/pack-plugins.mjs", path.join(dir, "out")];
  const options = { cwd: root, env: { ...process.env, PLUGINS_DIR: plugins } };
  execFileSync(process.execPath, args, options);
  const output = path.join(dir, "out/plugins");
  const entries = ["plugin.json", "index.js"].map((name) => ({ name, data: fs.readFileSync(path.join(source, name)) }));
  const archive = fs.readFileSync(path.join(output, "example-1.0.0.zip"));
  assert.deepEqual(archive, zip(entries));
  assert.deepEqual(archive, fs.readFileSync(path.join(output, "example.zip")));
  assert.deepEqual(checkPlugin(path.join(output, "example.zip")).errors, []);
  const market = JSON.parse(fs.readFileSync(path.join(output, "plugins.json")));
  assert.equal(market.plugins.length, 2); // One built-in plus our fixture; no template.
  assert.equal(market.plugins[1].size, archive.length);
  assert.equal(market.plugins[1].apiVersion, 2);
  assert.deepEqual(market.plugins[1].permissions, ["state", "ui"]);
  fs.writeFileSync(path.join(source, "plugin.json"), JSON.stringify({ ...m, main: "missing.js" }));
  const invalid = spawnSync(process.execPath, args, options);
  assert.equal(invalid.status, 1);
  assert.match(invalid.stderr.toString(), /main "missing.js" not found/);
});

test("template build packages a fixture module without invoking Rust", (t) => {
  const dir = temp(t), plugin = path.join(dir, "format-plugin");
  fs.cpSync(path.join(root, "templates/format-plugin"), plugin, {
    recursive: true,
    filter: (p) => !["target", "dist"].includes(path.basename(p)),
  });
  const cargo = path.join(dir, "cargo.cjs");
  fs.writeFileSync(cargo, `#!/usr/bin/env node
const fs = require("node:fs"), path = require("node:path"), assert = require("node:assert/strict");
const args = process.argv.slice(2);
for (const arg of ["build", "--target", "wasm32-unknown-unknown", "--release"]) assert.ok(args.includes(arg));
const out = path.join(args[args.indexOf("--target-dir") + 1], "wasm32-unknown-unknown/release");
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, "tmv_plugin_r16.wasm"), Buffer.from(${JSON.stringify([...wasm()])}));
`);
  fs.chmodSync(cargo, 0o755);
  // Use copied helpers, as a standalone template repository would.
  fs.mkdirSync(path.join(plugin, "tools"));
  for (const f of ["tmv-plugin.mjs", "plugin-manifest.mjs", "plugin-zip.mjs"]) fs.copyFileSync(path.join(root, "scripts", f), path.join(plugin, "tools", f));
  execFileSync(process.execPath, [path.join(plugin, "build.mjs")], {
    env: { ...process.env, CARGO: cargo, TMV_PLUGIN_TOOLS: path.join(plugin, "tools") },
  });
  const archive = path.join(plugin, "dist/format-plugin-1.0.0.zip");
  assert.deepEqual(checkPlugin(archive).errors, []);
  const files = execFileSync("unzip", ["-Z1", archive], { encoding: "utf8" }).trim().split("\n");
  assert.deepEqual(files, ["plugin.json", "index.js", "r16.wasm"]);
});
