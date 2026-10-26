// Tauri beforeBundleCommand: helpers are ready before signing/archiving.
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = fileURLToPath(new URL('../', import.meta.url));
const target = process.env.TAURI_ENV_TARGET_TRIPLE;
function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
if (process.platform === 'darwin') {
  run('bash', ['scripts/build-quicklook.sh', ...(target ? [target] : [])]);
} else if (process.platform === 'win32') {
  const triple = target || 'x86_64-pc-windows-msvc';
  run('cargo', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--target', triple, '-p', 'tmv-thumbnail-win']);
  const dest = path.join(root, 'src-tauri/target/thumbnail-windows');
  mkdirSync(dest, { recursive: true });
  copyFileSync(path.join(root, `src-tauri/target/${triple}/release/tmv_thumbnail_win.dll`), path.join(dest, 'tmv_thumbnail_win.dll'));
}
