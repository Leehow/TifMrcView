import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const context = vm.createContext({ window: {} });
vm.runInContext(fs.readFileSync(new URL('../ui/pro/merge-model.js', import.meta.url), 'utf8'), context);
const M = context.window.TmvMerge;
const plain = (x) => JSON.parse(JSON.stringify(x));
function source(path, token, depth = 1) {
  return { token, info: { path, name: path, width: 320, height: 240, sizeZ: depth, sizeT: 2, seriesIndex: 0,
    channels: [{ name: 'Gray', color: [255, 255, 255] }] } };
}

test('each stack chooses Z independently and a single image remains fixed', () => {
  const doc = M.create();
  const still = M.add(doc, M.layer(source('BF.tif', 1)));
  const short = M.add(doc, M.layer(source('green.mrc', 2, 3)));
  const deep = M.add(doc, M.layer(source('red.mrc', 3, 11)));
  M.setZ(short, 2); M.setZ(deep, 8); M.setZ(still, 8);
  assert.deepEqual(plain(doc.layers.map((l) => l.z)), [0, 2, 8]);
  assert.equal(still.mode, 'normal');
  assert.equal(short.mode, 'screen');
});

test('saved project retains editable state without runtime image handles', () => {
  const doc = M.create('test');
  const a = M.add(doc, M.layer(source('BF.tif', 1)));
  const b = M.add(doc, M.layer(source('green.mrc', 2, 7)));
  Object.assign(a, { visible: false });
  Object.assign(b, { x: -12.5, y: 18, width: 234, height: 177, opacity: 0.37, color: '#00ff00', mode: 'add', z: 5, t: 1 });
  b.channels[0].min = 42; b.channels[0].max = 16384;
  doc.view = { cx: 88, cy: 92, scale: 1.7 };
  const data = plain(M.validate(M.project(doc)));
  const reopened = M.create(data.name);
  Object.assign(reopened, { width: data.width, height: data.height, background: data.background, view: data.view });
  for (const [i, settings] of data.layers.entries()) reopened.layers.push(M.layer(source(settings.path, i + 90, i ? 7 : 1), settings));
  reopened.selected = reopened.layers[data.selected].id;
  assert.deepEqual(plain(M.project(reopened)), data);
  assert.equal(data.layers[1].token, undefined);
  assert.equal(data.layers[1].info, undefined);
  assert.equal(data.layers[1].id, undefined);
});

test('a saved copy is independent from later unsaved edits', () => {
  const doc = M.create(); const item = M.add(doc, M.layer(source('A', 1, 6)));
  const data = M.project(doc);
  item.channels[0].color[0] = 0; item.z = 5;
  assert.equal(data.layers[0].channels[0].color[0], 255);
  assert.equal(data.layers[0].z, 3);
});

test('invalid projects fail explicitly rather than silently dropping layers', () => {
  const doc = M.create(); M.add(doc, M.layer(source('A', 1)));
  const valid = plain(M.project(doc));
  for (const update of [
    (p) => { p.version = 99; }, (p) => { p.width = 0; },
    (p) => { p.layers[0].path = ''; }, (p) => { p.layers[0].opacity = 1.1; },
    (p) => { p.layers[0].color = 'bad'; }, (p) => { p.layers[0].z = -1; },
    (p) => { p.layers[0].channels[0].color = [500, 0, 0]; },
  ]) { const data = structuredClone(valid); update(data); assert.throws(() => M.validate(data)); }
});

test('converting an image retains its display and view as the base layer', () => {
  const image = source('BF.tif', 17, 7);
  const state = { z: 5, t: 1, gamma: 1.3, crop: null, view: { cx: 137, cy: 109, scale: 2.75 },
    chans: [{ visible: true, color: [12, 255, 31], min: 64, max: 18000, auto: false }] };
  const doc = M.fromImage(image, state, 'BF_merge');
  assert.equal(doc.width, 320); assert.equal(doc.height, 240);
  assert.deepEqual(plain(doc.view), state.view);
  assert.equal(doc.layers[0].token, 17);
  assert.equal(doc.layers[0].z, 5); assert.equal(doc.layers[0].t, 1);
  assert.equal(doc.layers[0].gamma, 1.3);
  assert.equal(doc.layers[0].channels[0].min, 64);
  assert.equal(doc.layers[0].mode, 'normal');
});

