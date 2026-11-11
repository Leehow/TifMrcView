import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

// Exercise the production pointer handlers with CSS coordinates, as delivered
// by WebKit on both standard and Retina displays. A tool listener runs after
// capture so an accidental pan/annotation start is observable.
function divider() {
  const source = fs.readFileSync(new URL('../ui/pro/denoise.js', import.meta.url), 'utf8');
  const listeners = new Map(), captured = new Set(), saved = [], tools = [];
  const view = { style: {}, getBoundingClientRect: () => ({ left: 40, top: 75 }),
    setPointerCapture: (id) => captured.add(id), hasPointerCapture: (id) => captured.has(id),
    releasePointerCapture: (id) => captured.delete(id) };
  const controls = { view, 'dn-preview': { checked: true }, 'dn-split': { value: 50 } };
  const S = { token: 1, z: 3, t: 0, axis: 'xy', xyz: false, op: null, textEdit: null };
  const N = { cache: new Map(), generation: 0 };
  const context = { S, N, panel: { hidden: false }, $: (id) => controls[id],
    previewReq: () => ({ token: S.token, z: S.z, t: S.t }),
    previewKey: (req) => JSON.stringify([N.generation, req]),
    toScreen: (x, y) => [x, y], draw: () => { context.draws++; }, draws: 0,
    store: () => saved.push(+controls['dn-split'].value),
    invoke: () => { throw new Error('divider must not request backend work'); },
    window: { devicePixelRatio: 2, addEventListener: (type, fn, capture) => {
      listeners.set(type, { fn, capture });
    } },
  };
  vm.createContext(context);
  vm.runInContext(source.slice(source.indexOf('  function comparisonGeometry('), source.indexOf('  function overlay(')), context);
  vm.runInContext(source.slice(source.indexOf('  // ---- Drag the comparison divider ----'), source.indexOf('  // ---- Save ----')), context);
  function cache() {
    N.cache.set(context.previewKey(context.previewReq()), { token: S.token, z: S.z, t: S.t, rect: [100, 50, 800, 600] });
  }
  cache();
  function send(type, x = 500, y = 300, extra = {}) {
    const e = { type, target: view, pointerId: 1, pointerType: 'mouse', isPrimary: true,
      button: 0, buttons: type === 'pointermove' && N.splitDrag ? 1 : 0,
      clientX: x + 40, clientY: y + 75, ...extra,
      preventDefault() { this.prevented = true; },
      stopImmediatePropagation() { this.stopped = true; },
    };
    listeners.get(type).fn(e);
    if (type === 'pointerdown' && !e.stopped) tools.push(e);
    return e;
  }
  return { context, S, N, controls, view, saved, captured, tools, listeners, cache, send };
}

test('divider drag uses the painted CSS bounds, keeps the grab offset and synchronizes the slider', () => {
  const h = divider();
  assert.equal(h.listeners.get('pointerdown').capture, true);
  assert.equal(h.send('pointermove').stopped, true);
  assert.equal(h.view.style.cursor, 'ew-resize');
  assert.equal(h.send('pointerdown', 503).stopped, true);
  assert.equal(h.captured.has(1), true);
  h.send('pointermove', 503);
  assert.equal(+h.controls['dn-split'].value, 50, 'grabbing near the line must not jump');
  h.send('pointermove', 703);
  assert.equal(+h.controls['dn-split'].value, 75);
  assert.equal(h.context.comparisonGeometry().split, 700, 'render and hit test must share the updated split');
  assert.equal(h.saved.length, 0, 'persist once when the gesture finishes');
  assert.equal(h.send('pointerup', 703).stopped, true);
  assert.deepEqual(h.saved, [75]);
  assert.equal(h.captured.size, 0);
  assert.equal(h.N.splitDrag, null);
  assert.equal(h.tools.length, 0, 'no pan or annotation gesture may start');
  assert.equal(h.S.op, null);
  assert.equal(h.context.draws, 3);
});

test('drag continues outside the image and clamps to both edges', () => {
  const h = divider();
  h.send('pointerdown');
  h.send('pointermove', -300, -100, { target: {} });
  assert.equal(+h.controls['dn-split'].value, 0);
  h.send('pointermove', 1500, 1000, { target: {} });
  assert.equal(+h.controls['dn-split'].value, 100);
  h.send('pointerup', 1500, 1000, { target: {} });
  assert.deepEqual(h.saved, [100]);
  assert.equal(h.captured.size, 0);
});

test('unrelated canvas interactions, other views and stale previews keep their handlers', () => {
  const cases = [
    (h) => [200, 300], (h) => [500, 10],
    (h) => [500, 300, { button: 1 }], (h) => [500, 300, { button: 2 }],
    ...['altKey', 'ctrlKey', 'metaKey', 'shiftKey'].map((key) => (h) => [500, 300, { [key]: true }]),
    (h) => { h.S.op = { kind: 'pan' }; return []; },
    (h) => { h.S.textEdit = {}; return []; },
    (h) => { h.context.panel.hidden = true; return []; },
    (h) => { h.controls['dn-preview'].checked = false; return []; },
    (h) => { h.S.axis = 'xz'; return []; }, (h) => { h.S.xyz = true; return []; },
    (h) => { h.S.z++; return []; }, (h) => { h.N.cache.clear(); return []; },
  ];
  for (const setup of cases) {
    const h = divider();
    assert.equal(h.send('pointerdown', ...setup(h)).stopped, undefined);
    assert.equal(h.tools.length, 1);
    assert.equal(h.N.splitDrag, undefined);
  }
  const h = divider();
  h.send('pointermove');
  h.send('pointermove', 200);
  assert.equal(h.view.style.cursor, '');
});

test('cancel, lost capture, blur and Escape release the gesture without a stuck cursor', () => {
  for (const type of ['pointercancel', 'lostpointercapture', 'blur', 'keydown']) {
    const h = divider();
    h.send('pointerdown'); h.send('pointermove', 700);
    h.send(type, 500, 300, { key: 'Escape' });
    assert.equal(h.N.splitDrag, null, type);
    assert.equal(h.captured.size, 0, type);
    assert.equal(h.view.style.cursor, '', type);
    assert.deepEqual(h.saved, [75], type);
    h.send('pointermove', 800);
    assert.equal(+h.controls['dn-split'].value, 75, type);
  }
});

test('source, Z, model or viewport changes cancel rather than moving a different preview', () => {
  for (const change of [
    (h) => { h.S.token++; h.cache(); }, (h) => { h.S.z++; h.cache(); },
    (h) => { h.N.generation++; h.cache(); },
    (h) => { h.context.toScreen = (x, y) => [x + 5, y]; },
  ]) {
    const h = divider(); h.send('pointerdown'); change(h);
    h.send('pointermove', 700);
    assert.equal(h.N.splitDrag, null);
    assert.equal(+h.controls['dn-split'].value, 50);
    assert.equal(h.captured.size, 0);
  }
});

test('a second pointer cannot move or finish the primary touch drag', () => {
  const h = divider();
  assert.equal(h.send('pointerdown', 512, 300, { pointerType: 'touch' }).stopped, true);
  h.send('pointermove', 700, 300, { pointerId: 2 });
  h.send('pointerup', 700, 300, { pointerId: 2 });
  assert.equal(+h.controls['dn-split'].value, 50);
  assert.equal(h.captured.has(1), true);
  h.send('pointerup', 712);
  assert.deepEqual(h.saved, [75]);
});
