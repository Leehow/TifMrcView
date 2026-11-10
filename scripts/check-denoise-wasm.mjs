import assert from 'node:assert/strict';
import fs from 'node:fs';
import { performance } from 'node:perf_hooks';

const file = process.argv[2];
if (!file) throw new Error('Usage: node scripts/check-denoise-wasm.mjs module.wasm');
const { instance } = await WebAssembly.instantiate(fs.readFileSync(file));
const api = instance.exports;
assert.equal(api.tmv_nad_version(), 1);
assert.equal(api.tmv_nad_prepare(0, 8, 8), 0);
assert.equal(api.tmv_nad_prepare(129, 8, 8), 0);
assert.equal(api.tmv_nad_prepare(8, 8, 33), 0);
assert.equal(api.tmv_nad_run(1, 2), 2);

function run(input, dims, strength, iterations) {
  const pointer = api.tmv_nad_prepare(...dims);
  assert.ok(pointer);
  new Float32Array(api.memory.buffer, pointer, input.length).set(input);
  assert.equal(api.tmv_nad_length(), input.length);
  const start = performance.now();
  assert.equal(api.tmv_nad_run(strength, iterations), 0);
  const ms = performance.now() - start;
  return { values: new Float32Array(api.memory.buffer, api.tmv_nad_pointer(), input.length).slice(), ms };
}

const fixtures = new URL('../src-tauri/tests/fixtures/nad3d/', import.meta.url);
const manifest = JSON.parse(fs.readFileSync(new URL('manifest.json', fixtures)));
const floats = (name) => {
  const b = fs.readFileSync(new URL(name, fixtures));
  return new Float32Array(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength));
};
const comparisons = [];
for (const c of manifest.cases) for (const ref of c.references) {
  const result = run(floats(c.input), c.shape, ref.strength, ref.iterations);
  const expected = floats(ref.output);
  let maxDelta = 0;
  for (let i = 0; i < expected.length; i++) maxDelta = Math.max(maxDelta, Math.abs(result.values[i] - expected[i]));
  assert.equal(maxDelta, 0, `${ref.output}: WASM must retain the reference numerical behavior`);
  comparisons.push({ case: ref.output, voxels: expected.length, maxDelta });
}

// Invalid settings and nonfinite data cannot replace a valid prepared image.
const pointer = api.tmv_nad_prepare(4, 4, 4);
new Float32Array(api.memory.buffer, pointer, 64).fill(7);
for (const [k, n] of [[NaN, 2], [0, 2], [1, 0], [1, 21]]) assert.equal(api.tmv_nad_run(k, n), 1);
assert.equal(new Float32Array(api.memory.buffer, api.tmv_nad_pointer(), 64)[0], 7);
new Float32Array(api.memory.buffer, pointer, 64)[0] = NaN;
assert.equal(api.tmv_nad_run(1, 2), 2);

const timings = [];
for (const dims of [[64, 64, 16], [96, 96, 16]]) {
  const input = Float32Array.from({ length: dims.reduce((a, b) => a * b, 1) }, (_, i) => Math.fround(Math.sin(i * 1.17) + (i % dims[0] > dims[0] / 2 ? 2 : 0)));
  for (const iterations of [2, 5]) {
    const samples = [];
    for (let repeat = 0; repeat < 3; repeat++) samples.push(run(input, dims, 1.22, iterations).ms);
    timings.push({ dims, iterations, samplesMs: samples, medianMs: samples.sort((a, b) => a - b)[1] });
  }
}
console.log(JSON.stringify({ reference: manifest.version, comparisons, timings, wasmBytes: fs.statSync(file).size }, null, 2));
