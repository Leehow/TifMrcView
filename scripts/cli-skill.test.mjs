// The single-file AGENTS.md carries the same instructions as the skill.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('cli/AGENTS.md matches the tifmrcview-remote skill', () => {
  const skill = readFileSync(new URL('../cli/skills/tifmrcview-remote/SKILL.md', import.meta.url), 'utf8');
  const agents = readFileSync(new URL('../cli/AGENTS.md', import.meta.url), 'utf8');
  const body = (s, mark) => s.slice(s.indexOf(mark) + mark.length);
  assert.match(skill, /^---\nname: tifmrcview-remote\ndescription: .{50,1000}\n---\n/);
  assert.equal(body(agents, 'this file is the single-file version.\n'), body(skill, '# TifMrcView Pro on a remote server (`tmv`)\n'));
});
