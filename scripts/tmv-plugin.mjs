#!/usr/bin/env node
// Checks source crates, unpacked packages and installable zips. No dependencies.
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { validateManifest } from "./plugin-manifest.mjs";

const EXPORTS = {
  tmv_abi: [[], [0x7f]],
  tmv_alloc: [[0x7f], [0x7f]],
  tmv_free: [[0x7f, 0x7f], []],
  tmv_open: [[0x7f, 0x7f], [0x7f]],
  tmv_open_series: [[0x7f, 0x7f, 0x7f], [0x7f]],
  tmv_info: [[0x7f], [0x7e]],
  tmv_page: [[0x7f, 0x7f], [0x7e]],
  tmv_close: [[0x7f], []],
  tmv_metadata: [[0x7f, 0x7f], [0x7e]],
};
const IMPORTS = {
  file_size: [[], [0x7e]],
  read: [[0x7e, 0x7f, 0x7f], [0x7f]],
  decompress: [[0x7f, 0x7f, 0x7f, 0x7f, 0x7f], [0x7f]],
  log: [[0x7f, 0x7f], []],
  error: [[0x7f, 0x7f], []],
};
const FORMAT_EXPORTS = ["tmv_open", "tmv_info", "tmv_page", "tmv_close"];
const CORE_EXTENSIONS = new Set(["tif", "tiff", "mrc", "mrcs", "rec", "st", "map", "png", "jpg", "jpeg", "gif", "bmp", "webp"]);

// WebAssembly types use i32 (0x7f) and i64 (0x7e) for this ABI.
class Reader {
  constructor(bytes) { this.bytes = bytes; this.pos = 0; }
  byte() {
    if (this.pos >= this.bytes.length) throw new Error("truncated wasm section");
    return this.bytes[this.pos++];
  }
  u32() {
    let n = 0;
    for (let shift = 0; shift < 35; shift += 7) {
      const b = this.byte();
      if (shift === 28 && b > 15) throw new Error("invalid wasm u32");
      n += (b & 127) * 2 ** shift;
      if (!(b & 128)) return n;
    }
    throw new Error("invalid wasm u32");
  }
  take(n) {
    if (n > this.bytes.length - this.pos) throw new Error("truncated wasm section");
    const b = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return b;
  }
  name() { return this.take(this.u32()).toString("utf8"); }
  vec(fn) {
    const n = this.u32();
    if (n > this.bytes.length - this.pos) throw new Error("invalid wasm vector length");
    return Array.from({ length: n }, () => fn());
  }
  limits() {
    const flags = this.u32();
    if (flags & ~3) throw new Error("only wasm32 limits are supported");
    this.u32();
    if (flags & 1) this.u32();
  }
}

/** Static ABI checks only: never instantiate or run an untrusted plugin. */
export function checkWasm(bytes, manifest) {
  const errors = [], warnings = [];
  if (bytes.length < 8 || !bytes.subarray(0, 8).equals(Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]))) {
    return { errors: ["not a WebAssembly binary (magic/version)"], warnings };
  }
  if (!WebAssembly.validate(bytes)) return { errors: ["invalid WebAssembly binary"], warnings };
  try {
    const r = new Reader(bytes.subarray(8));
    let types = [], imports = [], functions = [], exports = [];
    while (r.pos < r.bytes.length) {
      const id = r.byte(), s = new Reader(r.take(r.u32()));
      if (id === 1) types = s.vec(() => {
        if (s.byte() !== 0x60) throw new Error("unsupported wasm function type");
        return [s.vec(() => s.byte()), s.vec(() => s.byte())];
      });
      if (id === 2) imports = s.vec(() => {
        const module = s.name(), name = s.name(), kind = s.byte();
        let type;
        if (kind === 0) type = s.u32();
        else if (kind === 1) { s.byte(); s.limits(); }
        else if (kind === 2) s.limits();
        else if (kind === 3) { s.byte(); s.byte(); }
        else if (kind === 4) { s.byte(); s.u32(); }
        else throw new Error("unsupported wasm import kind");
        return { module, name, kind, type };
      });
      if (id === 3) functions = s.vec(() => s.u32());
      if (id === 7) exports = s.vec(() => ({ name: s.name(), kind: s.byte(), index: s.u32() }));
      if ([1, 2, 3, 7].includes(id) && s.pos !== s.bytes.length) throw new Error("unexpected wasm section data");
    }
    const functionTypes = [...imports.filter((i) => i.kind === 0).map((i) => i.type), ...functions];
    const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    for (const i of imports) {
      const expected = i.module === "tmv" && Object.hasOwn(IMPORTS, i.name) ? IMPORTS[i.name] : null;
      if (!expected) warnings.push(`unknown import ${i.module}.${i.name}; the app will refuse this module`);
      else if (i.kind !== 0 || !same(types[i.type], expected)) errors.push(`import ${i.module}.${i.name} has the wrong ABI signature`);
    }
    const required = ["memory", "tmv_abi", "tmv_alloc", "tmv_free"];
    if (manifest.formats?.length) required.push(...FORMAT_EXPORTS);
    if (manifest.metadata) required.push("tmv_metadata");
    if (!FORMAT_EXPORTS.every((n) => exports.some((e) => e.name === n)) && !exports.some((e) => e.name === "tmv_metadata")) {
      errors.push("module needs format exports or tmv_metadata");
    }
    for (const name of required) if (!exports.some((e) => e.name === name)) errors.push(`missing export ${name}`);
    for (const e of exports) {
      if (e.name === "memory" && e.kind !== 2) errors.push("memory export must be a memory");
      if (Object.hasOwn(EXPORTS, e.name) && (e.kind !== 0 || !same(types[functionTypes[e.index]], EXPORTS[e.name]))) {
        errors.push(`export ${e.name} has the wrong ABI signature`);
      }
    }
  } catch (e) { errors.push(e.message); }
  return { errors, warnings };
}

