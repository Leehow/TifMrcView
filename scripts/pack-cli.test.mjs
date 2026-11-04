import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { existsSync, mkdtempSync, mkdirSync, copyFileSync, writeFileSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'tmv-pack-test-'));
  for (const dir of ['scripts', 'cli/skills/test', 'src-tauri/target/pro/cuda/nvvm/libdevice', 'out', 'bin']) mkdirSync(path.join(root, dir), { recursive: true });
  copyFileSync(new URL('./pack-cli.mjs', import.meta.url), path.join(root, 'scripts/pack-cli.mjs'));
  // Packaging tests must never acquire dependencies from the Internet.
  writeFileSync(path.join(root, 'scripts/libdevice.mjs'), 'export async function libdevice() {}\n');
  for (const f of ['cli/README.md', 'cli/AGENTS.md', 'cli/skills/test/SKILL.md', 'LICENSE', 'src-tauri/target/pro/cuda/LICENSE', 'src-tauri/target/pro/cuda/nvvm/libdevice/libdevice.10.bc', 'binary']) writeFileSync(path.join(root, f), 'fixture\n');
  return root;
}

test('a failed CLI pack cleans only its own staging directory', () => {
  const root = fixture();
  try {
    const prior = path.join(root, 'out/tmv_0.0.test');
    mkdirSync(prior);
    writeFileSync(path.join(prior, 'keep'), 'existing output');
    const result = spawnSync(process.execPath, [path.join(root, 'scripts/pack-cli.mjs'), path.join(root, 'absent'), path.join(root, 'out'), '0.0.test']);
    assert.notEqual(result.status, 0);
    assert.equal(readFileSync(path.join(prior, 'keep'), 'utf8'), 'existing output');
    assert.deepEqual(readdirSync(path.join(root, 'out')), ['tmv_0.0.test']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('tar failure also removes CLI staging', { skip: process.platform === 'win32' }, () => {
  const root = fixture();
  try {
    const tarball = path.join(root, 'out/tmv_0.0.test_linux-x86_64.tar.gz');
    writeFileSync(tarball, 'valid previous archive');
    writeFileSync(tarball + '.sha256', 'previous checksum');
    writeFileSync(path.join(root, 'bin/tar'), '#!/bin/sh\nprintf partial > "$2"\nexit 23\n', { mode: 0o755 });
    const result = spawnSync(process.execPath, [path.join(root, 'scripts/pack-cli.mjs'), path.join(root, 'binary'), path.join(root, 'out'), '0.0.test'], { env: { ...process.env, PATH: `${path.join(root, 'bin')}:${process.env.PATH}` } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr.toString(), /CLI archive failed: 23/);
    assert.equal(readFileSync(tarball, 'utf8'), 'valid previous archive');
    assert.equal(readFileSync(tarball + '.sha256', 'utf8'), 'previous checksum');
    assert.deepEqual(readdirSync(path.join(root, 'out')).sort(), ['tmv_0.0.test_linux-x86_64.tar.gz', 'tmv_0.0.test_linux-x86_64.tar.gz.sha256']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('termination during preparation cleans CLI staging', { skip: process.platform === 'win32' }, async () => {
  const root = fixture();
  let child;
  try {
    writeFileSync(path.join(root, 'scripts/libdevice.mjs'), `import { writeFileSync } from 'node:fs';
      export async function libdevice() {
        writeFileSync(new URL('../ready', import.meta.url), 'ready');
        await new Promise(() => setInterval(() => {}, 1000));
      }`);
    child = spawn(process.execPath, [path.join(root, 'scripts/pack-cli.mjs'), path.join(root, 'binary'), path.join(root, 'out'), '0.0.test'], { stdio: 'ignore' });
    const finished = once(child, 'exit');
    for (let i = 0; i < 100 && !existsSync(path.join(root, 'ready')); i++) await delay(20);
    assert.ok(existsSync(path.join(root, 'ready')), 'pack must reach preparation');
    child.kill('SIGTERM');
    const [code, signal] = await finished;
    assert.equal(code, 143);
    assert.equal(signal, null);
    assert.deepEqual(readdirSync(path.join(root, 'out')), []);
  } finally {
    if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    rmSync(root, { recursive: true, force: true });
  }
});
