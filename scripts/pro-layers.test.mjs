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
    const ctx = vm.createContext({ window: { devicePixelRatio: dpr }, $: (id) => nodes[id], current: () => doc, request: () => requested++ });
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
