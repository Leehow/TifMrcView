// Pro on Linux: the CUDA backend compiles its math functions against
// NVIDIA's libdevice.10.bc, which the packages carry (with NVIDIA's licence:
// CUDA EULA Attachment A lists it as redistributable) so that no CUDA
// toolkit is needed. Taken from NVIDIA's own nvcc redistributable. Used by
// build-thumbnails.mjs (installers) and pack-cli.mjs (the `tmv` tarball).
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import path from 'node:path';

const LIBDEVICE = {
  url: 'https://developer.download.nvidia.com/compute/cuda/redist/cuda_nvcc/linux-x86_64/cuda_nvcc-linux-x86_64-12.9.86-archive.tar.xz',
  archive: '7a1a5b652e5ef85c82b721d10672fc9a2dbaab44e9bd3c65a69517bf53998c35',
  file: '9e84b504f25d26ec775ef91f7122e68dac77fa1f776f1044128f5028356265d8',
};
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

/// Puts nvvm/libdevice/libdevice.10.bc and NVIDIA's LICENSE into `dir`.
export async function libdevice(dir) {
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
  const r = spawnSync('tar', ['-xJf', archive, '-C', dir, '--strip-components=1', '--wildcards', '*/nvvm/libdevice/libdevice.10.bc', '*/LICENSE'], { stdio: 'inherit' });
  if (r.status !== 0) throw new Error('libdevice: tar failed');
  if (sha256(bc) !== LIBDEVICE.file) throw new Error('libdevice.10.bc: checksum mismatch');
}
