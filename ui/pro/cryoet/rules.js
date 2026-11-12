"use strict";
// Pure rules of the folder flow (no DOM, no backend): the words and states
// the wall and the review show. Tested in scripts/cryoet-rules.test.mjs.
(() => {
  const R = {};

  /** Stage chip of a series: {text, tone} (tone: idle | run | ok). */
  R.stageChip = (s) => {
    switch (s.stage) {
      case "movies":
        if (s.movies > 0 && s.corrected > 0) return { text: `运动校正 ${s.corrected}/${s.movies}`, tone: "run" };
        return { text: "待处理", tone: "idle" };
      case "stack": return { text: "已成序列", tone: "run" };
      case "aligned": return { text: "已对齐", tone: "run" };
      case "quick": return { text: "可预览", tone: "ok" };
      case "done": return { text: "已完成", tone: "ok" };
      default: return { text: "待处理", tone: "idle" };
    }
  };

  /** Verdict line of a series: {text, tone} (tone: ok | warn | bad), or null before it is judged. */
  R.verdictLine = (s) => {
    const v = s.verdict;
    if (!v) return null;
    const why = v.reason ? `：${v.reason}` : "";
    if (!v.usable) return { text: `✕ 不建议用${why}`, tone: "bad" };
    if (v.attention) return { text: `⚠ 建议看一下${why}`, tone: "warn" };
    return { text: "✓ 可用", tone: "ok" };
  };

  /** Whether a series should catch the eye first. */
  R.needsAttention = (s) => !!s.error || !!s.verdict && (!s.verdict.usable || !!s.verdict.attention);

  /** Series with something to look at first, each group by name. */
  R.sortSeries = (series) => [...series].sort((a, b) =>
    Number(R.needsAttention(b)) - Number(R.needsAttention(a)) || a.name.localeCompare(b.name, undefined, { numeric: true }));

  /** "去掉 N/M 张" or "" when nothing is left out. */
  R.leftOutText = (s) => (s.leftOut > 0 && s.views > 0 ? `去掉 ${s.leftOut}/${s.views} 张` : "");

  /** "残差 0.62 px · 好" or "". */
  R.residualText = (s) =>
    s.residual == null ? "" : `残差 ${s.residual.toFixed(2)} px${s.quality ? ` · ${s.quality}` : ""}`;

  /** The user's choice for view z (true, false or null: the program's), from the dataset's picks. */
  R.pickOf = (picks, z) => {
    const p = picks?.[z];
    return p === true || p === false ? p : null;
  };

  /**
   * State of one view: used | suggested (program would drop it) | dropped
   * (user dropped it) | kept (user kept it against the suggestion), and
   * whether it counts.
   */
  R.viewState = (check, picks) => {
    const pick = R.pickOf(picks, check.z);
    if (pick === false) return { state: "dropped", used: false, pick };
    if (pick === true) return { state: check.bad ? "kept" : "used", used: true, pick };
    return check.bad ? { state: "suggested", used: false, pick } : { state: "used", used: true, pick };
  };

  /** Views that count, and the summary line "共 M 张，用 K 张". */
  R.usedViews = (checks, picks) => checks.filter((c) => R.viewState(c, picks).used);
  R.summaryLine = (checks, picks) => `共 ${checks.length} 张，用 ${R.usedViews(checks, picks).length} 张`;

  /** The keep value a click on a view sends: a view that counts is dropped, one that does not is kept. */
  R.toggleKeep = (check, picks) => !R.viewState(check, picks).used;

  /** Label of a view's state. */
  R.stateLabel = (check, st) => ({
    used: "使用",
    suggested: `建议去掉${check.bad ? `：${check.bad}` : ""}`,
    dropped: "你去掉了",
    kept: `你保留了（程序建议去掉${check.bad ? `：${check.bad}` : ""}）`,
  })[st.state];

  /** Frames of the aligned stack (excluded views are not in it) with the alignment's views by angle. */
  R.alignedViews = (alignment) =>
    (alignment?.views || []).filter((v) => !v.excluded).sort((a, b) => a.angle - b.angle);

  /** Largest residual among the views (at least a small floor, for the bar scale). */
  R.residualScale = (views) => Math.max(0.5, ...views.map((v) => v.residual || 0));

  /** Series names the 开始重构 button works on: the ticked ones. */
  R.chosenNames = (series) => series.filter((s) => s.chosen).map((s) => s.name);

  /** Path and label of the tomogram the third tab shows. */
  R.tomogramOf = (s) => (s.result ? { path: s.result, label: "最终断层图" } : s.quick ? { path: s.quick, label: "快速断层图" } : null);

  /** Decodes the picture format of live_picture / tomo_view: {w, h, title, rgba} or null. */
  R.decodePicture = (buf) => {
    if (!buf || buf.byteLength < 12) return null;
    const dv = new DataView(buf);
    const w = dv.getUint32(0, true), h = dv.getUint32(4, true), n = dv.getUint32(8, true);
    const at = 12 + Math.ceil(n / 4) * 4;
    if (!w || !h || buf.byteLength < at + w * h * 4) return null;
    return { w, h, title: new TextDecoder().decode(new Uint8Array(buf, 12, n)), rgba: new Uint8ClampedArray(buf, at, w * h * 4) };
  };

  /** Section count from a title like "z 12 / 64", else null. */
  R.depthFromTitle = (title) => {
    const m = /(\d+)\s*\/\s*(\d+)/.exec(title || "");
    return m ? +m[2] : null;
  };

  /** Signature that changes when a series got further (to drop cached pictures). */
  R.signature = (s) => [s.stage, s.corrected, s.views, s.leftOut, s.take, !!s.small, !!s.aligned, !!s.quick, !!s.result].join("|");

  window.TmvCryoRules = R;
})();