test('conversion keeps a cropped image inside the same canvas bounds', () => {
  const state = { z: 0, t: 0, gamma: 1, crop: [25, 31, 100, 70],
    view: { cx: 90, cy: 81, scale: 4 },
    chans: [{ visible: true, color: [255, 255, 255], min: null, max: null, auto: true }] };
  const doc = M.fromImage(source('crop.tif', 1), state, 'crop_merge');
  assert.equal(doc.width, 100); assert.equal(doc.height, 70);
  assert.equal(doc.layers[0].x, -25); assert.equal(doc.layers[0].y, -31);
  assert.deepEqual(plain(doc.view), { cx: 65, cy: 50, scale: 4 });
});

test('changing sidebar or Z bar size preserves image screen position and zoom', () => {
  const view = { cx: 133, cy: 89, scale: 2.75 };
  const before = { width: 1120, height: 730 }, after = { width: 980, height: 666 };
  const next = M.reframe(view, before, after);
  assert.equal(next.scale, view.scale);
  assert.ok(Math.abs((before.width / 2 - view.cx * view.scale) - (after.width / 2 - next.cx * next.scale)) < 1e-9);
  assert.ok(Math.abs((before.height / 2 - view.cy * view.scale) - (after.height / 2 - next.cy * next.scale)) < 1e-9);
  assert.deepEqual(view, { cx: 133, cy: 89, scale: 2.75 });
});

test('new layers fit and center in the base canvas at any source pixel size', () => {
  for (const [width, height, expectedWidth, expectedHeight] of [
    [640, 480, 320, 240], [80, 60, 320, 240], [800, 200, 320, 80], [100, 900, 80 / 3, 240],
  ]) {
    const doc = M.create(); M.add(doc, M.layer(source('base.tif', 1)));
    const image = source('new.tif', 2); Object.assign(image.info, { width, height });
    const layer = M.add(doc, M.layer(image));
    assert.ok(Math.abs(layer.width - expectedWidth) < 1e-9);
    assert.ok(Math.abs(layer.height - expectedHeight) < 1e-9);
    assert.ok(Math.abs(layer.width / layer.height - width / height) < 1e-9);
    assert.ok(Math.abs(layer.x + layer.width / 2 - doc.width / 2) < 1e-9);
    assert.ok(Math.abs(layer.y + layer.height / 2 - doc.height / 2) < 1e-9);
    assert.equal(doc.layers[0].width, 320); assert.equal(doc.layers[0].height, 240);
  }
});

test('all eight transform handles preserve the opposite anchor and aspect ratio', () => {
  const rect = { x: 23, y: 41, width: 320, height: 240 };
  assert.equal(M.handles.length, 8);
  for (const [handle, hx, hy] of M.handles) {
    const next = M.resize(rect, handle, (hx - 0.5) * 64, (hy - 0.5) * 48);
    const ax = 1 - hx, ay = 1 - hy;
    assert.ok(Math.abs(next.width / next.height - 4 / 3) < 1e-9, handle);
    assert.ok(Math.abs(next.x + next.width * ax - rect.x - rect.width * ax) < 1e-9, handle);
    assert.ok(Math.abs(next.y + next.height * ay - rect.y - rect.height * ay) < 1e-9, handle);
    assert.ok(next.width > rect.width, handle);
  }
  const free = M.resize(rect, 'nw', 40, 10, false);
  assert.deepEqual(plain(free), { x: 63, y: 51, width: 280, height: 230 });
  const minimum = M.resize(rect, 'nw', 10000, 10000);
  assert.ok(minimum.width >= 1 && minimum.height >= 1);
});

