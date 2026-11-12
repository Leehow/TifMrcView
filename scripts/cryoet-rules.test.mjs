import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const context = vm.createContext({ window: {}, TextDecoder });
vm.runInContext(fs.readFileSync(new URL('../ui/pro/cryoet/rules.js', import.meta.url), 'utf8'), context);
const R = context.window.TmvCryoRules;
// Objects made inside the vm context are not deepEqual to local ones.
const J = (x) => JSON.parse(JSON.stringify(x));
const proJs = fs.readFileSync(new URL('../ui/pro/pro.js', import.meta.url), 'utf8');

const series = (o) => ({ name: 'TS_01', stage: 'movies', movies: 41, corrected: 0, views: 0, leftOut: 0, verdict: null, take: null, chosen: false, residual: null, quality: null, error: null, ...o });

test('stage chips say where a series is', () => {
  assert.deepEqual(J(R.stageChip(series({}))), { text: '待处理', tone: 'idle' });
  assert.equal(R.stageChip(series({ corrected: 12 })).text, '运动校正 12/41');
  assert.equal(R.stageChip(series({ stage: 'stack' })).text, '已成序列');
  assert.equal(R.stageChip(series({ stage: 'aligned' })).text, '已对齐');
  assert.equal(R.stageChip(series({ stage: 'quick' })).text, '可预览');
  assert.equal(R.stageChip(series({ stage: 'done' })).text, '已完成');
});

test('verdict lines: unusable beats attention beats fine; nothing before judging', () => {
  assert.equal(R.verdictLine(series({})), null);
  assert.equal(R.verdictLine(series({ verdict: { usable: true, reason: null, attention: false } })).text, '✓ 可用');
  assert.deepEqual(J(R.verdictLine(series({ verdict: { usable: true, reason: '倾角范围偏窄', attention: true } }))), { text: '⚠ 建议看一下：倾角范围偏窄', tone: 'warn' });
  assert.deepEqual(J(R.verdictLine(series({ verdict: { usable: false, reason: '残差太大', attention: true } }))), { text: '✕ 不建议用：残差太大', tone: 'bad' });
  assert.equal(R.verdictLine(series({ verdict: { usable: false, reason: null, attention: false } })).text, '✕ 不建议用');
});

test('series needing attention come first, each group by name', () => {
  const ok = { usable: true, reason: null, attention: false };
  const list = [
    series({ name: 'TS_10', verdict: ok }),
    series({ name: 'TS_2', verdict: ok }),
    series({ name: 'TS_5', error: 'boom' }),
    series({ name: 'TS_7', verdict: { usable: true, reason: 'x', attention: true } }),
    series({ name: 'TS_3', verdict: { usable: false, reason: 'y', attention: false } }),
  ];
  assert.deepEqual(J(R.sortSeries(list).map((s) => s.name)), ['TS_3', 'TS_5', 'TS_7', 'TS_2', 'TS_10']);
  assert.deepEqual(list.map((s) => s.name)[0], 'TS_10', 'input is not reordered');
});

test('left-out and residual texts', () => {
  assert.equal(R.leftOutText(series({ views: 41, leftOut: 3 })), '去掉 3/41 张');
  assert.equal(R.leftOutText(series({ views: 41, leftOut: 0 })), '');
  assert.equal(R.residualText(series({ residual: 0.6213, quality: '好' })), '残差 0.62 px · 好');
  assert.equal(R.residualText(series({ residual: 1 })), '残差 1.00 px');
  assert.equal(R.residualText(series({})), '');
});

test('what counts as a used view follows the user first, the program second', () => {
  const checks = [{ z: 0, bad: null }, { z: 1, bad: '太暗' }, { z: 2, bad: null }, { z: 3, bad: '漂移大' }];
  const picks = { 2: false, 3: true };
  assert.deepEqual(J(checks.map((c) => R.viewState(c, picks).state)), ['used', 'suggested', 'dropped', 'kept']);
  assert.deepEqual(J(R.usedViews(checks, picks).map((c) => c.z)), [0, 3]);
  assert.equal(R.summaryLine(checks, picks), '共 4 张，用 2 张');
  assert.equal(R.summaryLine(checks, {}), '共 4 张，用 2 张');
  assert.equal(R.summaryLine(checks, undefined), '共 4 张，用 2 张');
  assert.equal(R.viewState(checks[0], { 0: null }).pick, null, 'null is back to the program');
});

test('a click flips whether a view counts', () => {
  const bad = { z: 1, bad: '太暗' }, fine = { z: 0, bad: null };
  assert.equal(R.toggleKeep(fine, {}), false);
  assert.equal(R.toggleKeep(bad, {}), true);
  assert.equal(R.toggleKeep(bad, { 1: true }), false);
  assert.equal(R.toggleKeep(fine, { 0: false }), true);
});

test('aligned views leave out the excluded ones, by angle', () => {
  const al = { views: [{ angle: 10, excluded: false }, { angle: -10, excluded: false }, { angle: 0, excluded: true }] };
  assert.deepEqual(J(R.alignedViews(al).map((v) => v.angle)), [-10, 10]);
  assert.deepEqual(J(R.alignedViews(null)), []);
  assert.equal(R.residualScale([{ residual: 0.2 }]), 0.5);
  assert.equal(R.residualScale([{ residual: 2.5 }]), 2.5);
});

test('the review shows the final tomogram when there is one, else the quick one', () => {
  assert.equal(R.tomogramOf(series({ quick: 'q.mrc', result: 'r.mrc' })).path, 'r.mrc');
  assert.equal(R.tomogramOf(series({ quick: 'q.mrc' })).path, 'q.mrc');
  assert.equal(R.tomogramOf(series({})), null);
  assert.deepEqual(J(R.chosenNames([series({ name: 'a', chosen: true }), series({ name: 'b' })])), ['a']);
});

test('pictures decode as w, h, padded title, RGBA', () => {
  const title = new TextEncoder().encode('abc');
  const buf = new ArrayBuffer(12 + 4 + 2 * 1 * 4);
  const dv = new DataView(buf);
  dv.setUint32(0, 2, true); dv.setUint32(4, 1, true); dv.setUint32(8, title.length, true);
  new Uint8Array(buf, 12, 3).set(title);
  new Uint8Array(buf, 16).set([1, 2, 3, 255, 4, 5, 6, 255]);
  const p = R.decodePicture(buf);
  assert.equal(p.w, 2); assert.equal(p.h, 1); assert.equal(p.title, 'abc');
  assert.deepEqual(J([...p.rgba]), [1, 2, 3, 255, 4, 5, 6, 255]);
  assert.equal(R.decodePicture(buf.slice(0, 20)), null, 'truncated');
  assert.equal(R.decodePicture(null), null);
  assert.equal(R.depthFromTitle('z 12 / 64'), 64);
  assert.equal(R.depthFromTitle(''), null);
});

test('a series that got further has another signature', () => {
  const a = series({}), b = series({ stage: 'aligned', aligned: 'x' });
  assert.notEqual(R.signature(a), R.signature(b));
  assert.equal(R.signature(a), R.signature({ ...a }));
});

test('Pro loads the rules and the docked pane before the workspace, the folder flow after it', () => {
  const at = (s) => proJs.indexOf(`"pro/cryoet/${s}.js"`);
  assert.ok(at('rules') > 0 && at('rules') < at('live') && at('live') < at('index') && at('index') < at('session') && at('session') < at('motion'));
});
