"use strict";
// One routing boundary: local calls remain native; remote paths never reach local FS.
(() => {
  const prefix = 'tmv-ssh:';
  const contexts = new Map(), active = new Map(), lanes=new Map();
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
  const map = (v,fn) => typeof v === 'string' ? fn(v) : Array.isArray(v) ? v.map(x=>map(x,fn)) : v && typeof v==='object' ? Object.fromEntries(Object.entries(v).map(([k,x])=>[k,map(x,fn)])) : v;
  const current = () => S.folder?.remote ? S.folder.processing : null;
  const label = () => current() ? `运行位置：${S.folder.address} · SSH（原始数据留在服务器）` : '运行位置：本机';
  const emit = (event,payload) => T.event.emit(event,payload).catch(()=>{});
  const terminal = s => ['completed','needsChoice','failed','cancelled'].includes(s);
  async function watch(ctx,record,cmd) {
    const kind=cmd.startsWith('spa_')?'spa':'tomo', isLong=cmd==='tomo_session_run'||cmd==='spa_run';
    if(isLong)active.set(kind,{ctx,record});
    let cursor=record.cursor||0, refreshed=0;
    try {
      for (;;) {
        let state;
        try { state = await request(ctx,{ action:'status',job:record.job,after:cursor },isLong); }
        catch(e) { throw new Error(`远程任务保留在服务器，重连后点“恢复任务”：${e}`); }
        if(cmd==='tomo_session_run' && Date.now()-refreshed>5000){refreshed=Date.now();await emit('tomo-session',{folder:pack(ctx.identity,record.project)});}
        for(const e of state.events || []) { cursor=Math.max(cursor,e.seq); if(isLong){record.progress=e;record.updated=Date.now();} await emit(cmd.startsWith('spa_')?'spa-progress':'tomo-progress',e); }
        if(isLong){record.cursor=cursor;record.state=state.state;save([...jobs().filter(j=>j.requestId!==record.requestId),record]);}
        if(state.outputRoot)ctx.lastOutputRoot=state.outputRoot;
        if(isLong)updateBar(`${ctx.label || label()} · ${state.state==='queued'?'等待资源':state.state==='running'?'正在服务器上运行':state.state}`);
        if(terminal(state.state)) {
          save(jobs().filter(j=>j.requestId!==record.requestId));
          if(state.state==='failed')throw new Error(state.error || '服务器处理失败');
          if(state.state==='cancelled')throw new Error('已取消；已完成步骤仍保留在服务器');
          const result=state.result;
          if(result?.binary)return Uint8Array.from(atob(result.binary),c=>c.charCodeAt(0)).buffer;
          return map(result,s=>s.startsWith('/')?pack(ctx.identity,s):s);
        }
        if((state.events||[]).length<128)await new Promise(resolve=>setTimeout(resolve,isLong?900:150));
      }
    } finally { if(isLong && active.get(kind)?.record===record){active.delete(kind);updateBar();}else if(!isLong&&!active.size)updateBar(); }
  }
  async function remote(ctx,cmd,args) {
    if(cmd.endsWith('_cancel')) {const job=active.get(cmd.startsWith('spa_')?'spa':'tomo');if(job)return request(job.ctx,{action:'cancel',job:job.record.job},true);return;}
    if(cmd.startsWith('live_'))return null;
    const decoded=map(args,s=>{const x=unpack(s);if(x){if(x.id!==ctx.identity)throw new Error('项目属于其他服务器，请重新连接原服务器');return x.path;}return s===S.folder.dir?S.folder.remotePath:(typeof ext!=='undefined' && ext.remoteFiles?.get(s)?.remotePath)||s;});
    // project identity is pinned at first submission, including after navigation/reconnect.
    const project = decoded.req?.folder || (cmd==='spa_open'?decoded.source:null) || ctx.project;
    const key=JSON.stringify([ctx.identity,cmd,decoded]);
    let record=jobs().find(j=>j.key===key);
    if(!record){record={key,requestId:crypto.randomUUID(),command:cmd,args:decoded,identity:ctx.identity,project,outputRoot:ctx.outputRoot||null};if(cmd==='tomo_session_run'||cmd==='spa_run')save([...jobs(),record]);}
    if(!record.job){if(!ctx.prepared)updateBar(`${label()} · 正在准备服务器处理引擎…`);const result=await request(ctx,{action:'submit',requestId:record.requestId,command:cmd,args:decoded,project:record.project,outputRoot:record.outputRoot},cmd==='tomo_session_run'||cmd==='spa_run');ctx.prepared=true;record.job=result.job; if(cmd==='tomo_session_run'||cmd==='spa_run')save([...jobs().filter(j=>j.key!==key),record]);}
    return watch(ctx,record,cmd);
  }
  function updateBar(message) {
    for(const id of ['cryoet-workspace','spa-workspace']) {const panel=$(id);if(!panel)continue;let bar=panel.querySelector('.remote-processing-location');if(!bar){bar=document.createElement('div');bar.className='remote-processing-location';panel.querySelector('.al-head')?.after(bar);}
      bar.replaceChildren();const text=document.createElement('span');text.textContent=message||label();bar.append(text);
      const task=[...active.values()].find(j=>j.ctx.identity===current()?.identity);
      if(task){const rec=task.record,p=rec.progress;const status=document.createElement('div');status.className='remote-task-status';status.setAttribute('role','status');
        const phase=document.createElement('strong');phase.textContent=rec.state==='queued'?'等待服务器资源…':p?.stage||'正在提交并读取服务器进度…';
        const count=document.createElement('span');count.textContent=p&&p.total>1?`${p.done} / ${p.total}`:'正在处理，请稍候';
        const progress=document.createElement('progress');progress.setAttribute('aria-label','当前阶段进度');if(p&&p.total>1){progress.max=p.total;progress.value=p.done;}
        status.append(phase,count,progress);bar.append(status);}
      if(current()?.lastOutputRoot){const details=document.createElement('details');details.className='remote-output-details';const summary=document.createElement('summary');summary.textContent='查看服务器结果目录';const code=document.createElement('code');code.textContent=current().lastOutputRoot;details.append(summary,code);bar.append(details);}
      if(current()) {const input=document.createElement('input');input.className='remote-output';input.placeholder='输出位置：服务器用户缓存（可填写其他磁盘目录）';input.setAttribute('aria-label','服务器输出位置');input.value=current().outputRoot||'';input.disabled=[...active.values()].some(j=>j.ctx.identity===current().identity);input.onchange=()=>{const ctx=current();const path=input.value.trim();if(path&&!path.startsWith('/')){toast('请填写服务器绝对路径');return;}ctx.outputRoot=path||null;localStorage.setItem(`tmv.remote.output.${ctx.identity}`,path);window.TifMrcViewPro?.spa.show(null);document.dispatchEvent(new CustomEvent('cryoet-open'));};bar.append(input);}
      if(current() && jobs().some(j=>j.identity===current().identity)) {const b=document.createElement('button');b.className='small';b.textContent='恢复任务';b.disabled=[...active.values()].some(j=>j.ctx.identity===current().identity);b.onclick=async()=>{const ctx=current();const record=jobs().find(j=>j.identity===ctx.identity);if(!record)return;try {const result=await remote(ctx,record.command,map(record.args,s=>s.startsWith('/')?pack(ctx.identity,s):s));if(record.command==='spa_run')window.TifMrcViewPro?.spa.show(result.project);else await emit('tomo-session',{folder:S.folder.dir});toast('服务器任务状态已恢复');}catch(e){toast(String(e));}};bar.append(b);}
    }
  }
  window.TmvRemote={
    mount(folder){const previous=window.TmvRemote.mountedIdentity;if(folder.processing){folder.processing.serverIdentity ||= folder.processing.identity;folder.processing.identity = `${folder.processing.serverIdentity}|${folder.remotePath}`;folder.processing.label=`运行位置：${folder.address} · SSH`;folder.processing.project=folder.remotePath;folder.processing.outputRoot=localStorage.getItem(`tmv.remote.output.${folder.processing.identity}`)||null;contexts.set(folder.processing.identity,folder.processing);}updateBar();const identity=folder.processing?.identity||null;window.TmvRemote.mountedIdentity=identity;if(identity!==previous)document.dispatchEvent(new CustomEvent('tmv-remote-mounted'));},
    label, updateBar, pack, displayPath:path=>unpack(path)?.path||path,
    async open(path){const p=unpack(path);if(!p)return false;const ctx=contexts.get(p.id);if(!ctx)throw new Error('请重新连接原 SSH 服务器后打开结果');await ctx.open(p.path);return true;},
    invoke(cmd,args,native){
      const values=[];map(args,s=>{if(unpack(s))values.push(unpack(s));return s;});
      const pinned=values[0];const ctx=pinned?contexts.get(pinned.id):current();
      const processing=(/^(tomo_|spa_|live_)/.test(cmd) || cmd==='gpu_info');
      if(pinned && !processing && cmd!=='pick_folder')throw new Error('此操作尚未接入远程工作区，请先打开结果图像');
      if(cmd==='pick_folder' && ctx)return ctx.choose();
      if(processing && (pinned || ctx)) {if(!ctx)throw new Error('请先重新连接 SSH 服务器');return remote(ctx,cmd,args||{});}
      return native(cmd,args);
    },
  };
  document.addEventListener('pro-panel',()=>updateBar());
})();