const app = fs.readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const body = (start, end) => app.slice(app.indexOf(start), app.indexOf(end, app.indexOf(start)));
const sourceUi = body('function sourceUiActive()', 'function resizeCanvas()');
function renderingContext() {
  const calls = [], pending = [];
  let doc = { type: 'image' };
  const ctx = vm.createContext({
    window: { TifMrcViewPro: { documents: { current: () => doc } }, devicePixelRatio: 1 },
    S: { token: 17, info: {}, axis: 'xy', view: { scale: 9.74 }, z: 1, t: 0, gamma: 1, chans: [], frame: 'source', ov: 'source' },
    visibleRegion: () => [0, 0, 32, 24], bounds: () => [0, 0, 32, 24], channelReqs: () => [], OV_SIZE: 128,
    invoke: () => new Promise((resolve) => pending.push(resolve)),
    parseFrame: () => { calls.push('decode'); return { w: 32, h: 24, image: {}, ranges: [] }; },
    frameCanvas: {}, frameCtx: { putImageData: () => calls.push('frame') },
    ovCanvas: { getContext: () => ({ putImageData: () => calls.push('overview') }) },
  });
  for (const name of ['syncContrast', 'draw', 'sideDone', 'startFade', 'loadHist', 'pluginEmit', 'scheduleFft', 'scheduleSpec', 'toast']) ctx[name] = () => calls.push(name);
  vm.runInContext(sourceUi + body('function requestRender()', '/** Re-renders the whole-image') +
    body('async function refreshOverview()', 'function channelReqs()') +
    body('async function renderNow()', '// ---------- FFT / power spectrum'), ctx);
  return { ctx, calls, pending, select: (type) => { doc = { type }; } };
}

test('source draw, status and new render requests cannot overwrite a merge', async () => {
  const h = renderingContext(); h.select('merge');
  vm.runInContext(body('function draw()', '/** Dashed outline') + body('function updateInfo()', 'function togglePlay()'), h.ctx);
  // No DOM/canvas stubs: any source UI write here would throw.
  h.ctx.draw(); h.ctx.updateInfo(); h.ctx.requestRender();
  await h.ctx.renderNow(); await h.ctx.refreshOverview();
  assert.equal(h.pending.length, 0); assert.deepEqual(h.calls, []);
  h.select('image'); h.ctx.requestRender();
  assert.equal(h.pending.length, 1, 'source rendering resumes on return');
  h.select('merge'); h.pending.shift()(new ArrayBuffer(0));
  await Promise.resolve();
});

test('in-flight source frame and overview results are discarded on entering a merge', async () => {
  for (const method of ['renderNow', 'refreshOverview']) {
    const h = renderingContext();
    const done = h.ctx[method]();
    assert.equal(h.pending.length, 1);
    h.select('merge'); h.pending.shift()(new ArrayBuffer(0)); await done;
    assert.deepEqual(h.calls, [], 'no source pixels, overlays or status may be published');
    assert.equal(h.ctx.S.frame, 'source'); assert.equal(h.ctx.S.ov, 'source');
    assert.equal(h.ctx.S.view.scale, 9.74);
    assert.equal(!!h.ctx.S.inflight, false); assert.equal(!!h.ctx.S.ovBusy, false);
  }
});

test('late source load progress cannot replace the merge status or keep polling', async () => {
  const h = renderingContext(); let tick;
  h.ctx.setTimeout = (fn) => { tick = fn; return 1; };
  vm.runInContext(body('function watchLoad()', 'function sideDone()'), h.ctx);
  h.ctx.watchLoad();
  const done = tick(); assert.equal(h.pending.length, 1);
  h.select('merge'); h.pending.shift()([1, 10, true]); await done;
  assert.equal(h.ctx.S.loadTimer, null);
});

