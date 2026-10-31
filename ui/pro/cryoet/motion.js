"use strict";
// Native movie/EER motion correction and mdoc acquisition-order batches.
(() => {
  const W=window.TifMrcViewPro.cryoet,host=W.sections[0];
  const M={busy:false,movie:null,mdoc:null,folder:null,gain:null,result:null};
  host.innerHTML=`
    <div class="al-head"><b>束流诱导运动校正</b></div>
    <div class="al-opts">
      <label class="al-wide">输入来源<select id="mo-source"><option value="current">当前打开的电影</option><option value="file">选择电影 / 原生 EER</option><option value="mdoc">mdoc 倾转批次</option></select></label>
      <div class="al-wide al-row"><button id="mo-movie" class="small">选择电影…</button><span id="mo-movie-name" class="dim"></span></div>
      <div class="al-wide al-row"><button id="mo-mdoc" class="small">选择 / 关联 mdoc…</button><span id="mo-mdoc-name" class="dim"></span></div>
      <div class="al-wide al-row"><button id="mo-folder" class="small">电影搜索文件夹…</button><span id="mo-folder-name" class="dim">自动搜索 mdoc 同级、上级的 eer / frames</span></div>
      <label>每输出帧 fraction 数<input id="mo-group" type="number" min="0" max="256" value="0" title="0：EER 自动分为约 40 帧；普通电影不分组"></label>
      <label>EER 渲染<select id="mo-up"><option value="1">1×（4K）</option><option value="2">2×（8K 超分辨率）</option></select></label>
      <label>普通电影帧轴<select id="mo-axis"><option value="auto">自动 Z / T</option><option value="z">Z</option><option value="t">T</option></select></label>
      <label>计算短边尺寸<select id="mo-working"><option>512</option><option selected>1024</option><option>2048</option></select></label>
      <label>局部网格 P×P<input id="mo-patches" type="number" min="2" max="10" value="5"></label>
      <label class="al-check"><input id="mo-local" type="checkbox" checked>拟合局部形变</label>
      <div class="al-wide al-row"><button id="mo-gain" class="small">增益参考（可选）…</button><button id="mo-clear-gain" class="small">清除</button><span id="mo-gain-name" class="dim">无 gain</span></div>
      <label>Gain 旋转（顺时针）<select id="mo-rotate"><option value="0">0°</option><option value="1">90°</option><option value="2">180°</option><option value="3">270°</option></select></label>
      <div><label class="al-check"><input id="mo-flip-x" type="checkbox">Gain 左右翻转</label><label class="al-check"><input id="mo-flip-y" type="checkbox">Gain 上下翻转</label></div>
      <label>剂量来源<select id="mo-dose-mode"><option value="auto">自动 mdoc ExposureDose</option><option value="total">每倾角电影总剂量</option><option value="frame">每分组帧剂量</option><option value="none">不加权</option></select></label>
      <label>剂量 e⁻/Å²<input id="mo-dose" type="number" min="0" step="0.1" placeholder="自动" disabled></label>
      <label>原始 1× 像素 Å<input id="mo-pixel" type="number" min="0" step="0.01" placeholder="图像 / mdoc 自动"></label>
      <label>电压 kV<select id="mo-kv"><option value="auto" selected>mdoc 自动（默认 300）</option><option>300</option><option>200</option></select></label>
      <label>起始前曝光 e⁻/Å²<input id="mo-pre" type="number" min="0" step="0.1" value="0"></label>
      <label>计算<select id="mo-backend"><option value="auto">实测择优（CPU / GPU）</option><option value="gpu">GPU</option><option value="cpu">CPU 参考</option></select></label>
      <label class="al-check al-wide"><input id="mo-unweighted" type="checkbox" checked>同时写无剂量加权总和</label>
      <label class="al-check al-wide"><input id="mo-halves" type="checkbox" checked>写偶 / 奇分组帧总和（0,2… / 1,3…）</label>
    </div>
    <p class="dim">Gain 为乘法参考。剂量缺失或为 0 时保留无加权输出并提示；批次前曝光按采集时间累计，最终叠层按倾角排序。局部模型为 x/y 二次 × 时间三次。</p>
    <div class="al-row"><button id="mo-run" class="small primary">校正并保存…</button><button id="mo-open" class="small" disabled>打开校正总和</button></div>
    <p id="mo-summary" class="dim"></p>
    <canvas id="mo-trajectory" width="700" height="200" hidden title="全局位移：蓝 dx，红 dy，像素"></canvas>
    <canvas id="mo-vectors" width="700" height="220" hidden title="每块首末帧的拟合局部位移差"></canvas>
    <p id="mo-vectors-label" class="dim"></p><div id="mo-table"></div>
    <button id="mo-next" class="small primary" disabled title="mdoc 批次输出可继续倾转对齐">下一步：倾转对齐</button>`;
  const name=p=>p?.split(/[\\/]/).pop()||"";
  async function pick(kind) {return invoke("motion_pick",{dir:saveDir(),kind});}
  $("mo-movie").onclick=async()=>{const p=await pick("movie");if(p){M.movie=p;$("mo-movie-name").textContent=name(p);$("mo-source").value="file";}};
  $("mo-mdoc").onclick=async()=>{const p=await pick("mdoc");if(p){M.mdoc=p;$("mo-mdoc-name").textContent=name(p);}};
  $("mo-folder").onclick=async()=>{const p=await invoke("pick_folder",{dir:saveDir()});if(p){M.folder=p;$("mo-folder-name").textContent=p;}};
  $("mo-gain").onclick=async()=>{const p=await pick("gain");if(p){M.gain=p;$("mo-gain-name").textContent=name(p);}};
  $("mo-clear-gain").onclick=()=>{M.gain=null;$("mo-gain-name").textContent="无 gain";};
  $("mo-dose-mode").onchange=()=>{$("mo-dose").disabled=!["total","frame"].includes($("mo-dose-mode").value);};
  function params() {
    const mode=$("mo-dose-mode").value,dose=+$("mo-dose").value;
    if(["total","frame"].includes(mode)&&!(dose>0))throw Error("请输入正的剂量值");
    return {group:+$("mo-group").value,upsample:+$("mo-up").value,frameAxis:$("mo-axis").value,workingSize:+$("mo-working").value,local:$("mo-local").checked,patches:+$("mo-patches").value,doseWeighted:mode!=="none",unweighted:$("mo-unweighted").checked,evenOdd:$("mo-halves").checked,pixelA:+$("mo-pixel").value||null,voltageKv:+$("mo-kv").value||300,voltageAuto:$("mo-kv").value==="auto",dosePerFrame:mode==="frame"?dose:null,totalDose:mode==="total"?dose:null,preExposure:+$("mo-pre").value,gainPath:M.gain,gainRotate:+$("mo-rotate").value,gainFlipX:$("mo-flip-x").checked,gainFlipY:$("mo-flip-y").checked,backend:$("mo-backend").value,benchmark:false,diagnostics:false};
  }
  function busy(on) {
    M.busy=on;$("mo-run").textContent=on?"取消":"校正并保存…";
    host.querySelectorAll("input,select,button").forEach(el=>{if(el.id!=="mo-run")el.disabled=on;});
    $("cryo-steps").querySelectorAll("button").forEach(el=>el.disabled=on);
    if(!on){$("mo-dose-mode").onchange();$("mo-open").disabled=!M.result?.path;$("mo-next").disabled=!M.result?.records||!M.result.path;}
  }
  $("mo-run").onclick=async()=>{
    if(M.busy)return invoke("motion_cancel");
    try {
      const source=$("mo-source").value,p=params();if(source==="current"&&!S.info)throw Error("请先打开电影，或选择原生 EER / mdoc");
      if(source==="file"&&!M.movie)throw Error("请先选择电影");if(source==="mdoc"&&!M.mdoc)throw Error("请先选择 mdoc");
      const token=S.token;const stem=name(source==="mdoc"?M.mdoc:source==="file"?M.movie:S.info.name).replace(/\.[^.]+$/,"");
      const path=await invoke("pick_save_path",{dir:saveDir(),name:`${stem}_motion.mrc`,filter:"MRC",exts:["mrc"]});if(!path)return;if(source==="current"&&token!==S.token)return;
      setSaveDir(dirname(path));busy(true);M.result=null;let un;
      try {
        un=await T.event.listen("motion-progress",e=>{const p=e.payload;W.status(`${p.stage} ${p.done}/${p.total}`);});
        const r=source==="mdoc"?await invoke("motion_series",{req:{mdoc:M.mdoc,folder:M.folder,output:path,params:p}}):await invoke("motion_run",{req:{token:source==="current"?token:null,path:source==="file"?M.movie:null,mdoc:M.mdoc,output:path,c:source==="current"?fftChannel():0,z:source==="current"?S.z:0,t:source==="current"?S.t:0,params:p}});
        M.result=r;
        if(r.records){showSeries(r);}else{showMovie(r);$("mo-table").replaceChildren();}
        W.status(r.path?"运动校正完成":"没有可用电影，未写空叠层");
      }finally{un?.();busy(false);}
    }catch(e){W.status(`运动校正失败：${e}`);}
  };
  function showMovie(r) {
    $("mo-summary").textContent=`${r.rawFractions} fractions → ${r.frames} 帧 · 解码 ${(r.decodeMs/1000).toFixed(1)} s · 准备 ${(r.prepareMs/1000).toFixed(1)} s · 对齐 ${(r.alignmentMs/1000).toFixed(1)} s · 总计 ${(r.totalMs/1000).toFixed(1)} s · ${r.backend} · 总轨迹长度 ${r.totalDrift.toFixed(2)} px · 平均局部运动 ${r.meanLocalMotion.toFixed(2)} px`+(r.warnings.length?` · ${r.warnings.join("；")}`:"");
    drawTrajectory(r.global);drawVectors(r);
  }
  function showSeries(r) {
    const good=r.records.filter(v=>v.result),skipped=r.records.length-good.length;
    $("mo-summary").textContent=`${good.length} 个倾角已输出，${skipped} 个跳过 · 已按倾角排序`+(r.warnings.length?` · ${r.warnings.join("；")}`:"");
    const table=document.createElement("table");table.innerHTML="<thead><tr><th>ZValue</th><th>倾角</th><th>前曝光</th><th>电影 / 状态</th></tr></thead>";
    const body=document.createElement("tbody");
    r.records.forEach(v=>{const row=document.createElement("tr");if(v.skipped)row.className="missing";for(const value of [v.z,v.angle.toFixed(2),v.preExposure.toFixed(2),`${name(v.movie)} · ${v.skipped||"完成"}`]){const td=document.createElement("td");td.textContent=value;row.append(td);}if(v.result)row.onclick=()=>{drawTrajectory(v.result.global);drawVectors(v.result);};body.append(row);});
    table.append(body);$("mo-table").replaceChildren(table);if(good.length){drawTrajectory(good[0].result.global);drawVectors(good[0].result);}
  }
  function drawTrajectory(shifts) {
    const cv=$("mo-trajectory");cv.hidden=false;const ctx=cv.getContext("2d"),w=cv.width,h=cv.height;ctx.clearRect(0,0,w,h);if(!shifts.length)return;
    const lo=Math.min(...shifts.flat()),hi=Math.max(...shifts.flat());
    for(const [k,color] of [[0,"#0969da"],[1,"#cf222e"]]){ctx.strokeStyle=color;ctx.beginPath();shifts.forEach((s,i)=>{const x=35+i/Math.max(1,shifts.length-1)*(w-45),y=15+(hi-s[k])/Math.max(1,hi-lo)*(h-40);(i?ctx.lineTo:ctx.moveTo).call(ctx,x,y);});ctx.stroke();}
    ctx.fillStyle="#888";ctx.font="16px sans-serif";ctx.fillText(`${hi.toFixed(1)} px`,3,15);ctx.fillText(`${lo.toFixed(1)} px`,3,h-12);ctx.fillText("帧 · 蓝 dx / 红 dy",w/2-80,h-3);
  }
  function drawVectors(r) {
    const cv=$("mo-vectors"),ctx=cv.getContext("2d");cv.hidden=false;ctx.clearRect(0,0,cv.width,cv.height);
    const patches=r.local.patches,max=Math.max(.1,...patches.map(p=>Math.hypot(...p.vector))),scale=25/max;
    for(const p of patches){const x=15+p.centre[0]/r.width*(cv.width-30),y=15+p.centre[1]/r.height*(cv.height-30),dx=p.vector[0]*scale,dy=p.vector[1]*scale;ctx.strokeStyle="#cf222e";ctx.fillStyle="#0969da";ctx.beginPath();ctx.arc(x,y,3,0,Math.PI*2);ctx.fill();ctx.beginPath();ctx.moveTo(x,y);ctx.lineTo(x+dx,y+dy);const a=Math.atan2(dy,dx);ctx.lineTo(x+dx-6*Math.cos(a-.4),y+dy-6*Math.sin(a-.4));ctx.moveTo(x+dx,y+dy);ctx.lineTo(x+dx-6*Math.cos(a+.4),y+dy-6*Math.sin(a+.4));ctx.stroke();}
    $("mo-vectors-label").textContent=patches.length?`局部首末帧差，箭长放大 ${scale.toFixed(1)}× · ${r.local.backend} · 剔除 ${r.local.rejected} 个异常测量` : "局部校正已关闭";
  }
  $("mo-open").onclick=async()=>{if(M.result?.path&&!M.busy)await openPath(M.result.path);};
  $("mo-next").onclick=async()=>{if(!M.result?.records||!M.result.path||M.busy)return;await openPath(M.result.path);W.select(2);if(M.result.angles.length<3)W.status("已打开稀疏叠层；倾转对齐至少需要 3 个可用倾角");};
})();
