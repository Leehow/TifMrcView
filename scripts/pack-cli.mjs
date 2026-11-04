// Packs the `tmv` command-line tool (TifMrcView Pro without a window) for
// Linux x86_64 servers:
//
//   node scripts/pack-cli.mjs <tmv-binary> <out-dir> <version>
//
// Writes <out-dir>/tmv_<version>_linux-x86_64.tar.gz (+ .sha256) holding
// tmv_<version>/{tmv, cuda/ (NVIDIA libdevice + licence), README.md,
// AGENTS.md, skills/, LICENSE}. The release (make-release.mjs, Pro) lists it
// as files.cli in downloads-pro.json; the site's cli/install.sh unpacks it.
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { libdevice } from './libdevice.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const [bin, out, version] = process.argv.slice(2);
if (!bin || !out || !version) {
  console.error('usage: pack-cli.mjs <tmv-binary> <out-dir> <version>');
  process.exit(1);
}
const name = `tmv_${version}`;
const stage = path.join(out, name);
rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });
copyFileSync(bin, path.join(stage, 'tmv'));
chmodSync(path.join(stage, 'tmv'), 0o755);
await libdevice(path.join(root, 'src-tauri/target/pro/cuda'));
mkdirSync(path.join(stage, 'cuda/nvvm/libdevice'), { recursive: true });
copyFileSync(path.join(root, 'src-tauri/target/pro/cuda/nvvm/libdevice/libdevice.10.bc'), path.join(stage, 'cuda/nvvm/libdevice/libdevice.10.bc'));
copyFileSync(path.join(root, 'src-tauri/target/pro/cuda/LICENSE'), path.join(stage, 'cuda/LICENSE'));
for (const f of ['README.md', 'AGENTS.md']) copyFileSync(path.join(root, 'cli', f), path.join(stage, f));
cpSync(path.join(root, 'cli/skills'), path.join(stage, 'skills'), { recursive: true });
copyFileSync(path.join(root, 'LICENSE'), path.join(stage, 'LICENSE'));
writeFileSync(path.join(stage, 'VERSION'), version + '\n');
const tarball = path.join(out, `${name}_linux-x86_64.tar.gz`);
const r = spawnSync('tar', ['-czf', tarball, '-C', out, '--owner=0', '--group=0', name], { stdio: 'inherit' });
if (r.status !== 0) process.exit(r.status ?? 1);
rmSync(stage, { recursive: true, force: true });
const sum = createHash('sha256').update(readFileSync(tarball)).digest('hex');
writeFileSync(tarball + '.sha256', `${sum}  ${path.basename(tarball)}\n`);
console.log(`${tarball} ${sum}`);