test('Retina merge zoom matches the source and the 1 key selects physical 100%', () => {
  const layers = fs.readFileSync(new URL('../ui/pro/layers.js', import.meta.url), 'utf8');
  const update = layers.slice(layers.indexOf('  function updateZoom('), layers.indexOf('  function sync()'));
  const key = layers.slice(layers.indexOf('    key(e) {'), layers.indexOf('  } };', layers.indexOf('    key(e) {')));
  for (const dpr of [1, 2]) {
    const sourceState = { view: { cx: 131, cy: 90, scale: 2.86 }, chans: [], z: 0, t: 0, gamma: 1 };
    const doc = M.fromImage(source('BF.tif', 1), sourceState);
    const nodes = { 'btn-zoom': {}, 'st-zoom': {} };
    let requested = 0, prevented = false;
    const ctx = vm.createContext({ window: { devicePixelRatio: dpr }, $: (id) => nodes[id], current: () => doc, D: { touch: () => {} }, request: () => requested++ });
    vm.runInContext(update + `globalThis.keys = { ${key} } };`, ctx);
    ctx.updateZoom(doc);
    const sourcePct = `${Math.round(sourceState.view.scale * dpr * 100)}%`;
    assert.equal(nodes['btn-zoom'].textContent, sourcePct);
    assert.equal(nodes['st-zoom'].textContent, sourcePct);
    assert.deepEqual(plain(doc.view), sourceState.view, 'DPR changes labels without changing project geometry');
    ctx.keys.key({ key: '1', preventDefault: () => { prevented = true; } });
    ctx.updateZoom(doc);
    assert.equal(doc.view.scale, 1 / dpr);
    assert.equal(nodes['btn-zoom'].textContent, '100%');
    assert.equal(nodes['st-zoom'].textContent, '100%');
    assert.equal(doc.view.cx, 131); assert.equal(doc.view.cy, 90);
    assert.equal(requested, 1); assert.equal(prevented, true);
  }
});

const documents = fs.readFileSync(new URL('../ui/pro/documents.js', import.meta.url), 'utf8');
const turn = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function autosaving() {
  const values = new Map(), timers = new Map(), calls = [];
  let timer = 0;
  const ctx = vm.createContext({
    TmvMerge: M, saveTimers: new Map(), saveTails: new WeakMap(), active: null,
    pref: { get: (key, fallback) => values.get(key) ?? fallback, set: (key, value) => values.set(key, structuredClone(value)) },
    dirname: (s) => s.slice(0, s.lastIndexOf('/')),
    stem: (s) => s.slice(s.lastIndexOf('/') + 1).replace(/\.[^.]+$/, ''),
    setTimeout: (fn) => { timers.set(++timer, fn); return timer; }, clearTimeout: (id) => timers.delete(id),
    document: { dispatchEvent: () => {} }, CustomEvent: class {}, changed: () => {}, rememberProject: () => {},
    invoke: (command, args) => new Promise((resolve, reject) => calls.push({ command, args: structuredClone(args), resolve, reject })),
  });
  vm.runInContext(documents.slice(documents.indexOf('  function recoveryList()'), documents.indexOf('  function recover(id)')), ctx);
  const doc = M.create('merge'); M.add(doc, M.layer(source('/data/base.tif', 7)));
  return { ctx, doc, values, timers, calls };
}

test('autosave coalesces edits, then flushes a newer revision arriving during the first write', async () => {
  const h = autosaving(), layer = h.doc.layers[0];
  layer.opacity = 0.2; h.ctx.touch(h.doc);
  layer.opacity = 0.4; h.ctx.touch(h.doc);
  assert.equal(h.timers.size, 1); assert.equal(h.calls.length, 0);
  assert.equal(h.values.get('merge-recovery')[0].project.layers[0].opacity, 0.4);
  const saved = h.ctx.save(h.doc); await turn();
  assert.equal(h.calls[0].command, 'create_merge_project');
  assert.equal(h.calls[0].args.token, 7);
  assert.equal(h.calls[0].args.project.layers[0].opacity, 0.4);
  assert.equal(h.doc.dirty, true);
  layer.opacity = 0.7; h.ctx.touch(h.doc);
  h.calls[0].resolve('/data/merge.tmvmerge'); await turn();
  assert.equal(h.calls[1].command, 'save_merge_project');
  assert.equal(h.calls[1].args.path, '/data/merge.tmvmerge');
  assert.equal(h.calls[1].args.project.layers[0].opacity, 0.7);
  assert.equal(h.doc.dirty, true);
  h.calls[1].resolve(); assert.equal(await saved, true);
  assert.equal(h.doc.dirty, false); assert.equal(h.values.get('merge-recovery').length, 0);
});

