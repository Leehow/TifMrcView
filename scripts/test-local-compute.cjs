const fs=require('node:fs'),assert=require('node:assert/strict');
const {chromium}=require('playwright');
(async()=>{const browser=await chromium.launch({executablePath:process.env.TMV_TEST_CHROMIUM});try{
 const page=await browser.newPage();await page.route('http://localhost/',r=>r.fulfill({contentType:'text/html',body:'<html><body></body></html>'}));await page.goto('http://localhost/');
 await page.setContent('<div id="cryoet-workspace"><div class="al-head"></div></div><div id="spa-workspace"><div class="al-head"></div></div>');
 await page.evaluate(()=>{window.S={folder:null};window.TifMrcViewPro={};window.toasts=[];window.toast=s=>toasts.push(s);window.__TAURI__={event:{listen:async(name,f)=>{window.receive=f;return()=>{};}}};window.pending={};window.native=async(c,a)=>{if(c==='local_compute_resources')return{cpuThreads:10,memoryAvailableMb:12000,metal:{name:'Apple M1 Max',unified:true,availableBytes:8*2**30}};return new Promise((resolve,reject)=>pending[c]={resolve,reject,args:a});};});
 await page.addScriptTag({content:fs.readFileSync('ui/local-processing.js','utf8')});
 await page.evaluate(()=>{window.tomo=TmvLocal.invoke('tomo_session_run',{req:{}},native);window.spa=TmvLocal.invoke('spa_run',{req:{}},native);});
 await page.waitForFunction(()=>pending.spa_run&&pending.tomo_session_run);
 await page.evaluate(()=>{receive({payload:{id:pending.tomo_session_run.args.computeId,placement:{operation:'反卷积',backend:'CPU',reason:'内存预算不足',fallback:true}}});receive({payload:{id:pending.spa_run.args.computeId,placement:{operation:'分类',backend:'GPU · Metal',device:'Apple M1 Max',fallback:false}}});});
 assert.match(await page.locator('#cryoet-workspace').textContent(),/内存预算不足/);assert.doesNotMatch(await page.locator('#spa-workspace').textContent(),/预算不足/);assert.match(await page.locator('#spa-workspace').textContent(),/Apple M1 Max/);assert.equal(await page.evaluate(()=>toasts.length),1);assert.match(await page.locator('#spa-workspace [role=status]').textContent(),/分类：GPU · Metal/);assert.equal(await page.locator('#spa-workspace details').evaluate(e=>e.open),false);
 await page.locator('#cryoet-workspace details').evaluate(e=>e.open=false);
 await page.evaluate(()=>receive({payload:{id:pending.tomo_session_run.args.computeId,placement:{operation:'反卷积',backend:'GPU · Metal',fallback:false}}}));
 assert.equal(await page.locator('#cryoet-workspace details').evaluate(e=>e.open),false);assert.match(await page.locator('#cryoet-workspace').textContent(),/此前曾回退/);
 await page.evaluate(()=>{pending.tomo_session_run.resolve({});pending.spa_run.resolve({});});await page.evaluate(()=>Promise.all([tomo,spa]));assert.match(await page.locator('#cryoet-workspace').textContent(),/已完成/);
 await page.evaluate(()=>{S.folder={remote:true};document.getElementById('spa-workspace').textContent='SSH server';document.dispatchEvent(new CustomEvent('pro-panel'));});assert.equal(await page.locator('#spa-workspace').textContent(),'SSH server');
 await page.evaluate(()=>{S.folder=null;window.second=TmvLocal.invoke('tomo_session_run',{req:{}},native);});await page.waitForFunction(()=>document.getElementById('cryoet-workspace').textContent.includes('正在准备数据'));assert.doesNotMatch(await page.locator('#cryoet-workspace').textContent(),/此前曾回退/);await page.evaluate(()=>pending.tomo_session_run.resolve({}));await page.evaluate(()=>second);
 console.log('PASS: local task isolation, actual Metal/memory, fallback toast/history, details preservation, SSH isolation, new task reset');
}finally{await browser.close();}})().catch(e=>{console.error(e);process.exitCode=1});
