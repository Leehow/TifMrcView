"use strict";
// The MemBrain-seg model for membrane segmentation (labels panel, one-click
// cryo-ET). Its weights carry no licence, so the app does not ship them: the
// first time, this asks the user to download the checkpoint from the
// MemBrain-seg page and pick the file; the backend remembers it.
(() => {
  const invoke = window.__TAURI__.core.invoke;
  let open = null;

  // Resolves to the model path, or null when the user closes the box.
  async function ensure() {
    const st = await invoke("membrane_status");
    if (!st.available) throw new Error("膜分割只在 Pro 版里");
    if (st.model) return st.model;
    if (open) return open;
    open = new Promise((resolve) => {
      const dlg = document.createElement("div");
      dlg.className = "dialog mem-model";
      dlg.setAttribute("role", "dialog");
      dlg.innerHTML = `
        <div class="dlg-head"><h3>膜分割需要 MemBrain-seg 模型</h3><button class="small" data-act="close" title="关闭 (Esc)">✕</button></div>
        <div class="dim">膜分割用 MemBrain-seg 作者发布的模型（Lamm 等，2024）。模型没有附带许可，TifMrcView 不能自带，请从官方页面下载（推荐 MemBrain_seg_v10_beta.ckpt，约 730 MB），再在这里选择下载好的文件。只需选一次，以后自动载入。</div>
        <div class="row">
          <button class="small" data-act="page">打开下载页</button>
          <button class="small primary" data-act="pick">选择模型文件…</button>
        </div>
        <div class="dim" data-role="msg"></div>`;
      const msg = dlg.querySelector("[data-role=msg]");
      const done = (v) => {
        dlg.remove();
        window.removeEventListener("keydown", esc, true);
        open = null;
        resolve(v);
      };
      const esc = (e) => {
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); done(null); }
      };
      window.addEventListener("keydown", esc, true);
      dlg.addEventListener("click", async (e) => {
        const act = e.target.dataset && e.target.dataset.act;
        if (act === "close") done(null);
        if (act === "page") invoke("open_link", { url: st.page }).catch((err) => (msg.textContent = `无法打开链接：${err}`));
        if (act === "pick") {
          e.target.disabled = true;
          msg.textContent = "正在检查模型…";
          try {
            const p = await invoke("membrane_choose");
            if (p) return done(p);
            msg.textContent = "";
          } catch (err) {
            msg.textContent = String(err);
          } finally {
            e.target.disabled = false;
          }
        }
      });
      document.body.appendChild(dlg);
    });
    return open;
  }

  window.TifMrcViewPro.membrane = { ensure };
})();
