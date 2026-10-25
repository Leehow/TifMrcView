"use strict";

// Plugin: records the Z (or T) stack as an H.264 MP4, with the
// current contrast, channels, annotations and scale bar. Frames are rendered
// here and encoded by the native half (src-tauri/src/plugins/recorder.rs).

TifMrcView.register({
  id: "recorder",
  name: "录制视频",
  version: "1.0.0",
  description: "把 Z / T 序列录成 MP4，带当前的对比度、通道、标注和比例尺",

  activate(app) {
    if (!app.hasNative()) throw new Error("这个版本没有编译录制功能");
    const body = app.ui.addToolSection("录制视频 (MP4)");
    body.innerHTML = `
      <div class="row">
        <button data-k="mark-from" class="small" title="把当前层设为起点">⇤ 设起点</button>
        <button data-k="mark-to" class="small" title="把当前层设为终点">设终点 ⇥</button>
        <button data-k="go" class="primary">● 录制</button>
        <button data-k="stop" class="small" hidden>■ 停止</button>
        <button data-k="more" class="small" title="帧率、区域、尺寸、画质等设置">详细 ▾</button>
      </div>
      <div data-k="summary" class="dim"></div>
      <div data-k="details" hidden>
        <div class="row">
          <select data-k="axis" title="按哪个维度播放"><option value="z">Z 层</option><option value="t">时间 T</option></select>
          <input type="number" data-k="from" min="1" step="1" title="起点" style="width: 64px" />
          <span>–</span>
          <input type="number" data-k="to" min="1" step="1" title="终点" style="width: 64px" />
        </div>
        <div class="row">
          <label>帧率 <input type="number" data-k="fps" min="1" max="60" step="1" style="width: 64px" /> fps</label>
          <label title="播完后倒着再播一遍"><input type="checkbox" data-k="bounce" /> 往返</label>
        </div>
        <div class="row">
          <select data-k="region" title="录哪一块">
            <option value="all">整张图</option><option value="sel">选区</option><option value="view">当前视野</option>
          </select>
          <select data-k="size" title="视频尺寸。自动：小图按整数倍放大到至少 1000 像素，全屏播放时像素清晰不糊">
            <option value="auto">自动尺寸</option><option value="0">原始尺寸</option><option value="1920">1920</option>
            <option value="1280">1280</option><option value="720">720</option>
          </select>
          <select data-k="quality" title="画质">
            <option value="0.85">高画质</option><option value="0.6">标准</option><option value="0.3">小文件</option>
          </select>
        </div>
      </div>
      <div data-k="out" class="dim"></div>`;
    const el = (k) => body.querySelector(`[data-k="${k}"]`);
    const set = app.settings;
    el("fps").value = set.get("fps", 10);
    el("bounce").checked = set.get("bounce", false);
    el("size").value = set.get("size", "auto");
    el("quality").value = set.get("quality", "0.6");
    el("region").value = "all";
    for (const k of ["fps", "size", "quality"]) el(k).addEventListener("change", () => set.set(k, el(k).value));
    el("bounce").addEventListener("change", () => set.set("bounce", el("bounce").checked));

    let running = null; // {stop: bool}

    const showDetails = (on) => {
      el("details").hidden = !on;
      el("more").textContent = on ? "详细 ▴" : "详细 ▾";
      set.set("details", on);
    };
    showDetails(set.get("details", false));
    el("more").addEventListener("click", () => showDetails(el("details").hidden));

    const axisLen = () => {
      const st = app.state();
      return el("axis").value === "t" ? st.sizeT : st.sizeZ;
    };
    function summary() {
      const n = axisLen();
      if (!app.state().info || n < 2) return (el("summary").textContent = "");
      const a = Math.round(+el("from").value || 1);
      const b = Math.round(+el("to").value || n);
      const ax = el("axis").value === "t" ? "T" : "Z";
      let frames = Math.abs(b - a) + 1;
      if (el("bounce").checked && frames > 2) frames = frames * 2 - 2;
      const secs = frames / Math.max(1, +el("fps").value || 10);
      el("summary").textContent = `${ax} ${a} → ${b}，${frames} 帧，约 ${secs.toFixed(1)} 秒`;
    }
    for (const k of ["from", "to", "fps", "bounce", "axis"]) el(k).addEventListener("input", summary);
    const mark = (k) => {
      const st = app.state();
      el(k).value = (el("axis").value === "t" ? st.t : st.z) + 1;
      summary();
    };
    el("mark-from").addEventListener("click", () => mark("from"));
    el("mark-to").addEventListener("click", () => mark("to"));

    function sync() {
      const st = app.state();
      const hasT = st.sizeT > 1;
      el("axis").querySelector('[value="t"]').disabled = !hasT;
      if (!hasT) el("axis").value = "z";
      const n = el("axis").value === "t" ? st.sizeT : st.sizeZ;
      for (const k of ["from", "to"]) el(k).max = n;
      el("from").value = 1;
      el("to").value = n;
      el("region").querySelector('[value="sel"]').disabled = !st.selection;
      if (!st.selection && el("region").value === "sel") el("region").value = "all";
      el("go").disabled = !st.info || n < 2 || !!running;
      for (const k of ["mark-from", "mark-to"]) el(k).disabled = !st.info || n < 2;
      summary();
      if (st.info && n < 2) el("out").textContent = "这张图只有一层，没有可录的序列";
      else if (!running) el("out").textContent = "";
    }
    app.on("open", sync);
    el("axis").addEventListener("change", sync);
    el("region").addEventListener("focus", () => {
      el("region").querySelector('[value="sel"]').disabled = !app.state().selection;
    });
    sync();

    /**
     * Output size: fits the chosen size and the encoder's 3840×2160 limit,
     * even numbers. "auto" blows small images up by a whole factor (crisp
     * pixels) so players don't blur them when shown full screen.
     */
    function outSize(w, h) {
      let k = 1;
      const v = el("size").value;
      const max = +v;
      if (v === "auto") k = Math.max(1, Math.ceil(1000 / Math.max(w, h)));
      else if (max) k = Math.min(k, max / Math.max(w, h));
      k = Math.min(k, 3840 / Math.max(w, h), 2160 / Math.min(w, h));
      const even = (v) => Math.max(2, Math.floor((v * k) / 2) * 2);
      return [even(w), even(h)];
    }

    async function record() {
      const st = app.state();
      if (!st.info || running) return;
      const axis = el("axis").value;
      const n = axis === "t" ? st.sizeT : st.sizeZ;
      const a = Math.max(1, Math.min(n, Math.round(+el("from").value || 1)));
      const b = Math.max(1, Math.min(n, Math.round(+el("to").value || n)));
      const step = a <= b ? 1 : -1;
      const frames = [];
      for (let i = a; i !== b + step; i += step) frames.push(i - 1);
      if (el("bounce").checked && frames.length > 2) frames.push(...frames.slice(1, -1).reverse());
      if (frames.length < 2) return app.ui.toast("至少要两帧");

      const mode = el("region").value;
      const region =
        mode === "sel" && st.selection ? st.selection : mode === "view" && st.view ? st.view : [0, 0, st.width, st.height];
      const [w, h] = outSize(region[2], region[3]);
      const fps = Math.max(1, Math.min(60, +el("fps").value || 10));
      const name = `${app.fs.stem(st.path)}_${axis}${a}-${b}.mp4`;
      const path = await app.dialog.save({ name, filter: "MP4 视频", exts: ["mp4"] });
      if (!path) return;

      running = { stop: false };
      el("go").disabled = true;
      el("stop").hidden = false;
      const t0 = performance.now();
      try {
        await app.native("begin", { path, width: w, height: h, fps, quality: +el("quality").value });
        const c = document.createElement("canvas");
        c.width = w;
        c.height = h;
        const g = c.getContext("2d", { willReadFrequently: true });
        for (let i = 0; i < frames.length; i++) {
          if (running.stop || app.state().path !== st.path) throw new Error("已停止");
          el("out").textContent = `录制中 ${i + 1} / ${frames.length}`;
          const opts = axis === "t" ? { z: st.z, t: frames[i], size: [w, h] } : { z: frames[i], t: st.t, size: [w, h] };
          const src = await app.renderRegion(region, opts);
          g.drawImage(src, 0, 0);
          const px = g.getImageData(0, 0, w, h).data;
          await app.native("frame", {}, new Uint8Array(px.buffer, px.byteOffset, px.byteLength));
        }
        const res = await app.native("finish");
        const secs = ((performance.now() - t0) / 1000).toFixed(1);
        const mb = (res.bytes / 1048576).toFixed(1);
        el("out").textContent = `已保存 ${app.fs.basename(path)}（${w}×${h}，${res.frames} 帧，${mb} MB，用时 ${secs} 秒）`;
        app.ui.toast(`视频已保存 ${path}`);
      } catch (e) {
        await app.native("cancel").catch(() => {});
        const msg = e && e.message === "已停止" ? "已停止，未保存" : `录制失败：${e}`;
        el("out").textContent = msg;
        if (msg !== "已停止，未保存") app.ui.toast(msg, 6000);
      } finally {
        running = null;
        el("stop").hidden = true;
        el("go").disabled = false;
      }
    }

    el("go").addEventListener("click", () => {
      app.ui.showTools();
      record();
    });
    el("stop").addEventListener("click", () => {
      if (running) running.stop = true;
    });
    this._stop = () => {
      if (running) running.stop = true;
    };
  },

  deactivate() {
    if (this._stop) this._stop();
  },
});
