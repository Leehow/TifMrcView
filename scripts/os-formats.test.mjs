import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

const root = new URL('../', import.meta.url);
const read = (name) => JSON.parse(fs.readFileSync(new URL(name, root), 'utf8'));
const config = read('src-tauri/tauri.conf.json');
const formats = read('packaging/macos/plugin-formats.json');

test('macOS future formats are unique and separate from installed/core/official types', () => {
  const existing = new Set(config.bundle.fileAssociations.flatMap((a) => a.ext));
  for (const entry of fs.readdirSync(new URL('plugins/', root), { withFileTypes: true })) {
    const file = new URL(`plugins/${entry.name}/plugin.json`, root);
    if (entry.isDirectory() && fs.existsSync(file)) {
      for (const f of read(`plugins/${entry.name}/plugin.json`).formats ?? []) for (const ext of f.ext) existing.add(ext);
    }
  }
  const core = fs.readFileSync(new URL('src-tauri/src/formats/mod.rs', root), 'utf8')
    .match(/CORE_EXTENSIONS[^=]*=\s*&\[([^\]]+)\]/)[1].match(/"([^\"]+)"/g).map((s) => JSON.parse(s));
  for (const ext of Object.keys(formats)) {
    assert.match(ext, /^[a-z0-9]{1,16}$/);
    assert.ok(!existing.has(ext), ext);
    assert.ok(!core.includes(ext), ext);
  }
});

test('app and both Quick Look plists share imported UTIs and Alternate document entries', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tmv-plists-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const generated = {};
  for (const kind of ['App', 'Thumbnail', 'Preview']) {
    const file = path.join(dir, `${kind}.plist`);
    execFileSync('python3', [new URL('scripts/quicklook-plist.py', root).pathname, kind, file]);
    generated[kind] = JSON.parse(execFileSync('python3', ['-c', 'import json,plistlib,sys; print(json.dumps(plistlib.load(open(sys.argv[1],"rb"))))', file], { encoding: 'utf8' }));
  }
  assert.equal(generated.App.CFBundleDocumentTypes.length, config.bundle.fileAssociations.length + Object.keys(formats).length);
  assert.equal(generated.App.UTImportedTypeDeclarations.length, Object.keys(formats).length);
  for (const [ext, name] of Object.entries(formats)) {
    const uti = `com.leehow.tifmrcview.${ext}`;
    const doc = generated.App.CFBundleDocumentTypes.find((d) => d.CFBundleTypeExtensions.includes(ext));
    assert.equal(doc.CFBundleTypeName, name);
    assert.equal(doc.CFBundleTypeRole, 'Viewer');
    assert.equal(doc.LSHandlerRank, 'Alternate');
    const declaration = generated.App.UTImportedTypeDeclarations.find((d) => d.UTTypeIdentifier === uti);
    assert.deepEqual(declaration.UTTypeConformsTo, ['public.data', 'public.image']);
    assert.deepEqual(declaration.UTTypeTagSpecification['public.filename-extension'], [ext]);
    for (const kind of ['Thumbnail', 'Preview']) assert.ok(generated[kind].NSExtension.NSExtensionAttributes.QLSupportedContentTypes.includes(uti));
  }
  for (const assoc of config.bundle.fileAssociations) {
    const doc = generated.App.CFBundleDocumentTypes.find((d) => d.CFBundleTypeName === assoc.name);
    assert.deepEqual(doc.CFBundleTypeExtensions, assoc.ext);
    assert.equal(doc.LSHandlerRank, assoc.rank);
  }
});

test('IMOD aliases use the existing MRC association on every installer', () => {
  const mrc = config.bundle.fileAssociations.find((a) => a.exportedType?.identifier.endsWith('.mrc-alternate'));
  const nsis = fs.readFileSync(new URL('packaging/windows/thumbnail-hooks.nsh', root), 'utf8');
  const xml = fs.readFileSync(new URL('packaging/linux/tifmrcview-mrc.xml', root), 'utf8');
  for (const ext of ['ali', 'preali']) {
    assert.ok(mrc.ext.includes(ext));
    assert.ok(nsis.includes(`!insertmacro \${ACTION} ${ext}`));
    assert.ok(xml.includes(`<glob pattern="*.${ext}"/>`));
  }
});