test('failed autosave retains the editable draft and retries without losing state', async () => {
  const h = autosaving(); h.doc.layers[0].color = '#ff0000';
  const failed = h.ctx.save(h.doc); await turn(); h.calls[0].reject(new Error('disk full'));
  assert.equal(await failed, false); assert.equal(h.doc.dirty, true);
  assert.match(h.doc.saveError, /disk full/);
  assert.equal(h.values.get('merge-recovery')[0].project.layers[0].color, '#ff0000');
  const retry = h.ctx.save(h.doc); await turn(); h.calls[1].resolve('/data/merge.tmvmerge');
  assert.equal(await retry, true); assert.equal(h.doc.saveError, null);
  assert.equal(h.doc.layers[0].color, '#ff0000');
});

test('concurrent flush requests serialize writes and create a project only once', async () => {
  const h = autosaving();
  const first = h.ctx.save(h.doc), second = h.ctx.save(h.doc); await turn();
  assert.equal(h.calls.length, 1);
  h.calls[0].resolve('/data/merge.tmvmerge');
  assert.equal(await first, true); assert.equal(await second, true);
  assert.equal(h.calls.length, 1);
});

test('collision-safe allocation updates the project name and recovery path before final flush', async () => {
  const h = autosaving();
  const saved = h.ctx.save(h.doc); await turn();
  h.calls[0].resolve('/data/merge (2).tmvmerge'); await turn();
  assert.equal(h.doc.name, 'merge (2)');
  assert.equal(h.calls[1].command, 'save_merge_project');
  assert.equal(h.calls[1].args.project.name, 'merge (2)');
  assert.equal(h.values.get('merge-recovery')[0].path, '/data/merge (2).tmvmerge');
  assert.equal(h.doc.dirty, true);
  h.calls[1].resolve(); assert.equal(await saved, true);
});

test('deleting the last layer before initial autosave retains a recoverable source anchor', () => {
  const h = autosaving(); h.ctx.autosave(h.doc);
  h.doc.layers = []; h.ctx.touch(h.doc);
  const row = h.values.get('merge-recovery')[0];
  assert.equal(row.project.layers.length, 0);
  assert.deepEqual(row.anchor, { path: '/data/base.tif', series: 0 });
  assert.equal(h.doc.autosaveToken, 7);
});

function barSnapshot(image, state = null) {
  const ctx = vm.createContext({
    UNIT_M: { 'Å': 1e-10, nm: 1e-9, 'µm': 1e-6, mm: 1e-3, '1/µm': 1e6, '1/nm': 1e9, '1/Å': 1e10 },
    isRecip: (u) => u.startsWith('1/'), normUnit: (u) => u,
    plausiblePixel: () => true, pref: { get: (_key, fallback) => fallback },
  });
  vm.runInContext(body('function niceLength(', '/** Draws a scale bar') + body('function fmtLength(', 'const pref =') +
    documents.slice(documents.indexOf('  function scaleBarSnapshot('), documents.indexOf('  function changed(')), ctx);
  return ctx.scaleBarSnapshot(image, state);
}

test('the base scale bar retains manual calibration and follows crop, move and resize', () => {
  const image = source('/data/base.tif', 1);
  const state = { z: 0, t: 0, gamma: 1, chans: [], px: { x: 0.5, y: 0.5, unit: 'nm' }, scaleBar: true,
    view: { cx: 90, cy: 81, scale: 3 }, crop: [25, 31, 100, 70] };
  const bar = barSnapshot(image, state);
  assert.equal(bar.label, '20 nm');
  const doc = M.fromImage(image, state, 'base', bar), base = doc.layers[0]; base.visible = true;
  const before = M.scaleBarGeometry(base);
  assert.ok(Math.abs(before.width - 40) < 1e-9);
  assert.ok(Math.abs(before.physicalLength - 20) < 1e-9);
  assert.equal(before.unit, 'nm');
  assert.ok(before.x > 0 && before.x + before.width < doc.width, 'bar stays inside the cropped base');
  base.x += 17; base.y += 23;
  const moved = M.scaleBarGeometry(base);
  assert.ok(Math.abs(moved.x - before.x - 17) < 1e-9);
  assert.ok(Math.abs(moved.y - before.y - 23) < 1e-9);
  base.width *= 2; base.height *= 2;
  const resized = M.scaleBarGeometry(base);
  assert.equal(resized.width, before.width * 2); assert.equal(resized.font, before.font * 2);
  assert.equal(resized.physicalLength, before.physicalLength); assert.equal(resized.label, before.label);
  state.px.x = 99; assert.equal(base.scaleBar.pixelSize.x, 0.5);
  base.visible = false; assert.equal(M.scaleBarGeometry(base), null);
  assert.equal(barSnapshot(image), null, 'unknown calibration is never invented');
});

