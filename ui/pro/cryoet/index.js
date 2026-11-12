"use strict";
// The Cryo-ET workspace: one tilt series and its dataset (tomo::dataset,
// kept as <stem>.tomo.json in 2_tomograms/ of the project folder, beside the
// stack in an old flat project) carried through motion correction, CTF, tilt
// alignment, reconstruction and post-processing. The dataset is held by the
// stack's path, so the workspace keeps it while its tomograms are open.
// The folder flow (the wall of series, session.js) sits above the steps.
// The step modules (motion.js, ctf.js, align.js, recon.js, post.js) fill
// `sections` and call `register` with their hooks.
// Shares app.js globals: S, T, F, invoke, $, setFft, openPath, saveDir.
(() => {
  const STEPS = ["① 运动校正", "② CTF", "③ 倾转对齐", "④ 重构", "⑤ 去噪 / 去伪影"];
  const W = window.TifMrcViewPro.cryoet = {
    dataset: null,
    busy: false,
    sections: [],
    hooks: new Map(),
    current: 2,
  };

  const button = document.createElement("button");
  button.id = "btn-cryoet";
  button.className = "toggle";
  button.title = "Cryo-ET 工作区：运动校正、CTF、倾转对齐、重构、去噪与缺失楔补偿";
  button.innerHTML = '<svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9" ry="4" transform="rotate(-35 12 12)"/><ellipse cx="12" cy="12" rx="9" ry="4" transform="rotate(35 12 12)"/><path d="M12 2v20"/></svg><span>Cryo-ET</span>';
  $("quickbar").append(button);

  const panel = document.createElement("section");
  panel.id = "cryoet-workspace";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head"><b>Cryo-ET · 冷冻电子断层</b><button id="cryo-close" class="small" title="关闭">✕</button></div>
    <div id="cryo-dataset">
      <span id="cryo-dataset-name" class="dim"></span>
      <button id="cryo-use" class="small" title="把当前打开的倾转序列作为工作区的数据集">使用当前叠层</button>
      <button id="cryo-export" class="small" title="把对齐、CTF 等结果导出给其他软件" disabled>导出…</button>
    </div>
    <div id="cryo-export-box" class="al-opts" hidden>
      <label class="al-wide">格式<select id="cryo-export-format">
        <option value="relion5">RELION 5 断层项目（tilt_series.star + 每张倾转图）——直接接着做子断层平均</option>
        <option value="imod">IMOD / etomo 文件夹（.st .xf .tlt .defocus）——IMOD、Warp 都能导入</option>
        <option value="aligned">对齐后的倾转序列（MRC + .tlt）——AreTomo、EMAN2、Dynamo 等</option>
        <option value="table">表格 CSV（每张的倾角、平移、旋转、放大率、离焦）</option>
      </select></label>
      <label>对齐叠层 binning<select id="cryo-export-bin"><option>1</option><option>2</option><option selected>4</option><option>8</option></select></label>
      <div class="al-row"><button id="cryo-export-go" class="small primary">选择文件夹并导出…</button></div>
    </div>
    <div id="cryo-session"></div>
    <details id="cryo-advanced">
      <summary>高级设置（分步操作）</summary>
      <nav id="cryo-steps" aria-label="Cryo-ET 步骤"></nav>
      <div id="cryo-body"></div>
    </details>
    <div id="cryo-foot"><span id="cryo-status" role="status" class="dim"></span><button id="cryo-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  W.panel = panel;

  for (const [i, name] of STEPS.entries()) {
    const b = document.createElement("button");
    b.textContent = name;
    b.onclick = () => W.select(i);
    $("cryo-steps").append(b);
    const s = document.createElement("section");
    s.hidden = true;
    $("cryo-body").append(s);
    W.sections.push(s);
  }

  W.register = (i, hooks) => W.hooks.set(i, hooks);
  W.status = (text) => { $("cryo-status").textContent = text; };
  W.name = (path) => path?.split(/[\\/]/).pop() || "";

  W.select = (i) => {
    W.current = i;
    W.sections.forEach((s, j) => {
      s.hidden = i !== j;
      const b = $("cryo-steps").children[j];
      b.classList.toggle("on", i === j);
      b.setAttribute("aria-current", i === j ? "step" : "false");
    });
    if (!panel.hidden) W.hooks.get(i)?.enter?.();
  };

  // Which steps have results, shown as ✓ on the step bar.
  function marks() {
    const d = W.dataset;
    const done = [
      !!(d && (d.even || d.odd)),
      !!d?.ctf,
      !!d?.alignment,
      !!d?.tomograms.some((t) => ["full", "even", "odd"].includes(t.kind)),
      !!d?.tomograms.some((t) => ["denoised", "wedge"].includes(t.kind)),
    ];
    [...$("cryo-steps").children].forEach((b, i) => { b.textContent = STEPS[i] + (done[i] ? " ✓" : ""); });
  }

  // Makes `d` the workspace's dataset and lets every step show it.
  W.setDataset = (d) => {
    W.dataset = d;
    $("cryo-dataset-name").textContent = d?.angles.length ? `数据集：${W.name(d.stack)} · ${d.angles.length} 个倾角` : "";
    $("cryo-export").disabled = !d?.alignment;
    marks();
    for (const h of W.hooks.values()) h.dataset?.(d);
  };

  $("cryo-export").onclick = () => { $("cryo-export-box").hidden = !$("cryo-export-box").hidden; };
  $("cryo-export-format").onchange = () => { $("cryo-export-bin").parentElement.hidden = $("cryo-export-format").value !== "aligned"; };
  $("cryo-export-format").onchange();
  $("cryo-export-go").onclick = async () => {
    if (!W.dataset?.alignment) return W.status("先完成③倾转对齐再导出");
    const dir = await invoke("pick_folder", { dir: saveDir() });
    if (!dir) return;
    const files = await W.step("导出", "tomo_export", { format: $("cryo-export-format").value, dir, bin: +$("cryo-export-bin").value });
    if (files) W.status(`已导出 ${files.length} 个文件到 ${dir}`);
  };

  // Loads the dataset of a tilt series by its path.
  W.load = async (stack) => {
    const d = await invoke("tomo_dataset", { stack });
    W.setDataset(d);
    return d;
  };

  $("cryo-use").onclick = async () => {
    if (!S.info) return W.status("请先打开一个倾转序列");
    if (S.info.sizeZ < 3) return W.status("当前图像不足 3 层，不是倾转序列");
    try {
      const d = await W.load(S.info.path);
      W.status(d.angles.length ? `已载入 ${d.angles.length} 个倾角` : "没有找到倾角文件（.mdoc / .tlt / .rawtlt），请在倾转对齐里输入起始角和步长");
    } catch (e) { W.status(`无法载入：${e}`); }
  };

  // Runs a long backend call with progress in the status line, the docked
  // 「正在处理」 pane (live.js) and a cancel button; the workspace's buttons are
  // disabled meanwhile.
  W.run = async (label, call, cancel = "tomo_cancel") => {
    if (W.busy) return null;
    W.busy = true;
    const dock = window.TifMrcViewPro.liveDock;
    $("cryo-cancel").hidden = false;
    $("cryo-cancel").onclick = () => invoke(cancel);
    const buttons = [...panel.querySelectorAll("#cryo-body button, #cryo-dataset button, #cryo-session .cs-act")];
    const was = buttons.map((b) => b.disabled);
    buttons.forEach((b) => { b.disabled = true; });
    await dock.start();
    const un = await T.event.listen("tomo-progress", (e) => {
      const p = e.payload;
      const text = `${p.stage || label} ${p.done} / ${p.total}`;
      W.status(text);
      dock.progress(text);
      dock.refresh();
    });
    const un2 = await T.event.listen("motion-progress", () => dock.refresh());
    W.status(`${label}…`);
    dock.progress(`${label}…`);
    try {
      return await call();
    } catch (e) {
      W.status(String(e).includes("已取消") ? `${label}已取消` : `${label}失败：${e}`);
      return null;
    } finally {
      un();
      un2();
      dock.stop();
      W.busy = false;
      $("cryo-cancel").hidden = true;
      buttons.forEach((b, i) => { b.disabled = was[i]; });
      marks();
      document.dispatchEvent(new CustomEvent("cryoet-idle"));
    }
  };

  // A dataset step: `{result, dataset}` comes back and the dataset is kept.
  W.step = async (label, command, req) => {
    const r = await W.run(label, () => invoke(command, { req: { stack: W.dataset.stack, ...req } }));
    if (r) W.setDataset(r.dataset);
    return r?.result ?? null;
  };

  function close() {
    panel.hidden = true;
    document.dispatchEvent(new CustomEvent("cryoet-close"));
    button.classList.remove("on");
  }
  button.onclick = () => {
    if (!panel.hidden) return close();
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    panel.hidden = false;
    button.classList.add("on");
    // Start with the open tilt series when there is no dataset yet.
    if (!W.dataset && S.info?.sizeZ >= 3 && !W.busy) $("cryo-use").click();
    W.select(W.current);
    document.dispatchEvent(new CustomEvent("cryoet-open"));
  };
  $("cryo-close").onclick = close;
  document.addEventListener("pro-panel", (e) => { if (e.detail !== panel.id && !panel.hidden) close(); });
  $("btn-fft").addEventListener("click", () => { if (F.on) close(); });
  W.select(W.current);
})();
