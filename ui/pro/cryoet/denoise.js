"use strict";
(() => {
  const W=window.TifMrcViewPro.cryoet,host=W.sections[4];let busy=false,trainedToken=null;
  host.innerHTML=`<div class="al-head"><b>断层去噪 / 去伪影</b></div><p class="dim">在重构体上用相邻 Z 层训练 Noise2Noise（neighbours）。请勿在未重构的倾转序列上训练；相邻层应含相近结构。</p><label>训练步数 <select id="cryo-n2n-steps"><option>500</option><option selected>1000</option><option>2000</option></select></label><div class="al-row"><button id="cryo-n2n-train" class="small primary">训练 Noise2Noise</button><button id="cryo-n2n-save" class="small">保存去噪体并打开…</button></div><p id="cryo-n2n-result" class="dim"></p><button class="small" disabled title="由另一个开发线程实现">IsoNet · 即将推出</button>`;
  $("cryo-n2n-train").onclick=async()=>{
    if(busy)return invoke("denoise_cancel");if(!S.info||S.info.sizeZ<3)return W.status("请先打开重构体");
    const token=S.token;trainedToken=null;busy=true;$("cryo-n2n-train").textContent="取消";
    const un=await T.event.listen("denoise-train",e=>W.status(`训练 ${e.payload.step}/${e.payload.steps} · 损失 ${e.payload.loss.toPrecision(3)}`));
    try {
      const r=await invoke("denoise_train",{req:{token,c:fftChannel(),t:S.t,mode:"neighbours",steps:+$("cryo-n2n-steps").value,slab:0}});
      if(token!==S.token)return;trainedToken=token;$("cryo-n2n-result").textContent=`${r.images} 对 · ${r.backend} · ${r.seconds.toFixed(1)} s · 损失 ${r.loss.toPrecision(3)}`+(r.warning?` · ${r.warning}`:"");W.status("训练完成");
    }catch(e){W.status(String(e));}finally{un();busy=false;$("cryo-n2n-train").textContent="训练 Noise2Noise";}
  };
  $("cryo-n2n-save").onclick=async()=>{
    if(busy||trainedToken!==S.token)return W.status("请先在当前重构体上训练");
    const token=S.token,c=fftChannel(),t=S.t;
    const path=await invoke("pick_save_path",{dir:saveDir(),name:S.info.name.replace(/\.[^.]+$/,"")+"_n2n.mrc",filter:"MRC",exts:["mrc"]});if(!path||token!==S.token)return;
    busy=true;setSaveDir(dirname(path));const un=await T.event.listen("denoise-progress",e=>W.status(`去噪 ${e.payload.done}/${e.payload.total}`));
    try {await invoke("denoise_save",{req:{token,c,t,zs:[],path,params:{method:"learned",strength:1,iterations:10,patch:1,search:7,slab:0,device:"auto"}}});if(token===S.token)await openPath(path);W.status("已打开去噪体");}
    catch(e){W.status(String(e));}finally{un();busy=false;}
  };
})();
