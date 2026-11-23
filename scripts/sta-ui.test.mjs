import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const read = (rel) => fs.readFileSync(new URL(`../${rel}`, import.meta.url), 'utf8');

const indexJs = read('ui/pro/sta/index.js');
const layersJs = read('ui/pro/sta/layers.js');
const drawJs = read('ui/pro/sta/draw.js');
const foundJs = read('ui/pro/sta/found.js');
const proCss = read('ui/pro/pro.css');

test('STA live preview and status note sit outside the collapsed auto-candidates details box', () => {
  const linePos = indexJs.indexOf('id="sta-line"');
  const livePos = indexJs.indexOf('id="sta-live"');
  const autoBoxPos = indexJs.indexOf('id="sta-auto-box"');
  assert.ok(linePos > 0 && livePos > 0 && autoBoxPos > 0);
  assert.ok(linePos < autoBoxPos, '#sta-line must be outside (before) #sta-auto-box');
  assert.ok(livePos < autoBoxPos, '#sta-live must be outside (before) #sta-auto-box');
});

test('STA exposes advanced options (symmetry, particle cap, 2D pose refinement) and STAR/TBL export', () => {
  assert.match(indexJs, /id="sta-opts-box"/);
  assert.match(indexJs, /id="sta-opt-sym"/);
  assert.match(indexJs, /id="sta-opt-max"/);
  assert.match(indexJs, /id="sta-opt-2d"/);
  assert.match(indexJs, /invoke\("sta_options_get"/);
  assert.match(indexJs, /invoke\("sta_options_set"/);
  assert.match(indexJs, /id="sta-export"/);
  assert.match(indexJs, /invoke\("sta_export_star"/);
});

test('STA draw panel has a single visible heading and physical nm size controls with custom input', () => {
  assert.doesNotMatch(drawJs, /<h4>自己标注<\/h4>/);
  assert.doesNotMatch(proCss, /#sta-draw\s*>\s*h4:first-child\s*\{\s*display:\s*none/);
  assert.match(drawJs, /小 13 nm/);
  assert.match(drawJs, /中 22 nm/);
  assert.match(drawJs, /大 36 nm/);
  assert.match(drawJs, /id = "sta-size-custom"/);
});

test('STA layers warns on incompatible mixed families and folds extra unlabelled tomograms', () => {
  assert.match(layersJs, /id="sta-layers-warn"/);
  assert.match(layersJs, /hasParticles && hasShapes/);
  assert.match(layersJs, /sta-layers-empty/);
});

test('STA found and placed overlays support XY, XZ and YZ orthogonal views', () => {
  assert.match(foundJs, /function onAxis\(p\)/);
  assert.match(foundJs, /function fromAxis\(u, v, d\)/);
  assert.match(foundJs, /S\.axis !== "xy" && S\.axis !== "xz" && S\.axis !== "yz"/);
  assert.doesNotMatch(foundJs, /S\.axis !== "xy"\)\s*return/);
});
