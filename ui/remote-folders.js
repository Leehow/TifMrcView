"use strict";

// Local cache paths keep all native image readers and Pro tabs unchanged.
// Only an explicit image open resolves the full file. Previews transfer small PNGs.
function mountRemoteFolder(owner, { name, path, entries, resolve, thumbnail }) {
  ext.remoteFiles ||= new Map();
  const folder = { owner, name, remotePath: path, dir: `ssh:${owner}:${path}`,
    files: entries.map((e) => e.local), sizes: entries.map((e) => e.size),
    groups: entries.map((e) => fileGroup(e.name)), stacks: {}, remote: true, index: -1 };
  for (const entry of entries) {
    const old = ext.remoteFiles.get(entry.local);
    ext.remoteFiles.set(entry.local, { folder, ready: old?.ready || false,
      resolve: () => resolve(entry), thumbnail: thumbnail ? () => thumbnail(entry) : null });
  }
  showRemoteFolder(folder, null);
}

function showRemoteFolder(folder, path) {
  S.folder = { ...folder, index: folder.files.indexOf(path) };
  $("folder-name").textContent = `SSH · ${folder.name}`;
  $("folder-name").title = folder.remotePath;
  $("btn-reveal").hidden = true;
  buildStrip();
  markStrip();
}

function unmountRemoteFolder(owner) {
  if (!ext.remoteFiles) return;
  for (const [path, file] of ext.remoteFiles) {
    if (file.folder.owner === owner) ext.remoteFiles.delete(path);
  }
  if (S.folder.owner === owner) {
    S.folder.dir = null;
    if (S.info) refreshFolder(S.info.path).catch((e) => console.warn(e));
    else { S.folder = { dir: null, files: [], sizes: [], groups: [], stacks: {}, index: -1 }; buildStrip(); }
  }
}
