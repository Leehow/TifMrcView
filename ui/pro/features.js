"use strict";
// One table for what TifMrcView Pro can do, read by the window (ui/pro/agent.js: `tmv app`, `tmv mcp`) and checked by scripts/agent-parity.test.mjs. Every tool button of the window (`btn-…`) and every `tmv` command (src/cli.rs) has a row. `app` is the action `tmv app <app>` runs in the window, the user watching; `cli` is the `tmv <cli>` command that does the same without a window, or null when the feature only makes sense on screen (`cliWhy` says why). `ui.button` opens the feature in the window (`alsoButtons`: its own button when it is not inside another panel); `ui.ids` are the controls the action presses. Add a feature on one side, add its row and the other side.
// The table between `=` and the final `;` is plain JSON (the test parses it).
window.TmvFeatures = {
  "features": [
    {"id": "run", "name": "自动判断并处理", "app": "run", "cli": "run", "ui": {"ids": ["btn-open"]}},
    {"id": "tomo", "name": "Cryo-ET 一键重构断层", "app": "tomo", "cli": "tomo", "ui": {"button": "btn-cryoet", "ids": ["cryoet-workspace", "cryo-status", "cryo-cancel"]}},
    {"id": "sta", "name": "子断层平均（找候选卡片，由用户选）", "app": "sta", "cli": "sta", "ui": {"button": "btn-cryoet", "alsoButtons": ["btn-sta"], "ids": ["cryo-sta", "sta-workspace", "sta-run", "sta-again", "sta-cards"]}},
    {"id": "spa", "name": "单颗粒重构（一键）", "app": "spa", "cli": "spa", "ui": {"button": "btn-spa", "ids": ["spa-workspace", "spa-a-start", "spa-a-kinds", "spa-a-go", "spa-status"]}},
    {"id": "ctf", "name": "显微图的 CTF / 离焦", "app": "ctf", "cli": "ctf", "ui": {"button": "btn-spa", "ids": ["spa-advanced", "spa-run-ctf"]}},
    {"id": "membranes", "name": "膜、囊泡、微管分割", "app": "membranes", "cli": "membranes", "ui": {"ids": ["sec-labels", "lab-mem", "lab-mem-run"]}},
    {"id": "denoise", "name": "降噪", "app": "denoise", "cli": "denoise", "ui": {"button": "btn-denoise", "ids": ["denoise-panel", "dn-train", "dn-save-all"]}},
    {"id": "info", "name": "版本、显卡和模型", "app": "info", "cli": "info", "ui": {"ids": ["btn-settings"]}},
    {"id": "setup", "name": "准备模型（MemBrain-seg、SAM）", "app": "setup", "cli": "setup", "ui": {"ids": ["lab-sam-get"]}},
    {"id": "look", "name": "看清一层（带坐标网格的截图，给 agent 读坐标）", "app": "look", "cli": "look", "ui": {"ids": ["view"]}},
    {"id": "label-marks", "name": "按坐标做标签图层（点、轮廓、线 → mask）", "app": "labels", "cli": "labels", "ui": {"ids": ["sec-labels"]}},
    {"id": "labels", "name": "标签图层（画、改名、显示/隐藏）", "app": "label_set", "cli": null, "cliWhy": "画标签要看着图；tmv sta --marks / --points 读保存下来的标记", "ui": {"ids": ["sec-labels"]}},
    {"id": "layers", "name": "图层面板", "app": "workspace", "cli": null, "cliWhy": "只是显示", "ui": {"button": "btn-show-layers"}},
    {"id": "align", "name": "漂移校正（图像对齐）", "app": "workspace", "cli": null, "cliWhy": "tmv tomo / tmv spa 里自动做", "ui": {"button": "btn-align", "ids": ["al-run"]}},
    {"id": "wedge", "name": "缺失楔补偿", "app": "workspace", "cli": null, "cliWhy": "tmv tomo --for display 里自动做", "ui": {"button": "btn-wedge", "ids": ["wg-train"]}},
    {"id": "volume", "name": "三维显示", "app": "workspace", "cli": null, "cliWhy": "只是显示", "ui": {"button": "btn-3d"}},
    {"id": "figure", "name": "一键出图（论文三维图）", "app": "workspace", "cli": null, "cliWhy": "要看着结构挑颜色和角度", "ui": {"button": "btn-figure"}},
    {"id": "diffraction", "name": "衍射标定", "app": "workspace", "cli": null, "cliWhy": "要在图上点衍射点", "ui": {"button": "btn-diff"}},
    {"id": "stem4d", "name": "4D-STEM", "app": "workspace", "cli": null, "cliWhy": "交互式虚拟探测器", "ui": {"button": "btn-4d"}},
    {"id": "particles", "name": "颗粒统计", "app": "workspace", "cli": null, "cliWhy": "要在图上框选", "ui": {"button": "btn-part"}}
  ]
};
