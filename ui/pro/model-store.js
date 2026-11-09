"use strict";
// Shared presentation of image-local training records; model algorithms and
// compatibility checks remain in the native model owners.
(() => {
  const same = (a, b) => a && b && a.token === b.token && a.c === b.c && a.t === b.t;
  window.TifMrcViewPro.models = {
    setValue(id, value) {
      if (value === undefined || value === null) return;
      const el = $(id), text = String(value);
      if (el.tagName === "SELECT" && ![...el.options].some((o) => o.value === text)) {
        const option = document.createElement("option");
        option.value = option.textContent = text;
        el.append(option);
      }
      el.value = text;
    },
    attach(kind, id, current) {
      const container = () => $(id);
      function failure(error, loading = false) {
        const el = container();
        el.replaceChildren();
        el.textContent = loading ? `自动读取模型失败：${error}。未重新训练。` : `自动保存模型失败：${error}。当前模型仍在内存，请勿关闭应用。`;
        el.hidden = false;
      }
      function show(persistence, restored = false) {
        const el = container();
        el.replaceChildren();
        if (persistence?.error) { failure(persistence.error); return; }
        const saved = persistence?.saved;
        if (!saved) { el.hidden = true; return; }
        const label = document.createElement("div");
        label.textContent = `${restored ? "已自动恢复" : "已自动保存"}${saved.record.complete ? "模型" : "中间检查点"}，可直接推理`;
        const details = document.createElement("details");
        const title = document.createElement("summary");
        title.textContent = "训练记录：参数、数据来源与权重文件";
        const location = document.createElement("div");
        location.textContent = saved.recordPath;
        const reveal = document.createElement("button");
        reveal.className = "small";
        reveal.textContent = "显示模型文件";
        reveal.addEventListener("click", () => invoke("reveal_in_folder", { path: saved.recordPath }).catch((e) => failure(e, true)));
        const pre = document.createElement("pre");
        pre.textContent = JSON.stringify({ weightsFile: saved.weightsPath, ...saved.record }, null, 2);
        pre.style.cssText = "max-height:240px;overflow:auto;white-space:pre-wrap;overflow-wrap:anywhere;font-size:11px";
        details.append(title, location, reveal, pre);
        el.append(label, details);
        el.hidden = false;
      }
      T.event.listen("trained-model-persistence", ({ payload }) => {
        const ctx = current();
        if (payload.kind !== kind || payload.token !== ctx.token || payload.channel !== ctx.c || payload.time !== ctx.t) return;
        const record = payload.persistence?.saved?.record;
        if (record && (record.channel !== ctx.c || record.time !== ctx.t)) return;
        show(payload.persistence);
      });
      return {
        show,
        clear() { container().replaceChildren(); container().hidden = true; },
        async restore(req) {
          try {
            const saved = await invoke("trained_model_restore", { ...req, kind });
            if (!same(req, current())) return { stale: true };
            show({ saved }, true);
            return { saved };
          } catch (e) {
            if (!same(req, current())) return { stale: true };
            failure(e, true);
            return { error: String(e) };
          }
        },
      };
    },
  };
})();
