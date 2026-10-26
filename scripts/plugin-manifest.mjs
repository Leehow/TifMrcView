// Market manifest rules. Keep the packer and developer checker in sync.
// API 2 permission vocabulary (Rust: package::SANDBOX_PERMISSIONS).
// tmv-plugin.test.mjs also compares the runtime method map to this list.
export const SANDBOX_PERMISSION_NAMES = Object.freeze(["state", "ui", "metadata", "export", "settings"]);
const EXT = /^[a-z0-9]{1,16}$/;
const extList = (v) => Array.isArray(v) && v.length > 0 && v.every((e) => typeof e === "string" && EXT.test(e));

const REQUIRED = ["id", "name", "version", "description", "main"];
export function validateManifest(m, dirName, files) {
  const errs = [];
  if (!m || typeof m !== "object" || Array.isArray(m)) return ["plugin.json must be an object"];
  for (const k of REQUIRED) if (typeof m[k] !== "string" || !m[k]) errs.push(`missing "${k}"`);
  if (m.id !== dirName) errs.push(`id "${m.id}" does not match folder "${dirName}"`);
  if (typeof m.id === "string" && !/^[a-z0-9][a-z0-9._-]*$/.test(m.id)) errs.push("id must be lowercase letters, digits, . _ -");
  if (typeof m.version === "string" && !/^\d+\.\d+\.\d+([-+][\w.]+)?$/.test(m.version)) errs.push("version must look like 1.2.3");
  if (typeof m.main === "string" && !files.includes(m.main)) errs.push(`main "${m.main}" not found in package`);
  if (m.apiVersion != null && !Number.isInteger(m.apiVersion)) errs.push("apiVersion must be an integer");
  if (m.apiVersion === 2) {
    if (!Array.isArray(m.permissions) || !m.permissions.every((p) => SANDBOX_PERMISSION_NAMES.includes(p))) errs.push("API 2 permissions must be a list of known permission names");
    if (Object.hasOwn(m, "native")) errs.push("API 2 plugins must not declare native");
  } else if (m.permissions != null && (!Array.isArray(m.permissions) || !m.permissions.every((p) => SANDBOX_PERMISSION_NAMES.includes(p)))) {
    errs.push("permissions must be a list of known permission names");
  }
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

