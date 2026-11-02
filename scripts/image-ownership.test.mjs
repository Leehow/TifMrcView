import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const app = fs.readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const wedge = fs.readFileSync(new URL('../ui/pro/wedge.js', import.meta.url), 'utf8');
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
};
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
const image = (path, token) => ({ token, info: { path, name: path, seriesIndex: 0, width: 16, height: 16, sizeZ: 16, channels: [] } });

function opening(storage = new Map()) {
  const pending = new Map(), elements = new Map(), errors = [], events = [];
  const context = {
    S: { token: 0, info: null, sel: 0, axis: 'xy' },
    sessionStorage: { getItem: (key) => storage.get(key), setItem: (key, value) => storage.set(key, value) },
    $: (id) => { if (!elements.has(id)) elements.set(id, { hidden: true, dataset: {} }); return elements.get(id); },
    invoke: (_command, { path, request }) => { const d = deferred(); d.request = request; pending.set(path, d); return d.promise; },
    flushAnn: async () => {}, refreshFolder: async () => {},
    setTimeout: () => 1, clearTimeout: () => {}, UNIT_M: {},
    T: { window: { getCurrentWindow: () => ({ setTitle: async () => {} }) } }, EDITION: { name: 'test' },
    toast: (s) => errors.push(s), basename: (s) => s, needsPlugin: () => false,
    pluginEmit: (_event, info) => events.push(info.path),
  };
  for (const name of ['finishText', 'stopPlay', 'sideDone', 'holdFrame', 'showSize', 'setModel', 'buildChannels', 'buildSeries', 'setupSliders', 'syncScaleUi', 'updateInfo', 'updateSaveButtons', 'renderMeta', 'specOpened', 'clampView', 'draw', 'requestRender', 'fit', 'autoLoadModel', 'loadAnn', 'watchLoad']) context[name] = () => {};
  const start = app.indexOf('let openRequest =') >= 0 ? app.indexOf('let openRequest =') : app.indexOf('async function openPath(');
  const end = app.indexOf('/** Keeps the current picture', start);
  vm.createContext(context);
  vm.runInContext(app.slice(start, end), context);
  return { context, pending, elements, errors, events, open: (...args) => context.openPath(...args) };
}

test('a normal open still commits the image and emits its open event', async () => {
  const h = opening();
  const done = h.open('A'); await tick();
  h.pending.get('A').resolve(image('A', 1)); await done;
  assert.equal(h.context.S.token, 1);
  assert.equal(h.context.S.info.path, 'A');
  assert.deepEqual(h.events, ['A']);
  assert.equal(h.elements.get('loading').hidden, true);
});

test('the backend receives request order that survives a WebView reload', async () => {
  const storage = new Map();
  const first = opening(storage), a = first.open('A'); await tick();
  const reloaded = opening(storage), b = reloaded.open('B'); await tick();
  assert.ok(reloaded.pending.get('B').request > first.pending.get('A').request);
  first.pending.get('A').resolve(image('A', 1)); await a;
  reloaded.pending.get('B').resolve(image('B', 2)); await b;
});

test('a slow old open cannot replace the newer image', async () => {
  const h = opening();
  const a = h.open('A'); await tick();
  const b = h.open('B'); await tick();
  h.pending.get('B').resolve(image('B', 2)); await b;
  h.pending.get('A').resolve(image('A', 1)); await a;
  assert.equal(h.context.S.info.path, 'B');
  assert.deepEqual(h.events, ['B']);
});

test('an obsolete open error cannot report an error or hide the newer loading indicator', async () => {
  const h = opening();
  const a = h.open('A'); await tick();
  const b = h.open('B'); await tick();
  h.elements.get('loading').hidden = false;
  h.pending.get('A').reject(new Error('old failure')); await a;
  assert.deepEqual(h.errors, []);
  assert.equal(h.elements.get('loading').hidden, false);
  h.pending.get('B').resolve(image('B', 2)); await b;
});

test('annotation flushing cannot reverse the order of open requests', async () => {
  const h = opening(), flush = deferred();
  let n = 0;
  h.context.flushAnn = () => ++n === 1 ? flush.promise : Promise.resolve();
  const a = h.open('A');
  const b = h.open('B'); await tick();
  h.pending.get('B').resolve(image('B', 2)); await b;
  flush.resolve(); await tick();
  h.pending.get('A')?.resolve(image('A', 1)); await a;
  assert.equal(h.context.S.info.path, 'B');
});

test('selecting the displayed image supersedes an in-flight open of another image', async () => {
  const h = opening();
  h.context.S.info = image('B', 2).info; h.context.S.token = 2;
  const a = h.open('A'); await tick();
  const b = h.open('B'); await tick();
  assert.ok(h.pending.has('B'));
  h.pending.get('B').resolve(image('B', 3)); await b;
  h.pending.get('A').resolve(image('A', 1)); await a;
  assert.equal(h.context.S.token, 3);
});

test('an old folder listing cannot replace the newer images folder', async () => {
  const listed = deferred();
  const context = {
    S: { token: 1, info: {}, folder: { dir: 'B', files: ['B/image.mrc'] } },
    dirname: (p) => p.split('/')[0], basename: (p) => p,
    invoke: () => listed.promise, $: () => ({}),
    updateSaveDir: () => {}, buildStrip: () => {}, markStrip: () => {}, folderIndex: () => 0,
  };
  vm.createContext(context);
  vm.runInContext(app.slice(app.indexOf('async function refreshFolder('), app.indexOf('function step(delta)')), context);
  const done = context.refreshFolder('A/image.mrc');
  context.S.token = 2;
  listed.resolve({ files: [], sizes: [] }); await done;
  assert.equal(context.S.folder.dir, 'B');
});

function saving() {
  const calls = [], messages = [], elements = new Map();
  const context = {
    W: { model: { token: 1 }, saving: false }, S: { token: 1 },
    baseToken: () => context.S.token, baseInfo: () => ({ name: `image-${context.S.token}.mrc` }),
    $: (id) => { if (!elements.has(id)) elements.set(id, {}); return elements.get(id); },
    invoke: async (command, req) => { calls.push({ command, req }); return command === 'pick_save_path' ? '/tmp/output.mrc' : 'CPU'; },
    T: { event: { listen: async () => () => {} } },
    status: (s) => messages.push(s), saveDir: () => '/tmp', setSaveDir: () => {}, dirname: () => '/tmp',
    performance: { now: () => 0 },
  };
  vm.createContext(context);
  vm.runInContext(wedge.slice(wedge.indexOf('  async function save()'), wedge.indexOf('  $("wg-save").addEventListener')), context);
  return { context, calls, messages, save: () => context.save() };
}

test('a model from another image cannot be saved under the current image name', async () => {
  const h = saving(); h.context.S.token = 2;
  await h.save();
  assert.equal(h.calls.length, 0);
});

test('changing image while choosing a save path cannot save the previous model', async () => {
  const h = saving(), picked = deferred();
  const invoke = h.context.invoke;
  h.context.invoke = (command, req) => command === 'pick_save_path' ? picked.promise : invoke(command, req);
  const done = h.save(); await tick();
  h.context.S.token = 2; picked.resolve('/tmp/B.mrc'); await done;
  assert.equal(h.calls.filter((c) => c.command === 'wedge_save').length, 0);
});

test('saving identifies the model token at the backend boundary', async () => {
  const h = saving(); await h.save();
  const call = h.calls.find((c) => c.command === 'wedge_save');
  assert.equal(call.req.token, 1);
});
