"use strict";
// A dedicated Cryo-ET workspace; the ordinary Pro tools keep their entry points.
(() => {
  const button=document.createElement("button"); button.id="btn-cryoet"; button.className="toggle";
  button.title="Cryo-ET 工作区";
  button.innerHTML='<svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9" ry="4" transform="rotate(-35 12 12)"/><ellipse cx="12" cy="12" rx="9" ry="4" transform="rotate(35 12 12)"/><path d="M12 2v20M2 12h20"/></svg><span>Cryo-ET</span>';
  $("quickbar").append(button);
  const panel=document.createElement("section");panel.id="cryoet-workspace";panel.hidden=true;
  panel.innerHTML='<div class="al-head"><b>Cryo-ET · 冷冻电子断层</b><button id="cryo-close" class="small">✕</button></div><nav id="cryo-steps" aria-label="Cryo-ET 步骤"></nav><div id="cryo-status" role="status" class="dim"></div><div id="cryo-body"></div>';
  $("stage").append(panel);
  const steps=["① 运动校正","② CTF","③ 倾转对齐","④ 重构","⑤ 去噪/去伪影"];
  const sections=steps.map((name,i)=>{
    const b=document.createElement("button");b.textContent=name;b.onclick=()=>select(i);$("cryo-steps").append(b);
    const s=document.createElement("section");s.id=`cryo-step-${i}`;s.hidden=true;$("cryo-body").append(s);return s;
  });
  let current=2;
  const hooks=new Map();
  function select(i) {
    hooks.get(current)?.leave?.();current=i;
    sections.forEach((s,j)=>{s.hidden=i!==j;$("cryo-steps").children[j].classList.toggle("on",i===j);$("cryo-steps").children[j].setAttribute("aria-current",i===j?"step":"false");});
    if(!panel.hidden)hooks.get(i)?.enter?.();
  }
  function close() {hooks.get(current)?.leave?.();panel.hidden=true;button.classList.remove("on");}
  button.onclick=()=>{
    if(!panel.hidden)return close();
    if(F.on)setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel",{detail:panel.id}));
    panel.hidden=false;button.classList.add("on");select(current);
  };
  $("cryo-close").onclick=close;
  document.addEventListener("pro-panel",e=>{if(e.detail!==panel.id&&!panel.hidden)close();});
  $("btn-fft").addEventListener("click",()=>{if(F.on)close();});
  window.TifMrcViewPro.cryoet={sections,panel,select,active:()=>current,register:(i,h)=>hooks.set(i,h),status:s=>{$("cryo-status").textContent=s;}};
  select(current);
  setInterval(()=>{button.disabled=false;},400);
})();
