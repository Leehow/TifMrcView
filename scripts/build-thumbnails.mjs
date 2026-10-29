// Tauri beforeBundleCommand: helpers are ready before signing/archiving.
// `--pro` (tauri.pro.conf.json) builds them for TifMrcView Pro, which has its
// own identifiers so both editions can be installed side by side.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
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
// Pro on Linux: the CUDA backend compiles its math functions against
// NVIDIA's libdevice.10.bc, which the packages carry (with NVIDIA's licence:
// CUDA EULA Attachment A lists it as redistributable) so that no CUDA
// toolkit is needed. Taken from NVIDIA's own nvcc redistributable.
const LIBDEVICE = {
  url: 'https://developer.download.nvidia.com/compute/cuda/redist/cuda_nvcc/linux-x86_64/cuda_nvcc-linux-x86_64-12.9.86-archive.tar.xz',
  archive: '7a1a5b652e5ef85c82b721d10672fc9a2dbaab44e9bd3c65a69517bf53998c35',
  file: '9e84b504f25d26ec775ef91f7122e68dac77fa1f776f1044128f5028356265d8',
};
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
async function libdevice() {
  const dir = path.join(root, 'src-tauri/target/pro/cuda');
  const bc = path.join(dir, 'nvvm/libdevice/libdevice.10.bc');
  if (existsSync(bc) && existsSync(path.join(dir, 'LICENSE')) && sha256(bc) === LIBDEVICE.file) return;
  mkdirSync(dir, { recursive: true });
  const archive = path.join(dir, 'nvcc.tar.xz');
  if (!existsSync(archive) || sha256(archive) !== LIBDEVICE.archive) {
    const res = await fetch(LIBDEVICE.url);
    if (!res.ok) throw new Error(`libdevice download: HTTP ${res.status}`);
    writeFileSync(archive, Buffer.from(await res.arrayBuffer()));
    if (sha256(archive) !== LIBDEVICE.archive) throw new Error('libdevice download: checksum mismatch');
  }
  run('tar', ['-xJf', archive, '-C', dir, '--strip-components=1', '--wildcards', '*/nvvm/libdevice/libdevice.10.bc', '*/LICENSE']);
  if (sha256(bc) !== LIBDEVICE.file) throw new Error('libdevice.10.bc: checksum mismatch');
}
if (pro && process.platform === 'linux') await libdevice();

if (process.platform === 'darwin') {
  run('bash', ['scripts/build-quicklook.sh', ...(target ? [target] : [])]);
} else if (process.platform === 'win32') {
  const triple = target || 'x86_64-pc-windows-msvc';
  run('cargo', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--target', triple, '-p', 'tmv-thumbnail-win', ...(pro ? ['--features', 'tmv-thumbnail-win/pro'] : [])]);
  const dest = path.join(root, 'src-tauri/target/thumbnail-windows');
  mkdirSync(dest, { recursive: true });
  copyFileSync(path.join(root, `src-tauri/target/${triple}/release/tmv_thumbnail_win.dll`), path.join(dest, 'tmv_thumbnail_win.dll'));
}
