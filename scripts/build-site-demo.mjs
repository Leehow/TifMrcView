// Copies the viewer UI (ui/) into the website as a live demo, so the demo on
// tifmrcview.com always shows the current interface. site/demo/ holds the
// pieces that make it run in a browser: tauri-mock.js (a stand-in backend
// serving the samples in data/) and demo.css (hides what needs a computer).
//
//   node scripts/build-site-demo.mjs <site output folder>
//
// Writes <out>/demo/app/. The site workflow runs it after copying site/;
// for a local preview run it with `site` and open site/index.html via a
// web server (site/demo/app/ is git-ignored).
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(process.argv[2] || "site", "demo", "app");
mkdirSync(join(out, "plugins"), { recursive: true });

const files = [
  "style.css",
  "app.js",
  "plugin-host.js",
  "plugin-sandbox.js",
  "plugin-market.js",
  "plugin-dialog.js",
  "plugins/i18n.js",
];
for (const f of files) cpSync(join(root, "ui", f), join(out, f));

let html = readFileSync(join(root, "ui", "index.html"), "utf8");
function insert(anchor, text) {
  if (!html.includes(anchor)) throw new Error(`ui/index.html no longer has ${anchor}; update build-site-demo.mjs`);
  html = html.replace(anchor, text + anchor);
}
insert("</head>", '  <meta name="robots" content="noindex" />\n  <link rel="stylesheet" href="../demo.css" />\n');
insert('<script src="app.js"></script>', '<script src="../tauri-mock.js"></script>\n  ');
writeFileSync(join(out, "index.html"), html);
console.log(`demo UI written to ${out}`);
