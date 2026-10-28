// Tauri beforeBundleCommand: helpers are ready before signing/archiving.
// `--pro` (tauri.pro.conf.json) builds them for TifMrcView Pro, which has its
// own identifiers so both editions can be installed side by side.
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = fileURLToPath(new URL('../', import.meta.url));
const target = process.env.TAURI_ENV_TARGET_TRIPLE;
const pro = process.argv.includes('--pro');
const env = { ...process.env, TMV_EDITION: pro ? 'pro' : 'standard' };
function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit', env });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

// Pro packaging files, derived from the standard ones (keep in step with src-tauri/src/edition.rs).
function proHooks(nsh) {
  const out = nsh
    .replace('!define TMV_THUMB_CLSID "{926e1d8f-eb25-48d9-a251-9d27bf165d8a}"', '!define TMV_THUMB_CLSID "{330c748e-26ed-458a-ad27-9b0ce317cde1}"')
    .replaceAll('Software\\com.leehow.tifmrcview\\', 'Software\\com.leehow.tifmrcview.pro\\')
    .replace('"TifMrcView thumbnails"', '"TifMrcView Pro thumbnails"');
  if (!out.includes('330c748e') || out.includes('com.leehow.tifmrcview\\')) throw new Error('thumbnail-hooks.nsh changed; update proHooks');
  return out;
}
function proThumbnailer(text) {
  const out = text.replace(/^TryExec=tifmrcview$/m, 'TryExec=tifmrcview-pro').replace(/^Exec=tifmrcview /m, 'Exec=tifmrcview-pro ');
  if ((out.match(/tifmrcview-pro/g) || []).length !== 2) throw new Error('tifmrcview.thumbnailer changed; update proThumbnailer');
  return out;
}

if (pro) {
  const dir = path.join(root, 'src-tauri/target/pro');
  mkdirSync(dir, { recursive: true });
  const read = (p) => readFileSync(path.join(root, p), 'utf8');
  writeFileSync(path.join(dir, 'thumbnail-hooks.nsh'), proHooks(read('packaging/windows/thumbnail-hooks.nsh')));
  writeFileSync(path.join(dir, 'tifmrcview-pro.thumbnailer'), proThumbnailer(read('packaging/linux/tifmrcview.thumbnailer')));
}
if (process.platform === 'darwin') {
  run('bash', ['scripts/build-quicklook.sh', ...(target ? [target] : [])]);
} else if (process.platform === 'win32') {
  const triple = target || 'x86_64-pc-windows-msvc';
  run('cargo', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--target', triple, '-p', 'tmv-thumbnail-win', ...(pro ? ['--features', 'tmv-thumbnail-win/pro'] : [])]);
  const dest = path.join(root, 'src-tauri/target/thumbnail-windows');
  mkdirSync(dest, { recursive: true });
  copyFileSync(path.join(root, `src-tauri/target/${triple}/release/tmv_thumbnail_win.dll`), path.join(dest, 'tmv_thumbnail_win.dll'));
}
