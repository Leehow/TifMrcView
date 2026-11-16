"use strict";

TifMrcView.register({
  id: "ssh", name: "SSH 远程文件夹", version: "1.3.0",
  description: "通过密码或本地密钥连接 SSH / SFTP，浏览远程目录并按需加载图像",
  activate(app) {
    if (!app.hasNative()) return;
    const icon = (type) => {
      const paths = {
        server: '<rect x="3" y="3" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="7" rx="2"/><path d="M7 6.5h.01M7 17.5h.01M11 6.5h6M11 17.5h6"/>',
        folder: '<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
        image: '<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8" cy="8" r="1.5"/><path d="m3 17 5-5 4 4 4-6 5 7"/>',
        file: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6"/>',
      };
      return `<svg viewBox="0 0 24 24" aria-hidden="true">${paths[type]}</svg>`;
    };
    const dialog = document.createElement("dialog");
    dialog.className = "ssh-dialog";
    dialog.setAttribute("aria-labelledby", "ssh-title");
    dialog.innerHTML = `
      <header class="ssh-heading"><span class="ssh-symbol">${icon("server")}</span>
        <div><h2 id="ssh-title">SSH 远程文件夹</h2><p>连接服务器，像本地一样浏览图像</p></div>
        <span class="ssh-badge" data-k="badge">未连接</span><button data-k="close" class="ssh-close" aria-label="关闭远程文件夹窗口">×</button>
      </header>
      <div class="ssh-layout">
        <aside class="ssh-connection"><div class="ssh-eyebrow">01 · 连接服务器</div>
          <form data-k="connect-form" class="ssh-form">
            <label>服务器地址<input data-k="host" name="ssh-host" required placeholder="192.168.1.100" autocomplete="off" spellcheck="false" /></label>
            <div class="ssh-fields"><label>用户名<input data-k="username" name="ssh-user" required placeholder="用户名" autocomplete="off" spellcheck="false" /></label>
              <label class="ssh-port">端口<input data-k="port" type="number" required min="1" max="65535" value="22" /></label></div>
            <label>登录方式<select data-k="auth-method"><option value="password">密码登录</option><option value="keyFile">本地密钥登录</option></select></label>
            <label data-k="password-group">密码<span class="ssh-password"><input data-k="password" name="ssh-password" type="password" required placeholder="仅用于本次连接" autocomplete="off" /><button type="button" data-k="show-password" aria-label="显示密码">显示</button></span></label>
            <div data-k="key-group" class="ssh-key-group" hidden><label>私钥文件<span class="ssh-key-picker"><input data-k="key-path" placeholder="选择私钥文件，或输入路径" autocomplete="off" spellcheck="false" /><button type="button" data-k="pick-key">选择…</button></span></label>
              <label>密钥口令 <span class="ssh-muted">可选 · 未加密或已在本机解锁时留空</span><input data-k="passphrase" type="password" placeholder="仅用于解锁本次私钥" autocomplete="off" /></label></div>
            <button class="primary ssh-connect" data-k="connect" type="submit">连接服务器 →</button>
          </form>
          <p data-k="connection-status" class="ssh-connect-status" role="status" hidden>首次访问局域网时，请在系统提示中允许 macOS 的本地网络访问。</p>
          <div data-k="connected" class="ssh-connected" hidden><span class="ssh-connected-dot"></span><strong data-k="identity"></strong><p data-k="address"></p><button data-k="disconnect">断开连接</button></div>
          <div class="ssh-private">密码与密钥口令不保存，私钥仅在本机读取。<br>原始图像只读访问，缩略图按需生成。</div>
          <div data-k="recent-box" class="ssh-recent" hidden><div class="ssh-eyebrow">最近连接</div><div data-k="recent"></div></div>
        </aside>
        <section class="ssh-browser" aria-label="远程文件浏览器">
          <div class="ssh-browser-heading"><div class="ssh-eyebrow">02 · 选择远程文件夹</div><span data-k="count" class="ssh-muted"></span></div>
          <div data-k="navigation" hidden>
            <div class="ssh-navigation"><button data-k="up" title="上级目录" aria-label="上级目录">↑</button><button data-k="home" title="主目录" aria-label="主目录">⌂</button>
              <form data-k="path-form"><input data-k="path" aria-label="远程目录路径" spellcheck="false" /><button type="submit">前往</button></form><button data-k="refresh" title="刷新目录" aria-label="刷新目录">↻</button></div>
            <nav data-k="crumbs" class="ssh-crumbs" aria-label="目录层级"></nav>
            <div class="ssh-filter"><input data-k="search" type="search" placeholder="筛选当前目录…" aria-label="筛选当前目录" /><label><input data-k="hidden" type="checkbox" /> 显示隐藏项</label></div>
          </div>
          <div data-k="error" class="ssh-error" role="alert" hidden></div>
          <div data-k="trust" class="ssh-trust" hidden><strong>首次连接 · 确认服务器身份</strong><p>请与服务器管理员提供的指纹核对，确认后再登录。</p><code data-k="fingerprint"></code><div><button data-k="trust-no">取消</button><button data-k="trust-yes" class="primary">信任并连接</button></div></div>
          <div data-k="empty" class="ssh-empty"><span>${icon("folder")}</span><h3>远程文件夹，从这里打开</h3><p>填写左侧的连接信息<br>连接后即可浏览目录并加载图像</p><div class="ssh-steps"><span>连接</span><i>→</i><span>选择文件夹</span><i>→</i><span>加载图像</span></div></div>
          <div data-k="list-wrap" class="ssh-list-wrap" hidden><div class="ssh-list-heading"><span>名称</span><span>大小</span><span>修改时间</span></div><div data-k="list" class="ssh-list" role="list" aria-label="目录内容"></div></div>
        </section>
      </div>
      <footer class="ssh-footer"><div><strong data-k="selection">尚未选择文件夹</strong><span data-k="hint">连接后选择目录，图像会在打开时按需缓存</span></div><button data-k="load" class="primary" disabled>加载此文件夹 →</button></footer>`;
    document.body.append(dialog);
    const transfer = document.createElement("div");
    transfer.className = "ssh-transfer";
    transfer.hidden = true;
    transfer.setAttribute("role", "status");
    transfer.innerHTML = `<span class="ssh-symbol">${icon("server")}</span><div><strong data-k="file"></strong><span data-k="progress">正在读取远程图像…</span><progress max="1" value="0"></progress></div><button aria-label="取消传输">取消</button>`;
    document.body.append(transfer);
    const el = (key) => dialog.querySelector(`[data-k="${key}"]`);
    const native = (method, args = {}) => app.native(method, { ...args, connection });
    const bytes = (n) => n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : n < 1073741824 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1073741824).toFixed(2)} GB`;
    let connection = null, home = "", current = "", entries = [], selected = null;
    let busy = false, disposed = false, browsing = 0, trustAnswer = null, timer = null;
    let transferring = false, activeTransfer = null;
    const previews = new Map(), previewQueue = [];
    let previewActive = false;
    const previewObserver = new IntersectionObserver((items) => {
      for (const item of items) if (item.isIntersecting) {
        previewObserver.unobserve(item.target);
        const { row, entry } = item.target._preview;
        preview(entry).then((url) => {
          if (!row.isConnected || disposed) return;
          const image = document.createElement("img"); image.src = url; image.alt = "";
          row.querySelector("svg")?.replaceWith(image);
        }).catch(() => { if (row.isConnected) row.title = `${entry.path}\n缩略图暂不可用，仍可打开原图`; });
      }
    }, { root: el("list"), rootMargin: "80px" });
    function preview(entry) {
      const key = `${connection}:${entry.path}:${entry.size}:${entry.modified}`;
      if (!previews.has(key)) {
        const pending = connection;
        if (previews.size >= 128) previews.delete(previews.keys().next().value);
        previews.set(key, new Promise((resolve, reject) => { previewQueue.push({ key, pending, entry, resolve, reject }); }));
      }
      pumpPreviews();
      return previews.get(key);
    }
    async function pumpPreviews() {
      if (previewActive || transferring || busy || disposed || !previewQueue.length) return;
      const job = previewQueue.shift();
      if (!job.pending || job.pending !== connection) { job.reject(new Error("连接已变化")); pumpPreviews(); return; }
      previewActive = true;
      try { job.resolve((await app.native("thumbnail", { connection: job.pending, path: job.entry.path, size: job.entry.size, modified: job.entry.modified })).url); }
      catch (e) { job.reject(e); }
      finally { previewActive = false; pumpPreviews(); }
    }
    const recent = app.settings.get("recent", []);
    if (recent[0]) fill(recent[0]);

    function fill(profile) {
      for (const key of ["host", "username", "port"]) el(key).value = profile[key] ?? (key === "port" ? 22 : "");
      el("auth-method").value = profile.authMethod || "password";
      el("key-path").value = profile.keyPath || "";
      el("password").value = ""; el("passphrase").value = "";
      syncAuth();
    }
    function syncAuth() {
      const key = el("auth-method").value === "keyFile";
      el("key-group").hidden = !key; el("password-group").hidden = key;
      el("key-path").required = key; el("password").required = !key;
      el("password").value = ""; el("passphrase").value = "";
    }
    function renderRecent() {
      el("recent").replaceChildren();
      for (const item of recent) {
        const button = document.createElement("button");
        button.textContent = `${item.username}@${item.host}`;
        button.title = `端口 ${item.port} · 点击填入连接信息`;
        button.onclick = () => { if (!busy && !home) fill(item); };
        el("recent").append(button);
      }
      el("recent-box").hidden = !recent.length || !!home;
    }
    function error(message = "") { el("error").textContent = String(message); el("error").hidden = !message; }
    function setBusy(on, label = "") {
      busy = on;
      el("connection-status").hidden = !on || !!home || !navigator.userAgent.includes("Mac");
      el("connect").textContent = on && !home ? label || "正在连接…" : "连接服务器 →";
      for (const key of ["host", "username", "password", "port", "auth-method", "key-path", "passphrase", "pick-key", "connect", "up", "home", "refresh", "path", "disconnect"]) el(key).disabled = on;
      el("connect-form").querySelector("button[type=submit]").disabled = on;
      updateSelection();
      if (!on) pumpPreviews();
    }
    function updateSelection() {
      const target = selected?.directory ? selected.path : current;
      const images = entries.filter((e) => e.supported);
      el("selection").textContent = target || "尚未选择文件夹";
      el("selection").title = target;
      el("hint").textContent = !home ? "连接后选择目录，图像会在打开时按需缓存" : selected && !selected.directory ? `${selected.supported ? "所选图像" : "所选文件"} · ${bytes(selected.size)}` : "在右侧栏浏览全部文件夹和文件，点击图像时按需读取";
      el("load").textContent = selected && !selected.directory ? selected.supported ? "打开所选图像 →" : "查看所选文件 →" : "加载此文件夹 →";
      el("load").disabled = busy || !home;
    }
    function renderList() {
      previewObserver.disconnect();
      const query = el("search").value.toLocaleLowerCase();
      const visible = entries.filter((e) => (el("hidden").checked || !e.name.startsWith(".")) && e.name.toLocaleLowerCase().includes(query));
      el("list").replaceChildren();
      el("count").textContent = `${entries.filter((e) => e.directory).length} 个文件夹 · ${entries.filter((e) => e.supported).length} 个图像`;
      for (const entry of visible) {
        const row = document.createElement("button");
        row.className = `ssh-entry${entry.directory ? " directory" : entry.supported ? " image" : " unsupported"}`;
        row.classList.toggle("selected", selected?.path === entry.path);
        row.setAttribute("role", "listitem");
        row.setAttribute("aria-label", `${entry.name}${entry.directory ? "，文件夹" : ""}`);
        row.innerHTML = `<span class="ssh-entry-name">${icon(entry.directory ? "folder" : entry.supported ? "image" : "file")}<span></span></span><span class="ssh-muted"></span><span class="ssh-muted"></span>`;
        row.children[0].lastElementChild.textContent = entry.name;
        row.children[1].textContent = entry.directory ? "文件夹" : bytes(entry.size);
        row.children[2].textContent = entry.modified ? new Date(entry.modified * 1000).toLocaleDateString() : "—";
        row.title = entry.path;
        row.onclick = () => {
          if (busy) return;
          selected = entry;
          for (const other of el("list").children) other.classList.toggle("selected", other === row);
          updateSelection();
        };
        row.ondblclick = () => { if (!busy) entry.directory ? browse(entry.path) : load(entry); };
        row.onkeydown = (event) => {
          if (event.key === "Enter") { event.preventDefault(); if (!busy) entry.directory ? browse(entry.path) : load(entry); }
        };
        el("list").append(row);
        if (entry.supported) { row._preview = { row, entry }; previewObserver.observe(row); }
      }
      if (!visible.length) {
        const empty = document.createElement("p"); empty.className = "ssh-no-files";
        empty.textContent = query ? "没有匹配的文件或文件夹" : "此目录为空，或仅包含隐藏项";
        el("list").append(empty);
      }
      updateSelection();
    }
    function breadcrumbs() {
      el("crumbs").replaceChildren();
      let path = "";
      for (const part of ["/", ...current.split("/").filter(Boolean)]) {
        path = part === "/" ? "/" : `${path.replace(/\/$/, "")}/${part}`;
        const target = path, button = document.createElement("button");
        button.textContent = part; button.title = target;
        button.onclick = () => !busy && browse(target);
        el("crumbs").append(button);
      }
    }
    async function browse(path) {
      for (const job of previewQueue.splice(0)) { previews.delete(job.key); job.reject(new Error('目录已变化')); }
      const request = ++browsing;
      error(); setBusy(true);
      try {
        const result = await native("list", { path });
        if (disposed || request !== browsing) return;
        current = result.path; entries = result.entries; selected = null;
        el("path").value = current; el("search").value = "";
        el("navigation").hidden = false; el("empty").hidden = true; el("list-wrap").hidden = false;
        el("list").scrollTop = 0;
        breadcrumbs(); renderList();
        return true;
      } catch (e) { if (!disposed && request === browsing) { error(e); el("path").value = current; } }
      finally { if (!disposed && request === browsing) setBusy(false); }
    }
    async function connect(event) {
      event.preventDefault();
      if (busy) return;
      error(); setBusy(true);
      const profile = { host: el("host").value.trim(), port: Number(el("port").value), username: el("username").value.trim(), authMethod: el("auth-method").value };
      if (profile.authMethod === "keyFile") profile.keyPath = el("key-path").value.trim();
      try {
        const result = await app.native("probe", { host: profile.host, port: profile.port });
        connection = result.connection;
        if (disposed) { await native("disconnect"); return; }
        if (result.needsTrust) {
          el("empty").hidden = true; el("trust").hidden = false;
          el("fingerprint").textContent = result.fingerprint;
          el("badge").textContent = "确认身份";
          const accepted = await new Promise((resolve) => { trustAnswer = resolve; });
          trustAnswer = null; el("trust").hidden = true;
          if (!accepted || disposed) { await native("disconnect"); connection = null; return; }
        }
        const credentials = profile.authMethod === "keyFile" ? { passphrase: el("passphrase").value } : { password: el("password").value };
        const auth = await native("authenticate", { ...profile, ...credentials, trust: true });
        el("password").value = ""; el("passphrase").value = "";
        if (disposed) { await native("disconnect"); return; }
        home = auth.path;
        recent.splice(0, recent.length, profile, ...recent.filter((p) => p.host !== profile.host || p.port !== profile.port || p.username !== profile.username).slice(0, 3));
        app.settings.set("recent", recent);
        el("connect-form").hidden = true; el("connected").hidden = false;
        el("identity").textContent = profile.username; el("address").textContent = `${profile.host}:${profile.port}`;
        el("badge").textContent = "已连接"; el("badge").classList.add("online");
        el("badge").title = auth.fingerprint; renderRecent();
        await browse(home);
      } catch (e) {
        error(e);
        if (connection) await native("disconnect").catch(() => {});
        connection = null;
      } finally {
        el("password").value = ""; el("passphrase").value = "";
        if (!disposed) {
          if (!home) { el("badge").textContent = "未连接"; el("empty").hidden = !el("error").hidden; }
          setBusy(false);
        }
      }
    }
    async function disconnect() {
      if (busy || transferring) return;
      const pending = connection;
      connection = null; home = ""; current = ""; entries = []; selected = null; browsing++;
      if (pending) await app.native("disconnect", { connection: pending }).catch(() => {});
      app.folders.unmount();
      el("connect-form").hidden = false; el("connected").hidden = true;
      el("navigation").hidden = true; el("list-wrap").hidden = true; el("empty").hidden = false;
      el("badge").textContent = "未连接"; el("badge").classList.remove("online"); el("count").textContent = "";
      error(); renderRecent(); updateSelection();
    }
    async function download(entry) {
      if (activeTransfer) {
        if (activeTransfer.path === entry.path) return activeTransfer.promise;
        throw new Error("已有图像正在传输，请等待完成或先取消");
      }
      const pending = connection;
      if (!pending) throw new Error("连接已断开，请重新连接 SSH");
      const run = async () => {
        transferring = true;
        transfer.hidden = false; transfer.querySelector('[data-k="file"]').textContent = entry.name;
        transfer.querySelector('[data-k="progress"]').textContent = "正在读取远程图像…";
        transfer.querySelector("progress").value = 0;
        transfer.querySelector("button").disabled = false;
        const poll = async () => {
          try {
            const p = await app.native("progress", { connection: pending });
            if (!transferring || disposed) return;
            transfer.querySelector("progress").value = p.total ? p.done / p.total : 0;
            transfer.querySelector('[data-k="progress"]').textContent = `${bytes(p.done)} / ${bytes(p.total)} · 正在缓存到本机`;
          } catch (_) {}
          if (transferring && !disposed) timer = setTimeout(poll, 400);
        };
        timer = setTimeout(poll, 400);
        try {
          const result = await app.native("download", { connection: pending, path: entry.path, size: entry.size, modified: entry.modified });
          if (disposed || connection !== pending) throw new Error("连接已断开");
          if (result.path !== entry.local) throw new Error("缓存路径已变化，请刷新远程目录");
          return result.path;
        } finally {
          transferring = false; activeTransfer = null; clearTimeout(timer); transfer.hidden = true;
          pumpPreviews();
        }
      };
      const promise = run();
      activeTransfer = { path: entry.path, promise };
      return promise;
    }
    async function load(file = null) {
      if (busy) return;
      error();
      if (!file && selected?.directory) { await browse(selected.path); if (!current || !el("error").hidden) return; }
      const target = file || (selected && !selected.directory ? selected : null);
      setBusy(true);
      try {
        mountFolder();
        dialog.close();
        if (target) { if (target.supported) await app.open(target.local); else await inspect(target); }
      } catch (e) { if (!disposed) { if (!dialog.open) dialog.showModal(); error(e); } }
      finally { if (!disposed) setBusy(false); }
    }
    function mountFolder() {
      app.folders.mount({ name: current.split('/').filter(Boolean).pop() || '/', path: current, address: el('address').textContent,
        entries, resolve: download, thumbnail: preview, navigate, inspect });
    }
    async function navigate(path) {
      if (disposed || !connection) throw new Error('连接已断开，请重新连接');
      if (!await browse(path)) throw new Error(el('error').textContent || '无法打开远程目录');
      mountFolder();
    }
    const documentView = document.createElement('dialog'); documentView.className = 'ssh-document';
    documentView.innerHTML = '<header><h2></h2><button aria-label="关闭文件查看">×</button></header><p class="ssh-document-info"></p><div class="ssh-document-links"></div><pre></pre>';
    document.body.append(documentView);
    documentView.querySelector('button').onclick = () => documentView.close();
    documentView.addEventListener('keydown', e => e.stopPropagation());
    documentView.addEventListener('click', e => { const r = documentView.getBoundingClientRect(); if (e.target === documentView && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)) documentView.close(); });
    async function inspect(entry) {
      setBusy(true);
      documentView.querySelector('h2').textContent = entry.name;
      documentView.querySelector('.ssh-document-info').textContent = `${entry.path} · ${bytes(entry.size || 0)}`;
      documentView.querySelector('.ssh-document-links').replaceChildren();
      const text = documentView.querySelector('pre'); text.textContent = '正在读取文件内容…';
      if (!documentView.open) documentView.showModal();
      try {
        const result = await native('readText', {path:entry.path,size:entry.size,modified:entry.modified});
        if (disposed || !documentView.open) return;
        text.textContent = result.text;
        if (result.truncated) documentView.querySelector('.ssh-document-info').textContent += ' · 仅显示前 256 KB';
        if (/\.mdoc$/i.test(entry.name)) {
          const movies = [...new Set([...result.text.matchAll(/^SubFramePath\s*=\s*(.+)$/gm)].map(m => m[1].trim().split(/[\\/]/).pop()))];
          const links = documentView.querySelector('.ssh-document-links');
          const summary = document.createElement('p'); summary.textContent = `${(result.text.match(/^\[ZValue\s*=/gm)||[]).length} 条倾转记录 · ${movies.length} 个关联电影`; links.append(summary);
          for (const name of movies) { const b = document.createElement('button'); b.textContent = `查找 ${name}`; b.onclick = async () => {
            documentView.close();
            try { const target = await native('resolveMovie',{path:entry.path,movie:name}); await navigate(target.folder); S.folder.view.query = name; buildStrip(); } catch (e) { app.ui.toast(String(e)); }
          }; links.append(b); }
        }
      } catch (e) { if (!disposed) text.textContent = String(e); }
      finally { if (!disposed) setBusy(false); }
    }
    transfer.querySelector("button").onclick = () => {
      if (!connection) return;
      transfer.querySelector("button").disabled = true;
      transfer.querySelector('[data-k="progress"]').textContent = "正在取消…";
      native("cancel").catch(() => {});
    };
    el("connect-form").onsubmit = connect;
    el("path-form").onsubmit = (event) => { event.preventDefault(); if (!busy) browse(el("path").value.trim() || home); };
    const closeDialog = () => { if (trustAnswer) trustAnswer(false); dialog.close(); };
    el("close").onclick = closeDialog;
    // WebKit versions without closedby="any" need explicit light dismissal.
    const outside = (e) => {
      const r = dialog.getBoundingClientRect();
      return e.target === dialog && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom);
    };
    let pressedOutside = false;
    dialog.addEventListener("pointerdown", (e) => { pressedOutside = e.button === 0 && outside(e); });
    dialog.addEventListener("click", (e) => { if (pressedOutside && outside(e)) closeDialog(); pressedOutside = false; });
    dialog.oncancel = () => { if (trustAnswer) trustAnswer(false); };
    // Keep viewer shortcuts (slice stepping, save, tools) inside the modal.
    dialog.addEventListener("keydown", (event) => event.stopPropagation());
    dialog.onclose = () => { if (!busy) { el("password").value = ""; el("passphrase").value = ""; } toolbar.focus(); };
    el("trust-yes").onclick = () => trustAnswer?.(true);
    el("trust-no").onclick = () => trustAnswer?.(false);
    el("disconnect").onclick = disconnect;
    el("up").onclick = () => browse(current.replace(/\/[^/]+\/?$/, "") || "/");
    el("home").onclick = () => browse(home);
    el("refresh").onclick = () => browse(current);
    el("search").oninput = () => { selected = null; renderList(); };
    el("hidden").onchange = () => { selected = null; renderList(); };
    el("load").onclick = () => load();
    el("auth-method").onchange = syncAuth;
    el("pick-key").onclick = async () => {
      try { const path = await app.dialog.pickKey(); if (path && !disposed) el("key-path").value = path; }
      catch (e) { if (!disposed) error(e); }
    };
    el("show-password").onclick = () => {
      const show = el("password").type === "password";
      el("password").type = show ? "text" : "password";
      el("show-password").textContent = show ? "隐藏" : "显示";
      el("show-password").setAttribute("aria-label", show ? "隐藏密码" : "显示密码");
    };
    const toolbar = app.ui.addToolbarButton({ label: "SSH", title: "连接服务器并打开远程文件夹", onClick: () => {
      if (!dialog.open) dialog.showModal();
      if (!home) el("host").focus();
    } });
    toolbar.innerHTML = `${icon("server")}<span>SSH</span>`;
    toolbar.classList.add("ssh-toolbar");
    renderRecent(); updateSelection();
    this.deactivate = () => {
      disposed = true; browsing++; trustAnswer?.(false); clearTimeout(timer);
      previewObserver.disconnect();
      for (const job of previewQueue.splice(0)) job.reject(new Error("插件已关闭"));
      previews.clear();
      if (connection) native("disconnect").catch(() => {});
      app.folders.unmount(); dialog.remove(); transfer.remove(); documentView.remove();
    };
  },
});
