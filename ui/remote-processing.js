// The 「高级」 fold under 运行位置 of the Cryo-ET and SPA workspaces (local and SSH alike): the devices
// each step ran on, the results folder and, in Cryo-ET, a 「帮助」 fold saying how to go from movies
// to a tomogram and from tomograms to an average. Built here and used by local-processing.js too.
(() => {
  const el = (tag, text, cls) => { const e = document.createElement(tag); if (text != null) e.textContent = text; if (cls) e.className = cls; return e; };
  const RECON = [
    "点「选文件夹…」，选放着电影（EER / TIFF）和 .mdoc 的项目文件夹；每个倾转序列在下面占一格。",
    "点一格可以看这个序列的图像、对齐和断层图；不好的倾角程序会标出来，可以去掉或改回。",
    "勾上要重构的序列。需要的话勾「精对齐」「N2N 降噪」「去缺失楔」「自动展平」，都不勾最快。",
    "按「重构勾选序列」。做好的断层图出现在右侧栏，也会列在「子断层平均」里。",
  ];
  const STA = [
    "切到「子断层平均」：上面列出三维重构做出的断层图。勾上的参加平均；点图片，在主界面打开它的清晰版。",
    "在清晰版的 XY 截面上给 2–3 个例子：「涂例子」在你要找的东西上涂满（按住 Alt 擦掉），什么东西都行；膜用「膜」涂一笔，丝状物用「线」，囊泡用「球」。",
    "按「找出全部同类」：程序在所有勾选的断层图里找，找到的给你看。拖「少 / 多」，点掉错的、点上漏的，或「再找一轮」。",
    "确认后按平均。结果显示三个截面和分辨率，可以在查看器里打开。也可以先按「找候选」，从程序做的卡片里挑一张。",
  ];
  function steps(title, list) {
    const box = el("div", null, "ws-help-part");
    box.append(el("b", title));
    const ol = el("ol");
    for (const t of list) ol.append(el("li", t));
    box.append(ol);
    return box;
  }
  window.TmvHelp = {
    advanced() {
      const d = el("details", null, "ws-advanced");
      d.append(el("summary", "高级"));
      return d;
    },
    // The help fold goes last, and only in the Cryo-ET workspace.
    finish(advanced, id) {
      if (id !== "cryoet-workspace") return;
      const h = el("details", null, "ws-help");
      h.append(el("summary", "帮助：怎么做三维重构和子断层平均"), steps("三维重构", RECON), steps("子断层平均", STA));
      advanced.append(h);
    },
  };
})();
"use strict";
// 约剩 X 分钟 for a running stage, from its own pace since it began (a stage is its
// text without the numbers, so "运动校正 3/61 · +10°" and "运动校正 4/61 · +12°" are one).
window.TmvEta ||= () => { let key = null, at = 0, d0 = 0;
  return (p) => { if (!p || !(p.total > 1)) return '';
    const k = String(p.stage || '').replace(/[\d.+\-−°]+/g, '') + '|' + p.total, now = Date.now();
    if (k !== key || p.done < d0) { key = k; at = now; d0 = p.done; return ''; }
    const n = p.done - d0, t = now - at; if (n < 1 || t < 5000) return '';
    const s = (p.total - p.done) * t / n / 1000;
    return s < 60 ? '约剩不到 1 分钟' : s < 3600 ? `约剩 ${Math.round(s / 60)} 分钟` : `约剩 ${(s / 3600).toFixed(1)} 小时`; }; };
