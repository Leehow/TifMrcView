import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const source = fs.readFileSync(new URL('../ui/pro/model-store.js', import.meta.url), 'utf8');

class Element {
  constructor(tag = 'div') { this.tagName = tag.toUpperCase(); this.children = []; this.options = []; this.style = {}; this.handlers = {}; this.textContent = ''; }
  replaceChildren() { this.children = []; this.textContent = ''; }
  append(...nodes) { this.children.push(...nodes); if (this.tagName === 'SELECT') this.options.push(...nodes); }
  addEventListener(name, fn) { this.handlers[name] = fn; }
}

function harness() {
  let current = { token: 1, c: 0, t: 0 };
  const calls = [], pending = [], nodes = new Map(), events = [];
  const $ = (id) => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); };
  const context = {
    window: { TifMrcViewPro: {} }, $, document: { createElement: (tag) => new Element(tag) },
    T: { event: { listen: (_, fn) => { events.push(fn); return Promise.resolve(() => {}); } } },
    invoke: (name, req) => { calls.push({ name, req }); return new Promise((resolve, reject) => pending.push({ resolve, reject })); },
  };
  vm.createContext(context); vm.runInContext(source, context);
  const models = context.window.TifMrcViewPro.models;
  const store = models.attach('denoise', 'record', () => current);
  return { store, models, calls, pending, $,
    setContext: (value) => { current = value; }, event: (payload) => events.forEach((fn) => fn({ payload })) };
}

const saved = (path = '/image/model.json') => ({
  recordPath: path, weightsPath: '/image/weights.safetensors',
  record: { kind: 'denoise', channel: 0, time: 0, complete: true,
    parameters: { effective: { config: { seed: 17, lr: 0.001, batch: 8 }, mode: 'n2v' }, model: { mean: 0.7, std: 0.2 } } },
});

test('automatic restore displays full parameters and never starts training', async () => {
  const h = harness(), done = h.store.restore({ token: 1, c: 0, t: 0 });
  h.pending.shift().resolve(saved()); await done;
  assert.deepEqual(h.calls.map((c) => c.name), ['trained_model_restore']);
  const root = h.$('record');
  assert.equal(root.hidden, false);
  assert.match(root.children[0].textContent, /已自动恢复模型/);
  const details = root.children[1];
  const record = JSON.parse(details.children[3].textContent);
  assert.equal(record.parameters.effective.config.seed, 17);
  assert.equal(record.parameters.effective.config.lr, 0.001);
  assert.equal(record.parameters.model.std, 0.2);
  assert.equal(record.weightsFile, '/image/weights.safetensors');
});

test('a late old-image restore cannot replace the newer image record', async () => {
  const h = harness();
  const first = h.store.restore({ token: 1, c: 0, t: 0 });
  h.setContext({ token: 2, c: 0, t: 0 });
  const second = h.store.restore({ token: 2, c: 0, t: 0 });
  h.pending[1].resolve(saved('/new/model.json')); await second;
  h.pending[0].resolve(saved('/old/model.json'));
  assert.equal((await first).stale, true);
  assert.equal(h.$('record').children[1].children[1].textContent, '/new/model.json');
});

test('restore failure is visible and does not silently retrain', async () => {
  const h = harness(), done = h.store.restore({ token: 1, c: 0, t: 0 });
  h.pending.shift().reject(new Error('checksum mismatch'));
  assert.match((await done).error, /checksum mismatch/);
  assert.match(h.$('record').textContent, /读取模型失败/);
  assert.match(h.$('record').textContent, /未重新训练/);
  assert.deepEqual(h.calls.map((c) => c.name), ['trained_model_restore']);
});

test('save failures warn that the usable model is still only in memory', () => {
  const h = harness(); h.store.show({ error: 'disk full' });
  assert.match(h.$('record').textContent, /disk full/);
  assert.match(h.$('record').textContent, /仍在内存/);
});

test('training progress from another channel or time cannot change this record', () => {
  const h = harness();
  h.store.show({ saved: saved('/current/model.json') });
  h.event({ kind: 'denoise', token: 1, channel: 1, time: 0, persistence: { error: 'foreign channel' } });
  h.event({ kind: 'denoise', token: 1, channel: 0, time: 1, persistence: { error: 'foreign time' } });
  assert.equal(h.$('record').children[1].children[1].textContent, '/current/model.json');
});

test('saved non-default numeric settings remain selectable and readable', () => {
  const h = harness(), select = h.$('steps');
  select.tagName = 'SELECT';
  h.models.setValue('steps', 37);
  assert.equal(select.value, '37');
  assert.equal(select.options[0].value, '37');
  h.models.setValue('steps', 37);
  assert.equal(select.options.length, 1);
});
