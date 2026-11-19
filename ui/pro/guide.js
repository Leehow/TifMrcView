"use strict";
// The step guide at the top of a workspace (single particle, Cryo-ET): one
// thin row of numbered steps (done ✓, now, still locked) and a small 「怎么做」
// switch that opens a few lines on the step: what it does and what to press.
// It is open the first time a workspace is used and then stays as the user
// left it, so it takes one row for people who know the way.
//   const g = TifMrcViewPro.guide(host, "spa", [{ title, help, go }…]);
//   g.set(["done", "now", "locked"]);  // a state per step
// `help` is HTML (fixed text of this app, never data); `go()` runs when the
// step is clicked (open its card or tab). The step at hand (clicked, or set
// by `show(i)`) is underlined like an open tab; until then, the current one. Shares app.js globals: pref.
(() => {
  const check = '<svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>';
  const lock = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></svg>';

  window.TifMrcViewPro.guide = (host, id, steps) => {
    const el = document.createElement("div");
    el.className = "pg";
    const row = document.createElement("div");
    row.className = "pg-row";
    const list = document.createElement("ol");
    list.className = "pg-steps";
    const help = document.createElement("button");
    help.className = "small pg-help";
    help.type = "button";
    help.innerHTML = "怎么做";
    const detail = document.createElement("div");
    detail.className = "pg-detail";
    row.append(list, help);
    el.append(row, detail);
    host.prepend(el);

    let states = steps.map(() => "locked");
    let shown = -1; // the step whose help is shown (-1: the current one)
    const key = `guide-open-${id}`;
    let open = pref.get(key, true);

    const items = steps.map((s, i) => {
      const li = document.createElement("li");
      const b = document.createElement("button");
      b.type = "button";
      b.innerHTML = `<span class="pg-n">${i + 1}</span><span class="pg-t"></span>`;
      b.querySelector(".pg-t").textContent = s.title;
      b.onclick = () => {
        shown = i;
        paint();
        s.go?.();
      };
      li.append(b);
      list.append(li);
      return b;
    });

    function current() {
      const n = states.indexOf("now");
      return n >= 0 ? n : Math.max(0, states.lastIndexOf("done"));
    }

    function paint() {
      const now = current();
      const at = shown >= 0 ? shown : now;
      items.forEach((b, i) => {
        b.className = `pg-${states[i]}${i === at ? " pg-at" : ""}`;
        b.querySelector(".pg-n").innerHTML = states[i] === "done" ? check : states[i] === "locked" ? lock : String(i + 1);
        b.title = states[i] === "done" ? "已完成" : states[i] === "now" ? "现在做这一步" : `先完成第 ${i} 步`;
      });
      help.classList.toggle("on", open);
      help.setAttribute("aria-expanded", String(open));
      help.title = open ? "收起说明" : "每一步做什么、按哪里";
      detail.hidden = !open;
      const s = steps[at];
      const tag = states[at] === "done" ? "已完成" : states[at] === "now" ? "现在做这一步" : `先完成第 ${at} 步`;
      detail.innerHTML = `<b>第 ${at + 1} 步 · ${s.title}</b><span class="pg-tag">${tag}</span><div>${s.help}</div>`;
    }

    help.onclick = () => {
      open = !open;
      pref.set(key, open);
      paint();
    };

    paint();
    return {
      el,
      // A state per step: "done", "now" or "locked" (anything else: a step that can be done).
      set(next) {
        if (next.join() === states.join()) return;
        states = next;
        paint();
      },
      // Shows step `i` as the one at hand (as a click on it does, without `go`).
      show(i) { shown = i; paint(); },
      // The step at hand: the one shown last, else the current one.
      at: () => (shown >= 0 ? shown : current()),
    };
  };
})();
