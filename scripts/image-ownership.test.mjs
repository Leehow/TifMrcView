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
    window: {}, document: { dispatchEvent: () => {} }, CustomEvent: class {},
  };
  for (const name of ['finishText', 'stopPlay', 'sideDone', 'holdFrame', 'showSize', 'setModel', 'buildChannels', 'buildSeries', 'setupSliders', 'syncScaleUi', 'updateInfo', 'updateSaveButtons', 'renderMeta', 'specOpened', 'clampView', 'draw', 'requestRender', 'fit', 'autoLoadModel', 'loadAnn', 'watchLoad', 'rememberRecent']) context[name] = () => {};
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

const labels = fs.readFileSync(new URL('../ui/pro/labels.js', import.meta.url), 'utf8');
function membranes() {
  const elements = new Map(), pending = deferred(), calls = [], callbacks = [];
  const context = {
    S: { token: 1, info: { sizeZ: 16 }, t: 0 },
    L: { token: 1, busy: false, lastError: '', active: 1 },
    $: (id) => { if (!elements.has(id)) elements.set(id, { classList: { toggle: () => {} } }); return elements.get(id); },
    ready: () => context.L.token === context.S.token,
    EYE: '', EYE_OFF: '',
    T: { event: { listen: async (_event, callback) => { callbacks.push(callback); return () => {}; } } },
    window: { TifMrcViewPro: { membrane: { ensure: async () => true } } },
    editWith: (_command, req) => { calls.push(req); return pending.promise; },
    channel: () => 0, liveStart: () => {}, liveStop: () => {}, liveFetch: () => {},
    draw: () => {}, renderList: () => {}, drawVesicles: () => {}, toast: () => {},
    syncSam: () => {}, syncCls: () => {}, syncUnet: () => {}, any: () => false,
    sec: { querySelectorAll: () => [] },
  };
  vm.createContext(context);
  const state = labels.indexOf('  const M =');
  vm.runInContext(labels.slice(state, labels.indexOf('\n', state)) + '\nglobalThis.M = M;', context);
  vm.runInContext(labels.slice(labels.indexOf('  function syncMem()'), labels.indexOf('  /** Diameter histogram')), context);
  vm.runInContext(labels.slice(labels.indexOf('  function syncPanel()'), labels.indexOf('  function renderList()')), context);
  vm.runInContext(labels.slice(labels.indexOf('  async function memRun()'), labels.indexOf('  $("lab-mem-run").addEventListener')), context);
  return {
    context, pending, calls, callbacks, run: () => context.memRun(),
    text: () => { context.syncPanel(); return elements.get('lab-mem-text').textContent; },
    progress: () => callbacks.at(-1)({ payload: { stage: 'net', done: 1, total: 4 } }),
  };
}

test('a membrane cancellation stays on its image but not a newly opened image', async () => {
  const h = membranes(), done = h.run(); await tick();
  h.context.L.lastError = '已取消'; h.pending.resolve(null); await done;
  assert.equal(h.text(), '分割没有完成：已取消');
  assert.equal(h.text(), '分割没有完成：已取消');
  h.context.S.token = 2; // image open completed before labels_open
  assert.equal(h.text(), '');
  h.context.L.token = 2;
  assert.equal(h.text(), '');
});

for (const success of [false, true]) test(`late membrane ${success ? 'success' : 'failure'} cannot belong to a new image`, async () => {
  const h = membranes(), done = h.run(); await tick();
  h.progress(); const previous = h.context.M.text;
  h.context.S.token = 2; h.context.L.token = 2; h.context.L.active = 9;
  h.progress();
  assert.equal(h.text(), '');
  assert.equal(h.context.M.text, previous, 'ignore old progress writes');
  h.context.L.lastError = 'old failure';
  h.pending.resolve(success ? { components: 1, backend: 'Metal', seconds: 1, label: 3, vesicles: [{ diameter: 4 }] } : null);
  await done;
  assert.equal(h.text(), '');
  assert.equal(h.context.L.active, 9);
  assert.equal(h.context.M.token, 0, 'old vesicles must not acquire the new token');
});

test('changing image during membrane model selection does not start a new-image job', async () => {
  const h = membranes(), selected = deferred();
  h.context.window.TifMrcViewPro.membrane.ensure = () => selected.promise;
  const done = h.run(); await tick();
  h.context.S.token = 2; h.context.L.token = 2;
  selected.resolve(true); await tick();
  // Resolve an unexpected invocation too, so the regression fails without hanging.
  h.pending.resolve(null); await done;
  assert.equal(h.calls.length, 0);
  assert.equal(h.text(), '');
  assert.equal(h.context.M.busy, false);
});

test('queued progress after a membrane job finishes cannot erase its result', async () => {
  const h = membranes(), done = h.run(); await tick();
  h.pending.resolve({ components: 2, backend: 'Metal', seconds: 3, label: 1 }); await done;
  const result = h.text(); h.progress();
  assert.equal(h.text(), result);
  assert.match(result, /找到 2 片膜/);
});

test('an obsolete membrane model error cannot attach to the new image', async () => {
  const h = membranes(), selected = deferred();
  h.context.window.TifMrcViewPro.membrane.ensure = () => selected.promise;
  const done = h.run(); await tick();
  h.context.S.token = 2; h.context.L.token = 2;
  selected.reject(new Error('old model failure')); await done;
  assert.equal(h.text(), '');
  assert.equal(h.context.M.busy, false);
});
