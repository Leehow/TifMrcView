import { readReleaseNotes } from './release-notes.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

test('release notes reject missing, blank, placeholder and stale version content', t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tmv-notes-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const file = path.join(dir, 'notes.json');
  assert.throws(() => readReleaseNotes('1.0.0', file));
  for (const data of [
    {version:'0.9.0',features:['新增文件浏览'],fixes:[]},
    {version:'1.0.0',features:[],fixes:[]},
    {version:'1.0.0',features:[' '],fixes:[]},
    {version:'1.0.0',features:['TODO'],fixes:[]},
  ]) { fs.writeFileSync(file, JSON.stringify(data)); assert.throws(() => readReleaseNotes('1.0.0', file)); }
});