function versionParts(value) {
  if (typeof value !== "string") throw new Error("version must be a string");
  const core = value.trim().replace(/^v+/, "").split(/[-+]/)[0];
  if (!/^\d+(\.\d+)*$/.test(core)) throw new Error(`invalid version "${value}"`);
  return core.split(".").map(BigInt);
}

function newer(a, b) {
  const x = versionParts(a), y = versionParts(b);
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if ((x[i] || 0n) !== (y[i] || 0n)) return (x[i] || 0n) > (y[i] || 0n);
  }
  return false;
}

const safeRel = (s) => typeof s === "string" && s.split("/").every((c) => c && !c.startsWith(".") && /^[\w.-]+$/.test(c));

/** Source crates may omit the wasm until built; packages and zips may not. */
export function checkPlugin(input, { appVersion = "0.6.0" } = {}) {
  const errors = [], warnings = [], notes = [];
  let manifest;
  try {
    const dir = fs.statSync(input).isDirectory();
    let files, read, folder;
    if (dir) {
      read = (name) => fs.readFileSync(path.join(input, name));
      manifest = JSON.parse(read("plugin.json"));
      folder = path.basename(path.resolve(input));
      files = [manifest?.main, manifest?.wasm].filter((f) => safeRel(f) && fs.existsSync(path.join(input, f)) && fs.statSync(path.join(input, f)).isFile());
      if (safeRel(manifest?.wasm) && !files.includes(manifest.wasm) && fs.existsSync(path.join(input, "Cargo.toml"))) {
        files.push(manifest.wasm);
        notes.push("manifest only: wasm not built; ABI checks skipped");
      }
    } else {
      const archive = path.resolve(input);
      const names = execFileSync("unzip", ["-Z1", archive], { encoding: "utf8", maxBuffer: 4 << 20 }).trimEnd().split("\n");
      const real = names.filter((n) => !n.startsWith("__MACOSX/"));
      if (real.some((n) => !safeRel(n.replace(/\/$/, "")))) errors.push("unsafe path in zip");
      const prefix = real[0]?.split("/")[0] + "/";
      const strip = !names.includes("plugin.json") && real.every((n) => n.startsWith(prefix)) ? prefix : "";
      files = real.filter((n) => !n.endsWith("/")).map((n) => n.slice(strip.length));
      if (new Set(files).size !== files.length) errors.push("duplicate path in zip");
      read = (name) => execFileSync("unzip", ["-p", archive, strip + name], { maxBuffer: 256 << 20 });
      if (!files.includes("plugin.json")) throw new Error("plugin.json not found at zip root (or in a single wrapper folder)");
      manifest = JSON.parse(read("plugin.json"));
      folder = manifest?.id;
    }
    errors.push(...validateManifest(manifest, folder, files));
    if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) return { manifest, errors, warnings, notes };
    if (typeof manifest.id === "string" && !/^[a-z0-9-]{1,64}$/.test(manifest.id)) errors.push("id must use at most 64 lowercase letters, digits or - for the current app");
    if (Object.hasOwn(manifest, "apiVersion") && (!Number.isInteger(manifest.apiVersion) || manifest.apiVersion < 1 || manifest.apiVersion > 2)) {
      errors.push("apiVersion is unsupported; this app accepts API version 1 (default) or 2 (sandboxed)");
    }
    try {
      versionParts(appVersion);
      if (manifest.minAppVersion != null && newer(manifest.minAppVersion, appVersion)) errors.push(`minAppVersion ${manifest.minAppVersion} requires a newer app than ${appVersion}`);
    } catch (e) { errors.push(`minAppVersion: ${e.message}`); }
    if (!safeRel(manifest.main) || !/\.m?js$/.test(manifest.main)) errors.push("main must be a safe relative .js or .mjs path");
    if (manifest.wasm != null && !safeRel(manifest.wasm)) errors.push("wasm must be a safe relative path");
    for (const f of Array.isArray(manifest.formats) ? manifest.formats : []) {
      for (const ext of Array.isArray(f?.ext) ? f.ext : []) if (CORE_EXTENSIONS.has(ext)) errors.push(`formats cannot take over core extension .${ext}`);
    }
    if (!errors.length && manifest.wasm && !notes.length) {
      const abi = checkWasm(read(manifest.wasm), manifest);
      errors.push(...abi.errors);
      warnings.push(...abi.warnings);
      notes.push("static ABI checked; tmv_abi() == 1 and runtime behavior are checked by the app on install");
    }
  } catch (e) { errors.push(e.message); }
  return { manifest, errors, warnings, notes };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, input, flag, appVersion] = process.argv.slice(2);
  if (command !== "check" || !input || (flag && (flag !== "--app-version" || !appVersion)) || process.argv.length > 6) {
    console.error("usage: tmv-plugin.mjs check <dir-or-zip> [--app-version 0.6.0]");
    process.exitCode = 1;
  } else {
    const result = checkPlugin(input, { appVersion });
    for (const e of result.errors) console.error(`${input}: error: ${e}`);
    for (const w of result.warnings) console.warn(`${input}: warning: ${w}`);
    for (const n of result.notes) console.log(`${input}: ${n}`);
    if (result.errors.length) process.exitCode = 1;
    else console.log(`${input}: OK`);
  }
}