test('calibrated scale bars survive project roundtrip; legacy projects stay readable', () => {
  const image = source('/data/base.tif', 1); image.info.pixelSize = [0.5, 0.5, 0.5]; image.info.unit = 'nm';
  const doc = M.create(); M.add(doc, M.layer(image, { scaleBar: barSnapshot(image) }));
  const data = plain(M.validate(M.project(doc)));
  const reopened = M.layer(source('/data/base.tif', 9), data.layers[0]);
  assert.deepEqual(plain(M.scaleBarGeometry(reopened)), plain(M.scaleBarGeometry(doc.layers[0])));
  for (const change of [(b) => { b.pixelSize.x = 0; }, (b) => { b.font = null; }, (b) => { b.visible = 'yes'; }, (b) => { b.region[2] = 0; }]) {
    const bad = structuredClone(data); change(bad.layers[0].scaleBar); assert.throws(() => M.validate(bad));
  }
  delete data.layers[0].scaleBar; assert.doesNotThrow(() => M.validate(data));
});

test('scale-bar visibility changes cannot mutate an undo snapshot', () => {
  const layers = fs.readFileSync(new URL('../ui/pro/layers.js', import.meta.url), 'utf8');
  const image = source('/data/base.tif', 1); image.info.pixelSize = [1, 1, 1]; image.info.unit = 'nm';
  const doc = M.create(); M.add(doc, M.layer(image, { scaleBar: barSnapshot(image) }));
  const ctx = vm.createContext({ M });
  vm.runInContext(layers.slice(layers.indexOf('  function state(doc)'), layers.indexOf('  function remember(doc)')), ctx);
  const saved = ctx.state(doc); doc.layers[0].scaleBar.visible = false;
  assert.equal(saved.layers[0].scaleBar.visible, true);
});

test('recovery registers an existing save path before restoring its newer draft', async () => {
  for (const readable of [true, false]) {
    const h = autosaving(), opened = [];
    h.doc.path = '/data/merge.tmvmerge'; h.doc.layers[0].opacity = 0.63;
    h.ctx.autosave(h.doc);
    h.ctx.enqueue = (run) => run(); h.ctx.openProject = (path, row) => { opened.push({ path, row }); return row; };
    vm.runInContext(documents.slice(documents.indexOf('  function recover(id)'), documents.indexOf('  function rememberProject(doc)')), h.ctx);
    const recovered = h.ctx.recover(h.doc.autosaveId); await turn();
    assert.equal(h.calls[0].command, 'read_merge_project'); assert.equal(opened.length, 0);
    if (readable) h.calls[0].resolve({}); else h.calls[0].reject(new Error('missing'));
    await recovered;
    assert.equal(opened[0].path, readable ? '/data/merge.tmvmerge' : null);
    assert.equal(opened[0].row.project.layers[0].opacity, 0.63);
  }
});

test('silently failed local storage is detected while disk autosave can still succeed', async () => {
  const h = autosaving(); h.ctx.pref.set = () => {};
  h.ctx.autosave(h.doc); assert.match(h.doc.recoveryError, /未能写入/);
  const saved = h.ctx.save(h.doc); await turn(); h.calls[0].resolve('/data/merge.tmvmerge');
  assert.equal(await saved, true); assert.equal(h.doc.recoveryError, null); assert.equal(h.doc.dirty, false);
});
