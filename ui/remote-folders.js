"use strict";

// Local cache paths keep all native image readers and Pro tabs unchanged.
// Only an explicit image open resolves the full file. Previews transfer small PNGs.
function mountRemoteFolder(owner, { name, path, address = "", entries, resolve, thumbnail, navigate, inspect }) {
  ext.remoteFiles ||= new Map();
  const images = entries.filter(e => !e.directory && (e.supported || (!navigate && e.supported !== false)));
  const folder = { owner, name, remotePath: path, address, dir: `ssh:${owner}:${address}:${path}`,
    files: images.map((e) => e.local), sizes: images.map((e) => e.size),
    groups: images.map((e) => fileGroup(e.name)), stacks: {}, remote: true, index: -1,
    entries: navigate ? entries : null, navigate, inspect, view: { query: "", limit: 100 } };
  for (const entry of images) {
    const old = ext.remoteFiles.get(entry.local);
    ext.remoteFiles.set(entry.local, { folder, ready: old?.ready || false,
      resolve: () => resolve(entry), thumbnail: thumbnail ? () => thumbnail(entry) : null });
  }
  showRemoteFolder(folder, null);
}

/** A real directory browser: directories and metadata do not become images. */
function buildRemoteDirectory(strip) {
  const folder = S.folder;
  const nav = document.createElement("div"); nav.className = "remote-navigation";
  const up = document.createElement("button"); up.textContent = "↑ 上级目录";
  up.disabled = folder.remotePath === "/";
  const run = async (button, path) => { button.disabled = true; count.textContent = '正在读取目录…'; try { await folder.navigate(path); } catch (e) { toast(String(e)); } finally { button.disabled = false; if (button.isConnected) render(); } };
  up.onclick = () => run(up, folder.remotePath.replace(/\/[^/]+\/?$/, "") || "/");
  const refresh = document.createElement("button"); refresh.textContent = "↻"; refresh.setAttribute("aria-label", "刷新远程目录");
  refresh.onclick = () => run(refresh, folder.remotePath);
  nav.append(up, refresh); strip.append(nav);
  const search = document.createElement("input"); search.type = "search"; search.className = "remote-search";
  search.placeholder = "筛选文件夹和文件…"; search.setAttribute("aria-label", "筛选远程侧栏"); search.value = folder.view.query;
  strip.append(search);
  const count = document.createElement("div"); count.className = "remote-directory-count"; strip.append(count);
  const rows = document.createElement("div"); rows.className = "remote-directory-rows"; strip.append(rows);
  const more = document.createElement("button"); more.className = "remote-more"; strip.append(more);
  function render() {
    rows.querySelectorAll('.thumb').forEach(el => thumbObserver.unobserve(el));
    rows.replaceChildren();
    const all = folder.entries.filter(e => !e.name.startsWith('.') && e.name.toLocaleLowerCase().includes(folder.view.query.toLocaleLowerCase()));
    count.textContent = `${all.filter(e => e.directory).length} 个文件夹 · ${all.filter(e => !e.directory).length} 个文件`;
    for (const entry of all.slice(0, folder.view.limit)) {
      if (entry.supported && !entry.directory) {
        const item = document.createElement('div'); item.className = 'thumb remote-entry'; item.dataset.path = entry.local;
        item.innerHTML = '<div class="box"></div><div class="nm"></div><div class="meta"></div>';
        item.querySelector('.box').textContent = '正在生成远程预览…';
        item.querySelector('.nm').textContent = entry.name;
        item.querySelector('.meta').textContent = `${entry.name.split('.').pop().toUpperCase()} · ${fmtBytes(entry.size || 0)}`;
        item.title = entry.path;
        item.onclick = e => e.metaKey || e.ctrlKey ? openPath(entry.local) : openHere(entry.local);
        rows.append(item); thumbObserver.observe(item);
      } else {
        const item = document.createElement('button'); item.className = `remote-entry remote-file${entry.directory ? ' remote-folder' : ''}`;
        item.setAttribute('aria-label', `${entry.name}${entry.directory ? '，文件夹' : ''}`);
        const symbol = document.createElement('span'); symbol.className = 'remote-file-icon'; symbol.textContent = entry.directory ? '📁' : /\.mdoc$/i.test(entry.name) ? '▤' : '▧';
        const label = document.createElement('span'); label.textContent = entry.name;
        const detail = document.createElement('small'); detail.textContent = entry.directory ? '文件夹 ›' : fmtBytes(entry.size || 0);
        item.append(symbol, label, detail); item.title = entry.path;
        item.onclick = () => entry.directory ? run(item, entry.path) : folder.inspect(entry);
        rows.append(item);
      }
    }
    more.hidden = all.length <= folder.view.limit;
    more.textContent = `显示更多（已显示 ${Math.min(all.length, folder.view.limit)} / ${all.length}）`;
    if (!all.length) { const empty = document.createElement('p'); empty.className = 'dim'; empty.textContent = folder.view.query ? '没有匹配的文件或文件夹' : '此目录为空'; rows.append(empty); }
  }
  search.oninput = () => { folder.view.query = search.value; folder.view.limit = 100; render(); };
  more.onclick = () => { folder.view.limit += 100; render(); };
  render();
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
