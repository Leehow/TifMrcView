"use strict";
(() => {
  const W=window.TifMrcViewPro.cryoet, host=W.sections[1];
  host.innerHTML='<div class="al-head"><b>倾转序列 CTF</b></div><p class="dim">复用 CTF 工具：在当前视野逐层拟合，显示每个倾角的离焦。当前步骤估计 CTF，不应用相位翻转。</p><label>计算谱尺寸 <select id="cryo-ctf-size"><option>256</option><option selected>512</option><option>1024</option></select></label><div id="cryo-ctf-host"></div>';
  $("cryo-ctf-size").onchange=()=>{$("fft-n").value=$("cryo-ctf-size").value;};
  W.register(1,{enter(){ $("cryo-ctf-size").value=$("fft-n").value;window.TifMrcViewPro.ctfWorkspace.mount($("cryo-ctf-host"));},leave(){window.TifMrcViewPro.ctfWorkspace.unmount();}});
})();
