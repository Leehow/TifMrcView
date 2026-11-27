/* Actual local execution events, scoped to one invocation and one window. */
'use strict';
(() => {
  const kinds = new Map();
  for (const name of ['tomo_align','tomo_ctf','tomo_reconstruct','tomo_denoise','tomo_auto','tomo_wedge','tomo_session_run','tomo_save_aligned','tomo_export']) kinds.set(name,'tomo');
  for (const [name,kind] of Object.entries({spa_run:'spa',denoise_train:'denoise',denoise_preview:'denoise',denoise_save:'denoise',ctf_fit:'ctf',ctf_fit_stack:'ctf',motion_run:'tomo',motion_series:'tomo',align_estimate:'tomo'})) kinds.set(name,kind);
  const jobs = new Map(), latest = new Map();
  let listening, resources, resourcesAt = 0;
  const node = (tag,text) => { const e=document.createElement(tag); if(text!=null)e.textContent=text; return e; };
  const refresh = () => {
    if(typeof S!=="undefined" && S.folder?.remote)return;
    for(const [id,kind] of [['cryoet-workspace','tomo'],['spa-workspace','spa'],['denoise-panel','denoise'],['ctf-box','ctf'],['motion-workspace','motion']]) {
      const panel=document.getElementById(id);if(!panel)continue;
      let bar=panel.querySelector('.remote-processing-location');
      if(!bar){bar=node('div');bar.className='remote-processing-location';const head=panel.querySelector('.al-head,.ctf-head');if(head)head.after(bar);else panel.prepend(bar);}
      render(bar,kind);
    }
  };
  function render(bar,kind) {
    const expanded=new Map([...bar.querySelectorAll('details')].map(d=>[d.className,d.open]));
    const record=latest.get(kind);bar.replaceChildren();
    bar.append(node('strong','运行位置：本机'+(record?.running?' · 正在处理':record?.error?' · 处理失败':record?' · 已完成':'')));
    // 停止, at the right of the 运行位置 line: what was done is kept (the series' cards offer 继续).
    const cancel={tomo:'tomo_cancel',spa:'spa_cancel'}[kind];
    if(record?.running&&cancel){const stop=node('button',record.stopping?'正在停止…':'停止');stop.className='small remote-stop';stop.disabled=!!record.stopping;stop.title='停下来；做完的步骤都保留，以后可以接着做';stop.onclick=()=>{record.stopping=true;render(bar,kind);window.__TAURI__.core.invoke(cancel).catch(()=>{});};bar.append(stop);}
    if(resources) {
      let text=`CPU ${resources.cpuThreads} 线程 · 当前可回收内存 ${(resources.memoryAvailableMb/1024).toFixed(1)} GB`;
      if(resources.metal){const m=resources.metal;text+=` · ${m.name} · ${m.unified?'统一内存':'显存'}新任务预算 ${(m.availableBytes/2**30).toFixed(1)} GB（已预留系统空间）`;}
      bar.append(node('p',text));
      // Several NVIDIA cards here: which the Cryo-ET runs use.
      if(kind==='tomo'&&resources.gpuChoice&&resources.gpus?.length>1&&window.TmvGpuPick){for(const g of resources.gpus)bar.append(TmvGpuPick.row(g,resources.gpus,'local',!!record?.running,refresh));bar.append(TmvGpuPick.note(resources.gpus,'local'));}
    }
    if(record?.error)bar.append(node('p',record.error));
    const id=kind==='tomo'?'cryoet-workspace':kind==='spa'?'spa-workspace':'';
    const advanced=window.TmvHelp?.advanced(id);
    // 结果目录 (Cryo-ET): results go into the project folder's step folders; the disks and their free space are shown.
    if(advanced&&kind==='tomo'){
      const folder=window.TifMrcViewPro?.cryoet?.reconFolder?.();
      if(Date.now()-resourcesAt>15000||resources?.for!==folder){resourcesAt=Date.now();window.__TAURI__.core.invoke('local_compute_resources',{output:folder||null}).then(r=>{resources={...r,for:folder};refresh();}).catch(()=>{});}
      const size=mb=>mb>=1048576?`${(mb/1048576).toFixed(1)} TB`:`${(mb/1024).toFixed(mb<10240?1:0)} GB`;
      const out=node('details');out.className='remote-output-details';out.append(node('summary','结果目录'));
      out.append(node('p',folder?`结果保存在项目文件夹「${folder.split(/[\\/]/).filter(Boolean).pop()}」里（1_motion、2_tomograms、3_clean、4_sta 等子文件夹），原始数据不动。`:'结果保存在项目文件夹里（1_motion、2_tomograms、3_clean、4_sta 等子文件夹），原始数据不动。'));
      const free=resources?.for===folder?resources?.outputFreeMb:null;
      if(free!=null){const f=node('p',`项目文件夹所在的盘还剩 ${size(free)}${free<50*1024?'：空间不多，一个序列的中间结果通常要几 GB 到几十 GB，建议把数据放到空间大的盘上':''}`);f.className=free<50*1024?'ssh-pick-low':'dim';out.append(f);}
      if(resources?.disks?.length){const l=node('p','这台电脑的盘：'+resources.disks.map(d=>`${d.mount} 剩 ${size(d.freeMb)} / 共 ${size(d.totalMb)}`).join('；'));l.className='dim';out.append(l);}
      if(folder){const b=node('button','在文件夹中打开');b.className='small';b.onclick=()=>window.__TAURI__.core.invoke('open_folder',{path:folder}).catch(()=>{});out.append(b);}
      advanced.append(out);
    }
    const done=()=>{if(advanced){window.TmvHelp.finish(advanced,id);if(advanced.childElementCount>1)bar.append(advanced);}for(const d of bar.querySelectorAll('details'))if(expanded.has(d.className))d.open=expanded.get(d.className);};
    if(!record?.placements.size){if(record?.running)bar.append(node('span','正在准备数据；计算步骤开始后显示实际设备…'));return done();}
    const current=record.current;if(current){const status=node('p',`${current.operation}：${current.backend}${current.device?' · '+current.device:''}${current.reason?' · '+current.reason:''}`);status.setAttribute('role','status');bar.append(status);}
    const details=node('details');details.className='remote-backends';
    details.append(node('summary',`${record.running?'每一步实际计算设备':'本次任务的计算设备'} · ${record.placements.size} 个步骤`));
    for(const p of record.placements.values()){
      const row=node('div');row.className='remote-backend-row';row.classList.toggle('fallback',!!p.fallback);
      row.append(node('strong',p.operation),node('span',p.backend+(p.device?' · '+p.device:'')));
      if(p.usedBackends?.length>1)row.append(node('small','本步骤实际使用过：'+p.usedBackends.join('；')));
      if(p.reason)row.append(node('small',(p.fallback?'CPU 回退原因：':p.backend==='CPU'?'CPU 分工：':'分工：')+p.reason));
      if(p.priorFallback&&!p.fallback)row.append(node('small','本次此前曾回退 CPU：'+p.priorFallback));
      details.append(row);
    }
    (advanced||bar).append(details);
    done();
  }
  async function invoke(command,args,native) {
    if(!kinds.has(command)||!window.TifMrcViewPro)return native(command,args);
    listening ||= window.__TAURI__.event.listen('local-compute',({payload})=>{
      const record=jobs.get(payload.id);if(!record)return;
      const p={...payload.placement},previous=record.placements.get(p.operation);
      p.priorFallback=previous?.priorFallback||(previous?.fallback?previous.reason:null);
      p.usedBackends=[...new Set([...(previous?.usedBackends||[]),p.backend+(p.device?' · '+p.device:'')])];
      record.placements.set(p.operation,p);record.current=p;
      if(p.fallback && previous?.reason!==p.reason && typeof toast==='function')toast(`${p.operation}已回退 CPU：${p.reason}`);
      refresh();
    }).catch(e=>{listening=null;throw e;});
    await listening;
    const id=crypto.randomUUID(),kind=kinds.get(command),record={running:true,placements:new Map()};
    jobs.set(id,record);latest.set(kind,record);refresh();
    if(Date.now()-resourcesAt>15000){resourcesAt=Date.now();native('local_compute_resources',{output:window.TifMrcViewPro?.cryoet?.reconFolder?.()||null}).then(r=>{resources={...r,for:window.TifMrcViewPro?.cryoet?.reconFolder?.()};refresh();}).catch(()=>{});}
    try{return await native(command,{...args,computeId:id});}
    catch(e){record.error=String(e);throw e;}
    finally{record.running=false;while(jobs.size>100){const oldest=[...jobs].find(([_,r])=>!r.running);if(!oldest)break;jobs.delete(oldest[0]);}refresh();}
  }
  window.TmvLocal={invoke,render};
  document.addEventListener('pro-panel',refresh);
})();
