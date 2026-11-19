// The window and the `tmv` command can do the same things (ui/pro/features.json):
// every tmv command and every tool button of the window has a row there,
// every row's window action exists in ui/pro/agent.js, and every control a
// row names is still in the page. Adding a feature on one side without the
// other fails here.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const root = new URL('..', import.meta.url).pathname;
const read = (p) => readFileSync(join(root, p), 'utf8');
const { features } = JSON.parse(read('ui/pro/features.json'));

function files(dir) {
  return readdirSync(join(root, dir)).flatMap((f) => {
    const p = join(dir, f);
    return statSync(join(root, p)).isDirectory() ? files(p) : [p];
  });
}
const uiJs = files('ui').filter((p) => p.endsWith('.js'));
const uiText = uiJs.map(read).join('\n') + read('ui/index.html');

/** tmv's commands: the arms of `let result = match cmd {` in src/cli.rs. */
function cliCommands() {
  const src = read('src-tauri/src/cli.rs');
  const start = src.indexOf('let result = match cmd {');
  const block = src.slice(start, src.indexOf('};', start));
  return [...block.matchAll(/^\s*"([a-z]+)"(?:\s*\|\s*"[a-z]+")*\s*=>/gm)].map((m) => m[1]);
}

/** The actions agent.js answers: the keys of its ACTIONS table. */
function appActions() {
  const src = read('ui/pro/agent.js');
  const block = src.slice(src.indexOf('const ACTIONS = {'), src.indexOf('// ---------- the calls'));
  return new Set([...block.matchAll(/^ {4}(\w+): \{ args:/gm)].map((m) => m[1]));
}

test('every tmv command has a feature that the window does too', () => {
  const cmds = cliCommands();
  assert.ok(cmds.length >= 8, `found tmv commands: ${cmds}`);
  for (const c of cmds) {
    const f = features.find((x) => x.cli === c);
    assert.ok(f, `tmv ${c} has no row in ui/pro/features.json`);
    assert.equal(f.app, c, `tmv ${c} should be "tmv app ${c}" in the window too`);
  }
  for (const f of features) if (f.cli) assert.ok(cmds.includes(f.cli), `features.json names tmv ${f.cli}, which src/cli.rs does not have`);
});

test('every feature can be done from the command line', () => {
  const actions = appActions();
  for (const f of features) {
    assert.ok(actions.has(f.app), `${f.id}: agent.js has no action "${f.app}" (tmv app ${f.app})`);
    assert.ok(f.cli || f.cliWhy, `${f.id}: give a tmv command, or cliWhy saying why it only makes sense in the window`);
  }
});

test('every tool button of the window has a feature row', () => {
  const buttons = new Set(features.map((f) => f.ui?.button).filter(Boolean));
  for (const p of uiJs.filter((p) => p.startsWith('ui/pro/'))) {
    for (const m of read(p).matchAll(/\.id = "(btn-[a-z0-9-]+)"/g)) {
      assert.ok(buttons.has(m[1]), `${p} adds the tool button ${m[1]}; add its row (and tmv side) to ui/pro/features.json`);
    }
  }
});

test('the controls the table names are still in the page', () => {
  for (const f of features) {
    for (const id of [f.ui?.button, ...(f.ui?.ids || [])].filter(Boolean)) {
      assert.ok(uiText.includes(`"${id}"`) || uiText.includes(`id="${id}"`), `${f.id}: no control with id ${id} in ui/`);
    }
  }
});
