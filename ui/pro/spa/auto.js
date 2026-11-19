"use strict";
// The panel's default view: a three-step guide, each step a card with what
// it does, its button, its result and what to do next. A step lights up
// when the one before it is done; done steps fold to their result line
// (a click on the header opens them again).
//   1 运动校正和 CTF: motion correction (movies only) and the CTF of every
//     micrograph, with the optics read from the open folder; pixel size and
//     voltage are asked only when no file had them.
//   2 挑颗粒: the user marks a few particles with the viewer's one label
//     brush and ticks the layers (examples.js finds the rest and shows them
//     for checking). The old way, the kinds of particle found on a few
//     denoised micrographs as cards to click, stays as an option for tidy
//     tutorial data.
//   3 二维分类 → 三维重构: extraction and 2D classes with the sharp ones
//     chosen (spa_run "autoClasses"), the user looks at them and changes the
//     choice, then the point group, the initial model and the refinement
//     ("autoMap"). The steps by hand stay under 高级设置.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt } = W;
  const lockSvg = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></svg>';
  $("spa-auto").innerHTML = `
    <p class="dim spa-intro">三步得到颗粒的三维结构。按顺序做，每步做完下一步才会亮起；关掉软件再打开，会接着上次做到的地方。</p>

    <section class="spa-wz" id="spa-wz-1">
      <header><span class="spa-wz-n">1</span><b>运动校正和 CTF</b><span class="spa-wz-state"></span></header>
      <div class="spa-wz-body">
        <p class="dim">把电影里每一帧的漂移对齐、叠成清晰的显微图，再测出每张图的离焦（CTF）。全自动，参数从文件夹里的配置文件读取。</p>
        <p id="spa-optics-line" class="dim"></p>
        <div id="spa-ask" class="al-opts" hidden>
          <p class="dim al-wide">文件夹里没有找到这些参数，请填写：</p>
          <label id="spa-ask-pixel-row">像素大小 Å<input id="spa-ask-pixel" type="number" min="0.05" step="0.001" placeholder="例如 1.06"></label>
          <label id="spa-ask-kv-row">电压 kV<input id="spa-ask-kv" type="number" min="20" max="400" step="10" placeholder="例如 300"></label>
        </div>
        <div class="al-row"><button id="spa-a-start" class="primary">开始运动校正和 CTF</button></div>
      </div>
      <p class="spa-wz-result"></p>
    </section>

    <section class="spa-wz" id="spa-wz-2">
      <header><span class="spa-wz-n">2</span><b>挑颗粒</b><span class="spa-wz-state"></span></header>
      <div class="spa-wz-body">
        <p class="dim">程序不知道你要的是哪种颗粒，所以先由你在一张显微图上涂几个例子，程序再到所有显微图里找出像它们的，给你检查。</p>
        <div id="spa-ex-slot"></div>
        <details id="spa-a-autobox" class="spa-autobox">
          <summary>换一种方式：让程序先找出几种颗粒，你点选（适合教程那样干净的数据）</summary>
          <div class="al-row"><button id="spa-a-discover">在几张去噪的显微图上找颗粒种类</button></div>
          <div id="spa-a-kinds" class="ws-kinds"></div>
          <p id="spa-a-hint" class="dim"></p>
          <div class="al-row" id="spa-a-finish" hidden><button id="spa-a-go" class="primary" disabled>用选中的种类，去第 3 步</button></div>
        </details>
      </div>
      <p class="spa-wz-result"></p>
    </section>

    <section class="spa-wz" id="spa-wz-3">
      <header><span class="spa-wz-n">3</span><b>平均：二维分类和三维重构</b><span class="spa-wz-state"></span></header>
      <div class="spa-wz-body">
        <h4>3a · 二维分类</h4>
        <p class="dim">把挑到的颗粒剪下来，按长相分成约 20 类，每类叠加平均。平均后清晰的类说明颗粒挑得好，程序会自动选中它们。</p>
        <div class="al-row"><button id="spa-c-run" class="primary">剪出颗粒并做二维分类</button></div>
        <div id="spa-c-box" hidden>
          <p class="dim spa-wz-tip">蓝框是选中的类，只有它们的颗粒用于三维重构。点一下可以加选或去掉：模糊的、只有噪声的、冰渣或碳膜边的类去掉；能看清结构的类都留着（顶视、侧视等不同朝向都要）。</p>
          <div id="spa-c-gallery" class="ws-gallery"></div>
          <p id="spa-c-sum" class="dim"></p>
        </div>
        <h4>3b · 三维重构</h4>
        <p class="dim">用选中的类：程序判断对称性，从头建初始模型，再做金标准三维精修（颗粒分两半各自对齐，两半的 FSC 给出分辨率），最后锐化。</p>
        <div class="al-opts">
          <label>对称性<select id="spa-a-sym"><option value="">自动判断</option><option>C1</option></select></label>
          <label>或填写<input id="spa-a-sym-text" type="text" placeholder="如 C4、D7、O" size="5"></label>
        </div>
        <div class="al-row"><button id="spa-m-run" class="primary">用选中的类做三维重构</button></div>
        <div id="spa-m-box" hidden>
          <p id="spa-a-result"></p>
          <canvas id="spa-m-fsc" class="spa-fsc" width="600" height="220" title="FSC：灰 = 未掩膜，蓝 = 掩膜；虚线 0.143"></canvas>
          <div class="al-row"><button id="spa-a-map" class="small primary">打开三维图</button><button id="spa-a-more" class="small">详细结果</button></div>
        </div>
      </div>
      <p class="spa-wz-result"></p>
    </section>`;

  const cards = [1, 2, 3].map((i) => $(`spa-wz-${i}`));
  const userOpen = new Map(); // cards opened or closed by hand: i → open
  cards.forEach((c, i) => {
    c.querySelector("header").onclick = () => {
      if (c.classList.contains("locked")) return;
      const open = !c.classList.contains("open");
      userOpen.set(i, open);
      c.classList.toggle("open", open);
    };
  });

  const total = (p) => p.mics.reduce((a, m) => a + (m.picks || 0), 0);
  const kindsReady = { dir: "" }; // a kind was chosen in this session (step 2's other way)
  let classes = null; // the gallery's state (W.drawClasses)
  let classesDirty = false;

  // What is done: [step 1, step 2, 3a, 3b].
  function done(p) {
    if (!p) return [false, false, false, false];
    const motion = !p.movies || p.mics.every((m) => m.path !== m.movie);
    return [motion && p.mics.some((m) => m.ctf), total(p) > 0 || kindsReady.dir === p.dir, !!p.class2d, !!p.refine];
  }

  function median(v) {
    const s = v.filter((x) => isFinite(x)).sort((a, b) => a - b);
    return s.length ? s[Math.floor(s.length / 2)] : NaN;
  }

  function opticsLine(p) {
    const f = p.opticsFound || {};
    const bits = [];
    if (f.pixel) bits.push(`像素 ${fmt(p.optics.pixelA, 3)} Å`);
    if (f.kv) bits.push(`${fmt(p.optics.kv, 0)} kV`);
    if (f.cs) bits.push(`Cs ${fmt(p.optics.csMm, 2)} mm`);
    if (f.amplitude) bits.push(`振幅衬度 ${fmt(p.optics.amplitude, 2)}`);
    if (p.doseTotal > 0) bits.push(`剂量 ${fmt(p.doseTotal, 2)} e⁻/Å²`);
    if (f.file) bits.push(`读自 ${f.file}`);
    return bits.length ? `${p.mics.length} 张${p.movies ? "电影" : "显微图"} · ${bits.join(" · ")}` : `${p.mics.length} 张${p.movies ? "电影" : "显微图"}`;
  }

  // Each card's result line, state and whether it is open.
  function paint(p) {
    const d = done(p);
    const step = [d[0], d[1], d[2] && d[3]];
    const active = !p ? 0 : step.findIndex((x) => !x);
    const res = ["", "", ""];
    if (p && d[0]) {
      const ctf = p.mics.filter((m) => m.ctf);
      const df = ctf.map((m) => (m.ctf.df1 + m.ctf.df2) / 2e4);
      const ex = p.mics.filter((m) => m.excluded).length;
      res[0] = `✓ ${ctf.length} 张显微图 · 离焦 ${fmt(Math.min(...df), 1)}–${fmt(Math.max(...df), 1)} µm · CTF 拟合到约 ${fmt(median(ctf.map((m) => m.ctf.fitRes)), 1)} Å${ex ? ` · ${ex} 张不用` : ""}`;
    }
    if (p && d[1]) {
      const n = total(p);
      res[1] = n ? `✓ ${n} 个颗粒，来自 ${p.mics.filter((m) => m.picks).length} 张显微图 · 直径约 ${fmt(p.diameterA, 0)} Å` : `✓ 已选颗粒种类（直径约 ${fmt(p.diameterA, 0)} Å），第 3 步会先在所有显微图上挑颗粒`;
    }
    if (p && d[3]) {
      const r = p.refine;
      res[2] = `✓ 分辨率 ${fmt(r.res)} Å（掩膜后 ${fmt(r.resMasked)} Å）· ${r.particles} 个颗粒 · 对称性 ${r.params.symmetry}`;
    } else if (p && d[2]) {
      res[2] = `二维分类已完成：检查选中的类，然后按「用选中的类做三维重构」`;
    }
    const next = [
      "下一步：在显微图上涂几个颗粒（第 2 步）",
      "下一步：二维分类（第 3 步）",
      "",
    ];
    cards.forEach((c, i) => {
      const locked = !p || (i > 0 && !step[i - 1]);
      c.classList.toggle("locked", locked);
      c.classList.toggle("done", !!step[i]);
      c.classList.toggle("active", i === active);
      const st = c.querySelector(".spa-wz-state");
      if (locked) st.innerHTML = `${lockSvg}<span>${p ? `先完成第 ${i} 步` : "先打开一张显微图或电影"}</span>`;
      else st.textContent = step[i] ? "已完成" : i === active ? "现在做这一步" : "";
      const r = c.querySelector(".spa-wz-result");
      r.textContent = res[i] + (step[i] && next[i] && i + 1 === active ? `　→ ${next[i]}` : "");
      r.hidden = !res[i];
      const open = !locked && (userOpen.has(i) ? userOpen.get(i) : i === active || (i === 2 && step[2]));
      c.classList.toggle("open", open);
    });
    // A redo is a plain button: the next thing to do keeps the blue one.
    $("spa-a-start").classList.toggle("primary", !d[0]);
    $("spa-c-run").classList.toggle("primary", !d[2]);
    $("spa-m-run").classList.toggle("primary", !d[3]);
    $("spa-c-run").textContent = d[2] ? "重新做二维分类" : "剪出颗粒并做二维分类";
    $("spa-m-run").disabled = W.busy || !d[2];
    $("spa-m-run").textContent = d[3] ? "重新做三维重构" : "用选中的类做三维重构";
  }

  async function showKinds() {
    const p = W.project;
    const box = $("spa-a-kinds");
    box.replaceChildren();
    $("spa-a-finish").hidden = !p?.discovery;
    if (!p?.discovery) { $("spa-a-hint").textContent = ""; return; }
    let kinds;
    try { kinds = await invoke("spa_kinds", { dir: p.dir }); } catch (e) { W.status(`${e}`); return; }
    const chosen = W.kindsChosen = new Set(p.discovery.chosen || []);
    kinds.forEach((k, i) => {
      const card = document.createElement("figure");
      card.className = chosen.has(i) ? "on" : "";
      const img = new Image();
      img.src = `data:image/png;base64,${k.png}`;
      img.alt = `第 ${i + 1} 种`;
      const cap = document.createElement("figcaption");
      const shape = k.elongation > 1.6 ? " · 细长" : "";
      cap.textContent = `第 ${i + 1} 种 · 约 ${fmt(k.diameterA, 0)} Å · 占 ${fmt(k.share * 100, 0)}%${shape}`;
      card.append(img, cap);
      card.onclick = () => {
        chosen.has(i) ? chosen.delete(i) : chosen.add(i);
        card.className = chosen.has(i) ? "on" : "";
        $("spa-a-go").disabled = !chosen.size;
      };
      box.append(card);
    });
    $("spa-a-go").disabled = !chosen.size;
    $("spa-a-hint").textContent = `在 ${p.discovery.micrographs} 张显微图上找到 ${kinds.length} 种${p.discovery.outlined ? "（SAM 描边）" : ""}。点选你的颗粒（同一种颗粒的不同朝向可以多选），冰渣、碳膜边缘、团聚的不要选。`;
  }

  async function showClasses() {
    const p = W.project;
    $("spa-c-box").hidden = !p?.class2d;
    if (!p?.class2d) { classes = null; return; }
    classesDirty = false;
    classes = await W.drawClasses($("spa-c-gallery"), $("spa-c-sum"), () => { classesDirty = true; });
  }

  function showResult(p) {
    const r = p?.refine;
    $("spa-m-box").hidden = !r;
    if (!r) return;
    const sym = p.symmetry && p.symmetry.symmetry === r.params.symmetry ? "（自动判断）" : "";
    $("spa-a-result").textContent = `分辨率 ${fmt(r.res)} Å，掩膜后 ${fmt(r.resMasked)} Å · ${r.particles} 个颗粒 · 对称性 ${r.params.symmetry}${sym} · B 因子 ${fmt(r.bfactor, 0)} Å²`;
    W.drawFsc?.(r, $("spa-m-fsc"));
  }

  let shown = { kinds: "", classes: "" };
  W.hooks.set("auto", {
    show(p) {
      paint(p);
      if (!p) {
        shown = { kinds: "", classes: "" };
        $("spa-optics-line").textContent = "";
        $("spa-ask").hidden = true;
        $("spa-a-kinds").replaceChildren();
        $("spa-a-hint").textContent = "";
        $("spa-a-finish").hidden = true;
        $("spa-c-box").hidden = true;
        $("spa-m-box").hidden = true;
        return;
      }
      const f = p.opticsFound || { pixel: true, kv: true };
      $("spa-ask-pixel-row").hidden = !!f.pixel;
      $("spa-ask-kv-row").hidden = !!f.kv;
      $("spa-ask").hidden = !!(f.pixel && f.kv);
      $("spa-optics-line").textContent = opticsLine(p);
      $("spa-a-start").textContent = done(p)[0] ? "重新做" : p.movies ? "开始运动校正和 CTF" : "开始 CTF";
      showResult(p);
      // Rebuilding the cards or the gallery on every refresh would drop a
      // choice not saved yet: only when what they show changed.
      const km = `${p.dir}:${p.discovery?.micrographs ?? ""}`;
      if (km !== shown.kinds) {
        shown.kinds = km;
        if (p.discovery) showKinds();
        else { $("spa-a-kinds").replaceChildren(); $("spa-a-hint").textContent = ""; $("spa-a-finish").hidden = true; }
      }
      const cm = `${p.dir}:${p.class2d ? `${p.class2d.path}:${p.class2d.counts.join(",")}:${p.class2d.selected.join(",")}` : ""}`;
      if (cm !== shown.classes) { shown.classes = cm; showClasses(); }
    },
  });

  // Step 1.
  $("spa-a-start").onclick = async () => {
    if (!W.project) return W.status("请先打开一张显微图或电影");
    const f = W.project.opticsFound || { pixel: true, kv: true };
    if (!f.pixel) {
      const px = +$("spa-ask-pixel").value;
      if (!(px > 0)) return W.status("文件夹里没有找到像素大小，请填写（Å）");
      $("spa-pixel").value = px;
    }
    const pending = {};
    if (!f.kv) {
      const kv = +$("spa-ask-kv").value;
      if (!(kv > 0)) return W.status("文件夹里没有找到电压，请填写（kV）");
      $("spa-kv").value = kv;
      pending.kv = kv;
      pending.kvFromUser = true;
    }
    W.pendingOptics = pending;
    const again = done(W.project)[0];
    let p = W.project;
    if (p.movies && (again || p.mics.some((m) => m.path === m.movie))) {
      if (await $("spa-run-motion").onclick() == null) return;
      p = W.project;
    } else if (pending.kvFromUser || !f.pixel) {
      try { await W.settings(); } catch (e) { return W.status(`${e}`); }
    }
    if (again || !p.mics.some((m) => m.ctf)) {
      if (await $("spa-run-ctf").onclick() == null) return;
    }
    userOpen.clear();
    W.show(W.project);
    W.status("第 1 步完成。下一步：按第 2 步的说明，在显微图上涂几个颗粒");
    W.examples?.reload();
  };

  // Step 2, the other way.
  $("spa-a-discover").onclick = async () => {
    if (!W.project) return W.status("请先打开一张显微图或电影");
    if (await W.run("寻找颗粒种类", "discover", { sam: true }) != null) showKinds();
  };
  $("spa-a-go").onclick = async () => {
    const d = await W.run("保存选择", "choose", [...(W.kindsChosen || [])].sort((a, b) => a - b));
    if (d == null) return;
    kindsReady.dir = W.project.dir;
    W.next(3);
  };

  // Step 3.
  $("spa-c-run").onclick = async () => {
    const p = W.project;
    if (!p) return;
    const picked = total(p) > 0 && kindsReady.dir !== p.dir;
    const r = await W.run(picked ? "剪出颗粒并做二维分类" : "挑颗粒、剪出并做二维分类", "autoClasses", { picked, sam: true });
    if (r == null) return;
    shown.classes = "";
    W.show(W.project);
    W.status("二维分类完成：检查选中的类（蓝框），然后按「用选中的类做三维重构」");
  };
  $("spa-m-run").onclick = async () => {
    if (!W.project?.class2d) return;
    if (classesDirty && classes) {
      const n = await W.run("保存选中的类", "select", [...classes.chosen].sort((a, b) => a - b));
      if (n == null) return;
      classesDirty = false;
    }
    const symmetry = ($("spa-a-sym-text").value.trim() || $("spa-a-sym").value).toUpperCase();
    const r = await W.run("三维重构", "autoMap", { symmetry });
    if (r == null) return;
    showResult(W.project);
    W.status(`完成：分辨率 ${fmt(r.res)} Å。按「打开三维图」查看`);
  };

  // Opens step `i` (1-based) and scrolls to it.
  W.next = (i) => {
    userOpen.clear();
    if (W.project) W.show(W.project);
    cards[i - 1].scrollIntoView({ behavior: "smooth", block: "start" });
  };
  W.showResult = showResult;
  $("spa-a-map").onclick = () => W.project?.refine && openPath(W.project.refine.sharpened, true);
  $("spa-a-more").onclick = () => { $("spa-advanced").open = true; W.select(5); $("spa-advanced").scrollIntoView({ behavior: "smooth" }); };
})();
