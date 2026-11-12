"use strict";
// Pure tab rules (no DOM): where an image opens and which tab makes room.
(() => {
  const MAX_TABS = 20;
  window.TmvTabRules = {
    MAX_TABS,
    /** A replace-open shows the new image in the active tab when that is an image tab. */
    replacesActive(active, opts) {
      return !!(opts && opts.replace && active && active.type === "image");
    },
    /** The tab to close so a new one fits: least recently activated image
     * tab that is not the active one and has no unsaved changes; null if none. */
    evictionCandidate(tabs, active) {
      let best = null;
      for (const d of tabs) {
        if (d.type !== "image" || d === active || d.dirty) continue;
        if (!best || (d.used || 0) < (best.used || 0)) best = d;
      }
      return best;
    },
  };
})();
