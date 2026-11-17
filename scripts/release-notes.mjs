import fs from "node:fs";
import { fileURLToPath } from "node:url";

/** Both publication paths use the same version-bound, user-facing notes. */
export function readReleaseNotes(version, file = process.env.TMV_RELEASE_NOTES_FILE || fileURLToPath(new URL("../release-notes.json", import.meta.url))) {
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  if (data.version !== version) throw new Error(`更新说明版本 ${data.version} 与发布版本 ${version} 不符；请更新 release-notes.json`);
  const sections = [["features", "新功能"], ["fixes", "修复"]];
  const text = [];
  for (const [key, title] of sections) {
    if (!Array.isArray(data[key]) || data[key].some(s => typeof s !== "string" || !s.trim() || /[\r\n\u0000]/.test(s) || /^(TODO|TBD|待填写)$/i.test(s.trim()))) {
      throw new Error(`更新说明 ${key} 必须包含完整的单行用户文案`);
    }
    if (data[key].length) text.push(`${title}\n${data[key].map(s => `• ${s.trim()}`).join("\n")}`);
  }
  if (!text.length) throw new Error("发布前必须填写新功能或修复说明");
  return text.join("\n\n");
}
