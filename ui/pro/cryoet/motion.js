"use strict";
// Movie correction of the currently opened movie; one output, no implicit
// processing of other files or changes to the general drift panel.
(() => {
  const W=window.TifMrcViewPro.cryoet, host=W.sections[0];
  let result=null,busy=false;
  host.innerHTML=`<div class="al-head"><b>电影帧运动校正</b></div><p class="dim">对当前打开的电影逐帧校正。局部 patch 运动、EER 与剂量加权将在后续版本加入。</p><label>计算尺寸 <select id="cryo-motion-n"><option>512</option><option selected>1024</option><option>2048</option></select></label><div class="al-row"><button id="cryo-motion-run" class="small primary">计算电影位移</button><button id="cryo-motion-save" class="small" disabled>保存对齐电影并打开…</button></div><p id="cryo-motion-result" class="dim"></p>`;
  $("cryo-motion-run").onclick=async()=>{
    if(busy)return invoke("align_cancel");if(!S.info||S.info.sizeZ<2)return W.status("请打开电影叠层");
    const token=S.token,c=fftChannel(),t=S.t;busy=true;$("cryo-motion-run").textContent="取消";result=null;$("cryo-motion-save").disabled=true;
    const un=await T.event.listen("align-progress",e=>W.status(`电影校正 ${e.payload.done}/${e.payload.total}`));
    try {
      const r=await invoke("align_estimate",{req:{token,c,t,zs:[],region:null,mode:"movie",n:+$("cryo-motion-n").value,maxShift:0}});
      if(token!==S.token)return;
      result={...r,token,c,t};$("cryo-motion-result").textContent=`${r.zs.length} 帧 · ${r.iterations} 次迭代 · ${(r.ms/1000).toFixed(1)} s`;$("cryo-motion-save").disabled=false;W.status("运动校正完成");
    } catch(e){W.status(String(e));}finally{un();busy=false;$("cryo-motion-run").textContent="计算电影位移";}
  };
  $("cryo-motion-save").onclick=async()=>{
    if(!result||result.token!==S.token||busy)return W.status("请先对当前电影计算位移");
    const r=result;const path=await invoke("pick_save_path",{dir:saveDir(),name:S.info.name.replace(/\.[^.]+$/,"")+"_motion.mrc",filter:"MRC",exts:["mrc"]});if(!path||r.token!==S.token)return;
    busy=true;$("cryo-motion-save").disabled=true;setSaveDir(dirname(path));
    const un=await T.event.listen("align-progress",e=>W.status(`保存 ${e.payload.done}/${e.payload.total}`));
    try {await invoke("align_save",{req:{token:r.token,c:r.c,t:r.t,zs:r.zs,shifts:r.shifts,path,what:"stack"}});if(r.token===S.token)await openPath(path);W.status("已打开校正电影；可按需要打开倾转序列继续②");}
    catch(e){W.status(String(e));}finally{un();busy=false;$("cryo-motion-save").disabled=false;}
  };
})();
