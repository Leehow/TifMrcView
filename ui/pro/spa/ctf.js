"use strict";
// Step ②: the CTF of every micrograph (ctf::fit_region on the whole image),
// shown as a table; a row opens that micrograph in step ③.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt, name } = W;
  W.sections[1].innerHTML = `
    <div class="al-head"><b>每张显微图的 CTF</b></div>
    <div class="al-opts">
      <label>电压 kV<input id="spa-kv" type="number" min="60" max="400" value="300"></label>
      <label>Cs mm<input id="spa-cs" type="number" min="0" max="10" step="0.01" value="2.7"></label>
      <label>振幅衬度<input id="spa-amp" type="number" min="0" max="0.5" step="0.01" value="0.1"></label>
      <label>谱尺寸<select id="spa-n"><option>256</option><option selected>512</option><option>1024</option></select></label>
      <label>离焦范围 µm<span class="ct-range"><input id="spa-dfmin" type="number" min="0" step="0.1" value="0.3">–<input id="spa-dfmax" type="number" min="0" step="0.1" value="5"></span></label>
      <label>拟合分辨率 Å<span class="ct-range"><input id="spa-lo" type="number" min="5" value="30">–<input id="spa-hi" type="number" min="1" placeholder="自动"></span></label>
    </div>
    <div class="al-row"><button id="spa-run-ctf" class="small primary">拟合全部显微图</button></div>
    <div id="spa-ctf-table" class="ws-table"></div>`;

  W.register(1, {
    show(p) {
      $("spa-kv").value = p.optics.kv;
      $("spa-cs").value = p.optics.csMm;
      $("spa-amp").value = p.optics.amplitude;
      const t = document.createElement("table");
      t.innerHTML = "<thead><tr><th>显微图</th><th>离焦 µm</th><th>像散 Å</th><th>拟合到 Å</th><th>颗粒</th></tr></thead>";
      const body = document.createElement("tbody");
      p.mics.forEach((m, i) => {
        const row = document.createElement("tr");
        if (m.excluded) row.className = "missing";
        const c = m.ctf;
        for (const x of [name(m.path), c ? fmt((c.df1 + c.df2) / 2e4, 2) : "—", c ? fmt(c.df1 - c.df2, 0) : "—", c ? fmt(c.fitRes) : "—", m.picks || "—"]) {
          const td = document.createElement("td");
          td.textContent = x;
          row.append(td);
        }
        row.onclick = () => { W.mic = i; W.select(2); };
        body.append(row);
      });
      t.append(body);
      const ctf = p.mics.some((m) => m.ctf);
      $("spa-ctf-table").replaceChildren(ctf || p.mics.length <= 200 ? t : "");
    },
  });

  $("spa-run-ctf").onclick = () => {
    const px = +$("spa-pixel").value;
    if (!(px > 0)) return W.status("请先填写像素大小");
    W.run("CTF 拟合", "ctf", {
      n: +$("spa-n").value,
      params: {
        voltageKv: +$("spa-kv").value, csMm: +$("spa-cs").value, ampContrast: +$("spa-amp").value, pixelA: px,
        resLowA: +$("spa-lo").value, resHighA: +$("spa-hi").value || Math.max(4, px * 2.5),
        dfMinA: +$("spa-dfmin").value * 1e4, dfMaxA: +$("spa-dfmax").value * 1e4, maxAstigA: 2000, phase: false,
      },
    });
  };
})();
