import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('Metal completion ordering and device-loss wakeups', () => {
  const source = fileURLToPath(new URL('../src-tauri/vendor/wgpu-hal-30.0.1/src/metal/completion.rs', import.meta.url));
  const notification = fileURLToPath(new URL('../src-tauri/vendor/cubecl-wgpu-0.11.3/src/compute/loss_notify.rs', import.meta.url));
  const dir = mkdtempSync(join(tmpdir(), 'tmv-metal-completion-'));
  try {
    const driver = join(dir, 'test.rs');
    const binary = join(dir, process.platform === 'win32' ? 'test.exe' : 'test');
    writeFileSync(driver, `#[path = ${JSON.stringify(source)}] mod completion;\n#[path = ${JSON.stringify(notification)}] mod loss_notify;\n`);
    execFileSync('rustc', ['--edition', '2021', '--test', driver, '-o', binary], { stdio: 'pipe' });
    execFileSync(binary, [], { stdio: 'inherit' });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
