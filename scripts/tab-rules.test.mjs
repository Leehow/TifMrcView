import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const context = vm.createContext({ window: {} });
vm.runInContext(fs.readFileSync(new URL('../ui/pro/tab-rules.js', import.meta.url), 'utf8'), context);
const R = context.window.TmvTabRules;
const app = fs.readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const documents = fs.readFileSync(new URL('../ui/pro/documents.js', import.meta.url), 'utf8');

test('a replace-open takes the active image tab, never a merge tab', () => {
  assert.equal(R.replacesActive({ type: 'image' }, { replace: true }), true);
  assert.equal(R.replacesActive({ type: 'image' }, null), false);
  assert.equal(R.replacesActive({ type: 'merge' }, { replace: true }), false);
  assert.equal(R.replacesActive(null, { replace: true }), false);
});

test('the 21st tab evicts the stalest idle image tab', () => {
  const a = { type: 'image', used: 5 }, b = { type: 'image', used: 1 }, c = { type: 'image', used: 3 };
  const m = { type: 'merge', used: 0 }, d = { type: 'image', used: 0, dirty: true };
  assert.equal(R.MAX_TABS, 20);
  assert.equal(R.evictionCandidate([a, b, c, m, d], a), b);
  assert.equal(R.evictionCandidate([a, b, c], b), c, 'not the active tab');
  assert.equal(R.evictionCandidate([m, d, a], a), null, 'nothing closable');
});

test('sidebar clicks open here, Ctrl/Cmd, middle click and the menu open a new tab', () => {
  assert.match(app, /e\.metaKey \|\| e\.ctrlKey \? openPath\(f\) : openHere\(f\)/);
  assert.match(app, /openHere\(files\[next\]\)/);
  assert.match(app, /在新标签页打开/);
  assert.match(app, /docs\.open\(path, force, series, \{ replace: true \}\)/);
  assert.match(documents, /async function open\(path, force = false, series = 0, opts = null\)/);
  assert.match(documents, /最多同时打开 \$\{TmvTabRules\.MAX_TABS\} 个标签页，请先关掉几个/);
});

test('inside a project the sidebar is grouped by step and re-listed on request', () => {
  for (const name of ['原始数据', '① 倾转序列', '② 对齐后', '③ 断层图', '④ 清晰版', '⑤ 子断层平均']) assert.ok(app.includes(name), name);
  assert.match(app, /"tmv-folder-changed"/);
  assert.match(app, /\["raw", "原始数据", false\]/, 'only the raw data starts folded');
});

test('restoring Pro tabs retains remote folder identity and recovers local navigation after disconnect', () => {
  const elements = new Map(), calls = [];
  const h = { S: {}, ext: { remoteFiles: new Map([['/cache/image.mrc', {}]]) }, clearTimeout() {},
    $: (id) => { if (!elements.has(id)) elements.set(id, { dataset: {}, hidden: true }); return elements.get(id); },
    basename: (p) => p.split('/').pop(),
    T: { window: { getCurrentWindow: () => ({ setTitle: async () => {} }) } },
    EDITION: { name: 'Pro' }, document: { dispatchEvent() {} }, CustomEvent: class {},
    showRemoteFolder: () => calls.push('remote'), refreshFolder: async () => calls.push('local'),
    toast: () => {},
  };
  for (const name of ['buildChannels', 'buildSeries', 'setupSliders', 'syncScaleUi', 'updateInfo',
    'updateSaveButtons', 'setTool', 'toggleAnn', 'renderMeta', 'specOpened', 'buildStrip', 'markStrip',
    'setModel', 'clampView', 'draw', 'requestRender', 'watchLoad', 'pluginEmit']) h[name] = () => {};
  vm.createContext(h);
  vm.runInContext(documents.slice(documents.indexOf('function restore(doc)'), documents.indexOf('async function activate(doc)')), h);
  const doc = () => ({ name: 'image', state: { gamma: 1, info: { path: '/cache/image.mrc' }, folder: { remote: true, dir: 'ssh:server/data' } } });
  h.restore(doc()); assert.deepEqual(calls, ['remote']);
  h.ext.remoteFiles.clear(); h.restore(doc()); assert.deepEqual(calls, ['remote', 'local']);
  const local = doc(); local.state.folder = { dir: '/local/data' }; h.restore(local);
  assert.equal(elements.get('folder-name').textContent, 'data');
  assert.equal(elements.get('btn-reveal').hidden, false);
});
