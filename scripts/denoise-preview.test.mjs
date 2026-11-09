import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const tick = () => new Promise((resolve) => setTimeout(resolve, 10));

function preview(kind) {
  const wedge = kind === 'wedge';
  const source = fs.readFileSync(new URL(`../ui/pro/${kind}.js`, import.meta.url), 'utf8');
  const pending = [], calls = [], messages = [], drawn = [];
  const state = { busy: false, training: false, saving: false, live: false, model: { token: 1 },
    key: '', want: '', res: null, generation: 0, cache: new Map() };
  const S = { token: 1, axis: 'xy', z: 3, t: 0, info: { sizeZ: 20 }, view: { scale: 1 } };
  const context = {
    W: state, N: state, S, panel: { hidden: false },
    $: () => ({ checked: true }), visibleRegion: () => [0, 0, 2, 2],
    baseToken: () => S.token, fftChannel: () => 0, params: () => ({ method: 'nlm' }),
    status: (s) => messages.push(s), requestRender: () => {},
    setTimeout, clearTimeout, TextDecoder,
    window: { devicePixelRatio: 1 }, toScreen: (x, y) => [x, y],
    previewCanvas: () => state.res.values[0],
    invoke: (name, { req }) => {
      calls.push(req);
      return new Promise((resolve, reject) => pending.push({ resolve, reject }));
    },
  };
  vm.createContext(context);
  const start = source.indexOf('  // ---- Preview ----');
  const end = source.indexOf(wedge ? '  /** The preview as an image' : '  /** 0.5 and 99.5', start);
  vm.runInContext(source.slice(start, end), context);
  vm.runInContext(source.slice(source.indexOf('  function overlay('), source.indexOf('  ext.overlays.push(overlay)')), context);
  const graphics = new Proxy({}, { get: (_, key) => key === 'drawImage' ? (value) => drawn.push(value) : () => {}, set: () => true });
  function complete(value = 7) {
    const b = new ArrayBuffer((wedge ? 44 : 40) + 16);
    const d = new DataView(b);
    [0, 0, 2, 2, ...(wedge ? [2, 2] : [])].forEach((v, i) => d.setUint32(i * 4, v, true));
    new Float32Array(b, wedge ? 44 : 40).fill(value);
    pending.shift().resolve(b);
  }
  return { state, S, context, calls, messages, complete, run: () => context.runPreview(), draw: () => { drawn.length = 0; context.overlay(graphics); return [...drawn]; } };
}

for (const kind of ['denoise', 'wedge']) {
  test(`${kind}: returning to a computed Z layer reuses its preview`, async () => {
    const h = preview(kind);
    let done = h.run(); h.complete(3); await done;
    h.S.z = 4; done = h.run(); h.complete(4); await done;
    h.S.z = 3;
    assert.deepEqual(h.draw(), [3], 'cached layer must draw immediately, before polling');
    done = h.run();
    assert.equal(h.calls.length, 2, 'must reuse Z=3 instead of recomputing');
    await done;
    assert.equal(h.state.res.values[0], 3);
  });

  test(`${kind}: changing Z during a request queues the latest layer and rejects stale display`, async () => {
    const h = preview(kind);
    const done = h.run();
    h.S.z = 4; await h.run(); h.S.z = 5; await h.run();
    h.complete(3); await done; await tick();
    assert.notEqual(h.state.res?.values[0], 3, 'old Z must not become the displayed result');
    assert.equal(h.calls.length, 2, 'latest layer must start without another polling tick');
    assert.equal(h.calls[1][kind === 'wedge' ? 'index' : 'z'], 5);
    h.complete(5); await tick();
    assert.equal(h.state.res.values[0], 5);
  });

  test(`${kind}: an old image response cannot enter the next image cache`, async () => {
    const h = preview(kind), done = h.run();
    h.S.token = 2; h.state.model = null;
    h.complete(); await done;
    assert.equal(h.state.cache.size, 0);
    assert.equal(h.state.res, null);
    // denoise can compute immediately for a new image; drain that request.
    await tick();
    if (h.calls.length > 1) { h.complete(); await tick(); }
  });

  test(`${kind}: a new model invalidates cached and in-flight previews`, async () => {
    const h = preview(kind), done = h.run();
    h.context.clearPreviews(); h.complete(1); await done; await tick();
    assert.equal(h.state.cache.size, 0);
    assert.deepEqual(h.draw(), []);
    assert.equal(h.calls.length, 2);
    h.complete(2); await tick();
    assert.deepEqual(h.draw(), [2]);
  });

  if (kind === 'wedge') test('wedge: a live training snapshot supports changing Z before training ends', async () => {
    const h = preview(kind);
    h.state.training = true;
    await h.run();
    assert.equal(h.calls.length, 0, 'wait for the first usable snapshot');
    h.state.live = true;
    let done = h.run(); h.complete(3); await done;
    h.S.z = 4; done = h.run(); h.complete(4); await done;
    assert.deepEqual(h.draw(), [4]);
    assert.equal(h.state.training, true);
  });

  test(`${kind}: cache is bounded and does not show another channel or viewport`, async () => {
    const h = preview(kind);
    for (let z = 0; z < 10; z++) {
      h.S.z = z; const done = h.run(); h.complete(z); await done;
    }
    assert.equal(h.state.cache.size, 8);
    h.context.visibleRegion = () => [1, 1, 2, 2];
    assert.deepEqual(h.draw(), []);
    h.context.visibleRegion = () => [0, 0, 2, 2];
    if (kind === 'denoise') { h.context.fftChannel = () => 1; assert.deepEqual(h.draw(), []); }
  });
}