// Which GPUs a Cryo-ET run uses where there are several (the SSH server's, or this computer's NVIDIA
// cards): a checkbox on each GPU line of the resources under 运行位置. All ticked (the default) leaves it
// to the program; the cards left out are kept per place ('local' or 'ssh:<server>').
window.TmvGpuPick ||= (() => {
  const key = (w) => `cryo-gpus-off:${w}`;
  const get = (w) => { try { const v = JSON.parse(localStorage.getItem(key(w)) || '[]'); return Array.isArray(v) ? v : []; } catch (_) { return []; } };
  const set = (w, off) => { try { localStorage.setItem(key(w), JSON.stringify(off)); } catch (_) {} };
  const where = () => (typeof S !== 'undefined' && S.folder?.remote) ? `ssh:${S.folder.processing?.serverIdentity || S.folder.address || ''}` : 'local';
  const on = (cards, w) => { const off = get(w); return cards.filter((g) => !off.includes(g.uuid)); };
  // What a run is asked: nothing when all are ticked (the program's choice).
  const req = (cards, w = where()) => { if (!cards || cards.length < 2) return {}; const o = on(cards, w); return o.length && o.length < cards.length ? { gpus: o.map((g) => g.uuid) } : {}; };
  const gb = (mb) => (mb / 1024).toFixed(1);
  // One GPU line with its checkbox.
  function row(g, cards, w, disabled, changed) {
    const l = document.createElement('label'); l.className = 'gpu-pick';
    const b = document.createElement('input'); b.type = 'checkbox'; b.checked = !get(w).includes(g.uuid); b.disabled = !!disabled;
    b.onchange = () => {
      let off = get(w).filter((u) => cards.some((c) => c.uuid === u));
      off = b.checked ? off.filter((u) => u !== g.uuid) : [...new Set([...off, g.uuid])];
      if (off.length >= cards.length) { b.checked = true; if (typeof toast === 'function') toast('至少要用一块 GPU'); return; }
      set(w, off); changed?.();
    };
    const t = document.createElement('span');
    t.textContent = `GPU ${g.index} · ${g.name} · 可用显存 ${gb(g.freeMb)}/${gb(g.totalMb)} GB · 当前利用率 ${g.utilization}%`;
    l.append(b, t);
    return l;
  }
  function note(cards, w) {
    const o = on(cards, w), p = document.createElement('p'); p.className = 'dim gpu-pick-note';
    p.textContent = o.length >= cards.length ? '勾选 Cryo-ET 用哪几块 GPU。都勾着：程序自动分工（多个序列时每块 GPU 各做一个），空闲多的先用' : `Cryo-ET 只用 ${o.map((g) => `GPU ${g.index}`).join('、')}，其他的留给别的任务`;
    return p;
  }
  return { where, req, row, note, on };
})();
// One routing boundary: local calls remain native; remote paths never reach local FS.
(() => {
  const prefix = 'tmv-ssh:';
  const contexts = new Map(), active = new Map(), lanes=new Map(), etas=new Map();
  function request(ctx,body,control=false){
    const key=ctx.serverIdentity||ctx.identity;let lane=lanes.get(key);if(!lane){lane={busy:false,control:[],read:[]};lanes.set(key,lane);}
    return new Promise((resolve,reject)=>{(control?lane.control:lane.read).push({ctx,body,resolve,reject});pumpLane(lane);});
  }
  function pumpLane(lane){if(lane.busy)return;const job=lane.control.shift()||lane.read.shift();if(!job)return;lane.busy=true;Promise.resolve().then(()=>job.ctx.call(job.body)).then(job.resolve,job.reject).finally(()=>{lane.busy=false;pumpLane(lane);});}

  const storage = 'tmv.remote.jobs.v1';
  const jobs = () => { try { return JSON.parse(localStorage.getItem(storage) || '[]'); } catch (_) { return []; } };
  const save = records => localStorage.setItem(storage, JSON.stringify(records.slice(-80)));
  const pack = (id,path) => `${prefix}${encodeURIComponent(id)}:${encodeURIComponent(path)}`;
  const unpack = path => { if(typeof path !== 'string' || !path.startsWith(prefix))return null;const i=path.indexOf(':',prefix.length);return {id:decodeURIComponent(path.slice(prefix.length,i)),path:decodeURIComponent(path.slice(i+1))}; };
  // Keys are canonical so a job the server lists (sorted keys) matches the one the panel asks for.
  const canon = v => Array.isArray(v) ? `[${v.map(canon).join(',')}]` : v && typeof v==='object' ? `{${Object.keys(v).sort().map(k=>JSON.stringify(k)+':'+canon(v[k])).join(',')}}` : JSON.stringify(v===undefined?null:v);
  const map = (v,fn) => typeof v === 'string' ? fn(v) : Array.isArray(v) ? v.map(x=>map(x,fn)) : v && typeof v==='object' ? Object.fromEntries(Object.entries(v).map(([k,x])=>[k,map(x,fn)])) : v;
  const current = () => S.folder?.remote ? S.folder.processing : null;
  const label = () => current() ? `运行位置：${S.folder.address} · SSH（原始数据留在服务器）` : '运行位置：本机';
  const emit = (event,payload) => T.event.emit(event,payload).catch(()=>{});
  const terminal = s => ['completed','needsChoice','failed','cancelled'].includes(s);
  // A results folder that cannot be written is said as such, with what to do, not as an OS error.
  function readable(error,out){
    const e=String(error||'服务器处理失败'),where=out?`结果目录 ${out} `:'服务器用户缓存';
    if(/os error 30\b|Read-only file system/i.test(e))return `${where}在只读的盘上，结果写不进去。请在「结果目录」里换一个可写的位置，或清空它改用服务器用户缓存。`;
    if(/os error 13\b|Permission denied/i.test(e)&&out)return `${where}没有写入权限，结果写不进去。请在「结果目录」里换一个自己能写的位置，或清空它改用服务器用户缓存。`;
    if(/os error 28\b|No space left/i.test(e))return `${where}所在的盘满了。请在「结果目录」里换一个空间大的盘。`;
    return e;
  }
  async function watch(ctx,record,cmd) {
    const kind=cmd.startsWith('spa_')?'spa':'tomo', isLong=cmd==='tomo_session_run'||cmd==='spa_run';
    if(isLong){active.set(kind,{ctx,record});ctx.placements=record.placements||{};}
    let cursor=record.cursor||0, refreshed=0;
    try {
      for (;;) {
        let state;
        try { state = await request(ctx,{ action:'status',job:record.job,after:cursor },isLong); }
        catch(e) { ctx.discoveredFor?.clear(); throw new Error(`连接中断，任务仍在服务器上运行；重新连接后会自动接上进度：${e}`); }
        if(cmd==='tomo_session_run' && Date.now()-refreshed>5000){refreshed=Date.now();await emit('tomo-session',{folder:pack(ctx.identity,record.project)});}
        for(const e of state.events || []) { cursor=Math.max(cursor,e.seq);
          if(e.type==='backend'){const p=e.placement;ctx.placements||={};const key=`${p.worker||'本机'}|${p.operation}`;const old=ctx.placements[key];ctx.placements[key]={...p,priorFallback:p.backend==='CPU'&&p.fallback!==false?p.reason:old?.priorFallback};if(p.backend==='CPU'&&p.reason&&p.fallback!==false&&old?.reason!==p.reason)toast(`${p.operation} 使用 CPU：${p.reason}`);localStorage.setItem(`tmv.remote.backends.${ctx.identity}`,JSON.stringify(ctx.placements));if(isLong)record.placements=ctx.placements;continue;}
          if(isLong){if(e.stage){etas.has(record.job)||etas.set(record.job,TmvEta());record.eta=etas.get(record.job)(e);}record.progress=e;record.updated=Date.now();if(e.worker){record.workers||={};record.workers[e.worker]=e;}} await emit(cmd.startsWith('spa_')?'spa-progress':'tomo-progress',e); }
        if(isLong){record.cursor=cursor;record.state=state.state;save([...jobs().filter(j=>j.requestId!==record.requestId),record]);}
        if(state.outputRoot)ctx.lastOutputRoot=state.outputRoot;
        if(isLong)updateBar(`${ctx.label || label()} · ${state.state==='queued'?'等待资源':state.state==='running'?'正在服务器上运行':state.state}`);
        if(terminal(state.state)&&(state.events||[]).length===128)continue;
        if(terminal(state.state)) {
          save(jobs().filter(j=>j.requestId!==record.requestId));
          if(state.state==='failed')throw new Error(readable(state.error,record.outputRoot));
          if(state.state==='cancelled')throw new Error('已取消；已完成步骤仍保留在服务器');
          const result=state.result;
          if(result?.binary)return Uint8Array.from(atob(result.binary),c=>c.charCodeAt(0)).buffer;
          return map(result,s=>s.startsWith('/')?pack(ctx.identity,s):s);
        }
        if(isLong&&Date.now()-(ctx.resourcesAt||0)>15000){ctx.resourcesAt=Date.now();request(ctx,{action:'inspect',outputRoot:ctx.outputRoot}).then(r=>{ctx.resources=r.resources;updateBar();}).catch(()=>{});}
        if((state.events||[]).length<128)await new Promise(resolve=>setTimeout(resolve,isLong?900:150));
      }
    } finally { if(isLong && active.get(kind)?.record===record){active.delete(kind);updateBar();}else if(!isLong&&!active.size)updateBar(); }
  }
  async function remote(ctx,cmd,args,native) {
    if(cmd==='tomo_cancel'&&localRun){localRun.cancelled=true;localRun.ctx.cancelTransfer?.().catch(()=>{});return T.core.invoke('tomo_cancel').catch(()=>{});}
    if(cmd.endsWith('_cancel')) {const job=active.get(cmd.startsWith('spa_')?'spa':'tomo');if(job)return request(job.ctx,{action:'cancel',job:job.record.job},true);return;}
    if(cmd.startsWith('live_'))return null;
    const decoded=map(args,s=>{const x=unpack(s);if(x){if(x.id!==ctx.identity)throw new Error('项目属于其他服务器，请重新连接原服务器');return x.path;}return s===S.folder.dir?S.folder.remotePath:(typeof ext!=='undefined' && ext.remoteFiles?.get(s)?.remotePath)||s;});
    if(cmd==='tomo_preview_file'){
      ctx.previewFiles||=new Map();ctx.previewFiles.delete(decoded.path);
      let local=await ctx.previewFile?.(decoded.path);
      // A big file (a tomogram) is not fetched whole: the server makes a small byte copy of all
      // its sections (tomo_preview_stack), and that comes over in one transfer.
      if(!local&&ctx.previewFile){
        const made=await remote(ctx,'tomo_preview_stack',{path:args.path,max:512},native);
        const small=unpack(made?.path)?.path||made?.path;
        if(small)local=await ctx.previewFile(small);
      }
      if(local)ctx.previewFiles.set(decoded.path,local);
      return !!local;
    }
    if(cmd==='tomo_view'&&ctx.previewFiles?.has(decoded.path))return native(cmd,{...decoded,path:ctx.previewFiles.get(decoded.path)});
    // project identity is pinned at first submission, including after navigation/reconnect.
    const project = decoded.req?.folder || (cmd==='spa_open'?decoded.source:null) || ctx.project;
    const key=canon([ctx.identity,cmd,decoded]);
    let record=jobs().find(j=>j.key===key);
    if(!record){record={key,requestId:crypto.randomUUID(),command:cmd,args:decoded,identity:ctx.identity,project,outputRoot:ctx.outputRoot||null};if(cmd==='tomo_session_run'||cmd==='spa_run')save([...jobs(),record]);}
    if(!record.job){if(!ctx.prepared)updateBar(`${label()} · 正在准备服务器处理引擎…`);const result=await request(ctx,{action:'submit',requestId:record.requestId,command:cmd,args:decoded,project:record.project,outputRoot:record.outputRoot},cmd==='tomo_session_run'||cmd==='spa_run');ctx.prepared=true;record.job=result.job; if(cmd==='tomo_session_run'||cmd==='spa_run')save([...jobs().filter(j=>j.key!==key),record]);}
    return watch(ctx,record,cmd);
  }
  const running = j => j.state==='queued'||j.state==='running';
  const within = (p,root) => !!p && !!root && (p===root || p.startsWith(root.replace(/\/+$/,'')+'/'));
  const kindOf = cmd => cmd==='spa_run'?'spa':'tomo';
  // A job this client already follows (from this window, this session).
  const following = job => [...active.values()].some(a=>a.record.job===job);
  // Long jobs run detached on the server (setsid), so closing the app never
  // stops them. On every connection the server is asked which jobs of this
  // folder still run; each is handed to its panel, which shows the progress
  // as if it had started it. Jobs that ended while the app was closed are
  // reported once.
  async function discover(ctx,asked=false){
    if(!ctx||ctx.discovering)return;ctx.discovering=true;
    try{
      let list;
      try{list=(await request(ctx,{action:'jobs'})).jobs||[];}
      catch(e){if(asked)toast(`无法读取服务器任务：${e}`);return;}
      ctx.serverJobs=list;
      const seenKey=`tmv.remote.lastSeen.${ctx.serverIdentity}`,lastSeen=+localStorage.getItem(seenKey)||0;
      const records=jobs();let changed=false,resumed=0;
      for(const j of list){
        if(!within(j.project,ctx.project))continue;
        let record=records.find(r=>r.job===j.job&&r.identity===ctx.identity);
        if(running(j)){
          if(following(j.job))continue;
          if(!record){record={key:canon([ctx.identity,j.command,j.args]),requestId:j.requestId,command:j.command,args:j.args,identity:ctx.identity,project:j.project,outputRoot:j.outputRoot??null,job:j.job,cursor:0};records.push(record);changed=true;}
          record.progress=j.progress||record.progress;record.state=j.state;
          const detail={kind:kindOf(j.command),record,ctx,claimed:false};
          document.dispatchEvent(new CustomEvent('tmv-remote-running',{detail}));
          // No panel took it (not loaded): follow it here so the bar still shows it.
          if(!detail.claimed)attach(record).catch(()=>{});
          resumed++;
        } else if(record || (lastSeen && j.updated>lastSeen)){
          if(record){records.splice(records.indexOf(record),1);changed=true;}
          const name=j.command==='spa_run'?'单颗粒处理':'冷冻电镜断层处理';
          const where=j.project.split('/').filter(Boolean).pop();
          toast(j.state==='completed'||j.state==='needsChoice'?`软件关闭期间，服务器已完成「${where}」的${name}，结果已在文件夹里`:j.state==='cancelled'?`「${where}」的${name}已取消`:`软件关闭期间，「${where}」的${name}失败：${j.error||'请查看服务器日志'}`);
        }
      }
      if(changed)save(records);
      localStorage.setItem(seenKey,String(Date.now()));
      if(resumed)toast(resumed>1?`已接上服务器上仍在运行的 ${resumed} 个任务`:'已接上服务器上仍在运行的任务，正在显示进度');
      else if(asked)toast('这个文件夹在服务器上没有正在运行的任务');
      if(asked)save(jobs().filter(r=>r.identity!==ctx.identity||list.some(j=>j.job===r.job&&running(j))));
      updateBar();
    } finally { ctx.discovering=false; }
  }
  // Follows a job already on the server; resolves like the call that started it.
  function attach(record){
    const ctx=contexts.get(record.identity)||current();
    if(!ctx)return Promise.reject(new Error('请先重新连接 SSH 服务器'));
    const existing=[...active.values()].find(a=>a.record.job===record.job);
    if(existing)return existing.promise;
    const promise=watch(ctx,record,record.command);
    const entry=active.get(kindOf(record.command));if(entry&&entry.record===record)entry.promise=promise;
    return promise;
  }
  // The first processing request on this computer downloads the server engine (about 100 MB, once).
  function watchEngine(ctx){if(!ctx.engineProgress)return 0;const mb=n=>Math.round(n/1048576);let shown=false;const timer=setInterval(()=>{ctx.engineProgress().then(p=>{if(p&&p.active&&p.total){shown=true;if(current()?.identity===ctx.identity)updateBar(`第一次使用远程计算：正在下载服务器引擎（只需一次）${mb(p.done)} / ${mb(p.total)} MB`);}else if(shown){shown=false;updateBar();}}).catch(()=>{});},700);return timer;}
  // Where a Cryo-ET run of an SSH project goes: on the server (the default), or on this computer:
  // what the ticked series need is downloaded, processed here, and either kept here (and opened)
  // or sent back into the server's results and the copy here removed.
  const hereKey = ctx => `cryo-run-here:${ctx.serverIdentity||ctx.identity}`, keepKey = ctx => `cryo-keep-here:${ctx.serverIdentity||ctx.identity}`;
  const flag = k => { try { return localStorage.getItem(k)==='1'; } catch(_) { return false; } };
  const setFlag = (k,v) => { try { localStorage.setItem(k,v?'1':'0'); } catch(_) {} };
  const runHere = ctx => !!ctx?.transfer && flag(hereKey(ctx));
  let localRun = null;
  function whereBox(ctx,locked){
    const box=document.createElement('div');box.className='run-where';
    const here=runHere(ctx),keep=flag(keepKey(ctx));
    const radio=(value,text,checked)=>{const l=document.createElement('label');const r=document.createElement('input');r.type='radio';r.name='run-where';r.checked=checked;r.disabled=locked;r.onchange=()=>{setFlag(hereKey(ctx),value==='here');updateBar();};l.append(r,document.createTextNode(text));return l;};
    box.append(radio('server','在服务器上跑',!here),radio('here','在这台电脑上跑',here));
    if(here){const l=document.createElement('label');l.className='run-keep';const c=document.createElement('input');c.type='checkbox';c.checked=keep;c.disabled=locked;c.onchange=()=>{setFlag(keepKey(ctx),c.checked);updateBar();};l.append(c,document.createTextNode('结果保存到这台电脑'));box.append(l);
      const n=document.createElement('p');n.className='dim run-where-note';n.textContent=keep?'勾选序列需要的数据先下载到这台电脑，处理完结果留在这台电脑上，并在这里打开。':'勾选序列需要的数据先下载到这台电脑，处理完把结果传回服务器，再删掉这台电脑上的临时文件。';box.append(n);}
    return box;
  }
  const raw = s => unpack(s)?.path || s;
  const mb = n => Math.round(n/1048576);
  async function runLocally(req,native){
    const ctx=current();
    if(!ctx)throw new Error('请先重新连接 SSH 服务器');
    if(!ctx.transfer)throw new Error('在这台电脑上跑需要 SSH 插件 1.6.0 以上，请到插件市场更新');
    const keep=flag(keepKey(ctx));
    const run=localRun={ctx,cancelled:false};
    const step=(stage,done,total)=>emit('tomo-progress',{stage,done,total});
    const stopped=()=>{if(run.cancelled)throw new Error('已取消；已下载的文件会留着，下次接着用');};
    try{
      updateBar();
      step('正在列出需要下载的文件',0,1);
      const list=await remote(ctx,'tomo_session_files',{folder:req.folder,only:req.only||[]},native);
      const project=raw(list.project),output=raw(list.output),files=list.files||[];
      stopped();
      let dir=null;
      if(keep){
        const name=project.split('/').filter(Boolean).pop()||'project';
        const base=localStorage.getItem('tmv.localRun.dir')||await T.core.invoke('pick_folder',{dir:''});
        if(!base)throw new Error('没有选保存结果的文件夹');
        try{localStorage.setItem('tmv.localRun.dir',base);}catch(_){}
        dir=`${base.replace(/[\\/]+$/,'')}/${name}`;
      }
      const folder=await T.core.invoke('tomo_mirror',{action:'prepare',key:`${ctx.serverIdentity||ctx.identity}|${project}`,folder:dir});
      const total=files.reduce((a,f)=>a+(f.size||0),0)||1;let done=0;
      for(const f of files){stopped();
        await ctx.transfer('fetchTo',{path:raw(f.path),dest:`${folder}/${f.rel}`},p=>step(`下载到这台电脑：${f.rel.split('/').pop()}（MB）`,mb(done+(p.done||0)),mb(total)));
        done+=f.size||0;}
      await T.core.invoke('tomo_mirror',{action:'rebaseIn',folder,project,output});
      const before=await T.core.invoke('tomo_mirror',{action:'manifest',folder});
      stopped();
      const here=window.TmvLocal?TmvLocal.invoke('tomo_session_run',{req:{folder,purpose:req.purpose,only:req.only}},(n,v)=>T.core.invoke(n,v)):T.core.invoke('tomo_session_run',{req:{folder,purpose:req.purpose,only:req.only}});
      const result=await here;
      if(keep){await T.core.invoke('tomo_mirror',{action:'keep',folder});return {local:folder,session:result};}
      const ups=await T.core.invoke('tomo_mirror',{action:'changes',folder,project,output,before});
      const up=ups.reduce((a,u)=>a+u.size,0)||1;let sent=0;
      for(const u of ups){stopped();
        await ctx.transfer('upload',{local:u.local,remote:u.remote},p=>step(`把结果传回服务器：${u.remote.split('/').pop()}（MB）`,mb(sent+(p.done||0)),mb(up)));
        sent+=u.size;}
      await T.core.invoke('tomo_mirror',{action:'remove',folder});
      step('结果已传回服务器',1,1);
      return {session:await remote(ctx,'tomo_session',{folder:req.folder},native)};
    } finally { if(localRun===run)localRun=null; updateBar(); }
  }
  function updateBar(message) {
    for(const id of ['cryoet-workspace','spa-workspace']) {const panel=$(id);if(!panel)continue;let bar=panel.querySelector('.remote-processing-location');if(!bar){bar=document.createElement('div');bar.className='remote-processing-location';panel.querySelector('.al-head')?.after(bar);}
      if(!current()&&window.TmvLocal){TmvLocal.render(bar,id==='spa-workspace'?'spa':'tomo');continue;}
      const expanded=new Map([...bar.querySelectorAll('details')].map(d=>[d.className,d.open]));
      bar.replaceChildren();const text=document.createElement('span');text.textContent=message||label();bar.append(text);
      const task=[...active.values()].find(j=>j.ctx.identity===current()?.identity);
      // 停止, at the right of the 运行位置 line: the steps done stay on the server, and the series' cards offer 继续.
      if(task){const stop=document.createElement('button');stop.className='small remote-stop';stop.textContent=task.stopping?'正在停止…':'停止';stop.disabled=!!task.stopping;stop.title='停下这个任务；做完的步骤保留在服务器上，以后可以接着做';stop.onclick=()=>{task.stopping=true;stop.disabled=true;stop.textContent='正在停止…';request(task.ctx,{action:'cancel',job:task.record.job},true).catch(e=>{task.stopping=false;if(typeof toast==='function')toast(`无法停止：${e}`);updateBar();});};bar.append(stop);}
      if(task){const rec=task.record,p=rec.progress;const status=document.createElement('div');status.className='remote-task-status';status.setAttribute('role','status');
        const phase=document.createElement('strong');phase.textContent=rec.state==='queued'?'等待服务器资源…':p?.stage||'正在提交并读取服务器进度…';
        const count=document.createElement('span');count.textContent=p&&p.total>1?`${p.done} / ${p.total}${rec.eta?' · '+rec.eta:''}`:'正在处理，请稍候';
        const progress=document.createElement('progress');progress.setAttribute('aria-label','当前阶段进度');if(p&&p.total>1){progress.max=p.total;progress.value=p.done;}
        const away=document.createElement('small');away.className='dim remote-task-away';away.textContent='可以关闭软件：任务在服务器上继续运行，下次连接这个文件夹会自动显示进度';status.append(phase,count,progress,away);bar.append(status);
        const workers=Object.entries(rec.workers||{});if(workers.length){const grid=document.createElement('div');grid.className='remote-worker-grid';for(const [name,p]of workers){const card=document.createElement('div');card.className='remote-worker-card';const title=document.createElement('strong');title.textContent=name;const stage=document.createElement('span');stage.textContent=p.stage.replace(/^GPU \d+ · /,'');const meter=document.createElement('progress');meter.setAttribute('aria-label',name+' 当前阶段进度');if(p.total>1){meter.max=p.total;meter.value=p.done;}const count=document.createElement('small');count.textContent=p.total>1?`${p.done} / ${p.total}`:'正在处理';card.append(title,stage,meter,count);grid.append(card);}bar.append(grid);}}

      const ctx=current();
      if(id==='cryoet-workspace'&&ctx?.transfer&&window.TifMrcViewPro)bar.append(whereBox(ctx,!!task||!!localRun));
      if(ctx?.resources){const r=ctx.resources,details=document.createElement('details');details.className='remote-resources';const title=document.createElement('summary');title.textContent=`服务器资源：CPU ${r.cpuThreads} 线程 · 可用内存 ${(r.memoryAvailableMb/1024).toFixed(1)} GB · ${r.gpus.length} 张 GPU`;details.append(title);const pickable=id==='cryoet-workspace'&&r.gpus.length>1&&!runHere(ctx);const w=`ssh:${ctx.serverIdentity||''}`;for(const g of r.gpus){if(pickable){details.append(TmvGpuPick.row(g,r.gpus,w,!!task,()=>updateBar()));continue;}const row=document.createElement('p');row.textContent=`GPU ${g.index} · ${g.name} · 可用显存 ${(g.freeMb/1024).toFixed(1)}/${(g.totalMb/1024).toFixed(1)} GB · 当前利用率 ${g.utilization}%`;details.append(row);}if(pickable)details.append(TmvGpuPick.note(r.gpus,w));if(r.outputFreeMb!=null){const row=document.createElement('p');row.textContent=`输出磁盘可用 ${(r.outputFreeMb/1024).toFixed(1)} GB`;details.append(row);}for(const w of r.warnings||[]){const row=document.createElement('p');row.textContent=w;details.append(row);}bar.append(details);}
      const placements=Object.values(ctx?.placements||{});
      const advanced=window.TmvHelp.advanced(id);
      if(placements.length){const details=document.createElement('details');details.className='remote-backends';details.open=false;const title=document.createElement('summary');title.textContent=task?'每一步实际计算设备':'上次任务的计算设备';details.append(title);for(const p of placements){const row=document.createElement('div');row.className='remote-backend-row';row.classList.toggle('fallback',p.backend==='CPU'&&!!p.reason&&p.fallback!==false);const name=document.createElement('strong');name.textContent=p.operation;const device=document.createElement('span');device.textContent=`${p.backend}${p.device?' · '+p.device:p.worker?' · '+p.worker+' 的 CPU 路径':''}`;row.append(name,device);if(p.reason){const reason=document.createElement('small');reason.textContent=(p.backend==='CPU'?(p.fallback===false?'CPU 分工：':'CPU 回退原因：'):'分工：')+p.reason;row.append(reason);}if(p.priorFallback&&p.backend!=='CPU'){const prior=document.createElement('small');prior.textContent='本次此前曾回退 CPU：'+p.priorFallback;row.append(prior);}details.append(row);}advanced.append(details);}
      // The results folder: where the last task wrote, and where the next one writes (editable), folded together.
      let out=null;if(current()){out=document.createElement('details');out.className='remote-output-details';const summary=document.createElement('summary');summary.textContent='结果目录';out.append(summary);if(current().lastOutputRoot){const p=document.createElement('p');p.className='dim';p.textContent='上次的结果在：';const code=document.createElement('code');code.textContent=current().lastOutputRoot;out.append(p,code);}advanced.append(out);}
      if(current()) {const input=document.createElement('input');input.className='remote-output';input.placeholder='输出位置：服务器用户缓存（可填写其他磁盘目录）';input.setAttribute('aria-label','服务器输出位置');input.value=current().outputRoot||'';input.disabled=[...active.values()].some(j=>j.ctx.identity===current().identity);const setOut=async path=>{const ctx=current();if(path&&!path.startsWith('/')){toast('请填写服务器绝对路径');return;}if(path){const r=await request(ctx,{action:'inspect',outputRoot:path}).catch(()=>null);if(r?.resources?.outputWritable===false){toast(`${path} 不能写入（只读的盘或没有权限），请换一个位置`);input.value=ctx.outputRoot||'';return;}}ctx.outputRoot=path||null;localStorage.setItem(`tmv.remote.output.${ctx.identity}`,path);window.TifMrcViewPro?.spa.show(null);window.TifMrcViewPro?.spa.useCurrent?.();document.dispatchEvent(new CustomEvent('cryoet-open'));};input.onchange=()=>setOut(input.value.trim());const lab=document.createElement('p');lab.className='dim';lab.textContent='以后的结果保存到（服务器上的绝对路径；空着就用服务器用户缓存）：';const pick=document.createElement('button');pick.className='small remote-output-pick';pick.textContent='选位置…';pick.title='在服务器上浏览文件夹，选一个保存结果';pick.disabled=input.disabled;pick.onclick=async()=>{const ctx=current();if(!ctx?.pickFolder)return;try{const p=await ctx.pickFolder(ctx.outputRoot||ctx.lastOutputRoot||ctx.project,path=>request(ctx,{action:'inspect',outputRoot:path}).then(r=>r.resources));if(p&&p!==ctx.outputRoot){input.value=p;await setOut(p);updateBar();}}catch(e){if(typeof toast==='function')toast(`${e}`);}};const row=document.createElement('div');row.className='remote-output-row';row.append(input,pick);out.append(lab,row);const free=ctx?.resources?.outputFreeMb;if(free!=null){const f=document.createElement('p');f.className=free<50*1024?'ssh-pick-low':'dim';f.textContent=`${ctx.outputRoot?'这个位置':'服务器用户缓存'}所在的盘还剩 ${free>=1048576?(free/1048576).toFixed(1)+' TB':(free/1024).toFixed(1)+' GB'}${free<50*1024?'，空间不多，建议换一个盘':''}`;out.append(f);}}
      if(current() && !task && jobs().some(j=>j.identity===current().identity && j.job)) {const b=document.createElement('button');b.className='small';b.textContent='恢复任务';b.title='重新显示这个文件夹在服务器上运行的任务';b.onclick=()=>discover(current(),true);bar.append(b);}
      const others=(ctx?.serverJobs||[]).filter(j=>running(j)&&!within(j.project,ctx.project));
      if(current() && others.length){const p=document.createElement('p');p.className='dim remote-other-jobs';p.textContent=`这台服务器上还有 ${others.length} 个任务在运行（${others.map(j=>j.project.split('/').filter(Boolean).pop()).join('、')}），打开对应文件夹即可查看进度`;bar.append(p);}
      window.TmvHelp.finish(advanced,id);if(advanced.childElementCount>1)bar.append(advanced);
      for(const d of bar.querySelectorAll('details'))if(expanded.has(d.className))d.open=expanded.get(d.className);
    }
  }
  window.TmvRemote={
    mount(folder){const previous=window.TmvRemote.mountedIdentity;if(folder.processing){folder.processing.serverIdentity ||= folder.processing.identity;folder.processing.identity = `${folder.processing.serverIdentity}|${folder.remotePath}`;folder.processing.label=`运行位置：${folder.address} · SSH`;folder.processing.project=folder.remotePath;folder.processing.outputRoot=localStorage.getItem(`tmv.remote.output.${folder.processing.identity}`)||null;contexts.set(folder.processing.identity,folder.processing);const ctx=folder.processing;if(window.TifMrcViewPro&&!ctx.resourcesLoading){ctx.resourcesLoading=true;const fetching=watchEngine(ctx);request(ctx,{action:'inspect',outputRoot:ctx.outputRoot}).finally(()=>clearInterval(fetching)).then(r=>{ctx.resources=r.resources;ctx.resourcesAt=Date.now();try{ctx.placements=JSON.parse(localStorage.getItem(`tmv.remote.backends.${ctx.identity}`)||'{}');}catch(_){}updateBar();}).catch(e=>{ctx.resourceError=String(e);updateBar();});}}updateBar();const identity=folder.processing?.identity||null;window.TmvRemote.mountedIdentity=identity;if(identity!==previous)document.dispatchEvent(new CustomEvent('tmv-remote-mounted'));
      // Once per connection and folder: pick up jobs that kept running while the app was closed.
      const ctx=folder.processing;if(ctx){ctx.discoveredFor||=new Set();if(!ctx.discoveredFor.has(identity)){ctx.discoveredFor.add(identity);setTimeout(()=>discover(ctx),0);}}},
    label, updateBar, pack, attach, discover:()=>discover(current(),true),
    // A Cryo-ET run of the open SSH project on this computer (the 运行位置 switch), and the server's GPUs.
    runHere:()=>runHere(current()), runLocally:req=>runLocally(req,(c,a)=>T.core.invoke(c,a)), gpus:()=>current()?.resources?.gpus||[],
    // A server path as the panels name it: the open folder itself, or a packed remote path.
    localPath:(record,path)=>{const ctx=contexts.get(record.identity)||current();return path===S.folder?.remotePath&&ctx===current()?S.folder.dir:pack(ctx.identity,path);},
    displayPath:path=>unpack(path)?.path||path,
    async open(path){const p=unpack(path);if(!p)return false;const ctx=contexts.get(p.id);if(!ctx)throw new Error('请重新连接原 SSH 服务器后打开结果');await ctx.open(p.path);return true;},
    invoke(cmd,args,native){
      const values=[];map(args,s=>{if(unpack(s))values.push(unpack(s));return s;});
      const pinned=values[0];const ctx=pinned?contexts.get(pinned.id):current();
      const processing=(/^(tomo_|spa_|live_)/.test(cmd) || cmd==='gpu_info');
      if(pinned && !processing && cmd!=='pick_folder')throw new Error('此操作尚未接入远程工作区，请先打开结果图像');
      if(cmd==='pick_folder' && ctx)return ctx.choose();
      if(processing && (pinned || ctx)) {if(!ctx)throw new Error('请先重新连接 SSH 服务器');return remote(ctx,cmd,args||{},native);}
      if(cmd==='tomo_preview_file')return Promise.resolve(false);
      return native(cmd,args);
    },
  };
  document.addEventListener('pro-panel',()=>updateBar());
})();
