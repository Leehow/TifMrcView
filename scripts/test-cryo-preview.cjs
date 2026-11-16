// Real session UI, deterministic delayed/failing pictures; no downloads or network.
const assert=require('node:assert/strict'),fs=require('node:fs'),http=require('node:http');
const {chromium}=require('playwright');
const server=http.createServer((req,res)=>{const name=req.url.slice(1);if(!['rules.js','session.js','style.css'].includes(name)){res.writeHead(404);return res.end();}const path=name==='style.css'?'ui/pro/pro.css':`ui/pro/cryoet/${name}`;res.setHeader('Content-Type',name.endsWith('css')?'text/css':'application/javascript');res.end(fs.readFileSync(path));});
(async()=>{await new Promise(r=>server.listen(0,'127.0.0.1',r));const origin=`http://127.0.0.1:${server.address().port}`;const browser=await chromium.launch({executablePath:process.env.TMV_TEST_CHROMIUM});try{
 const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.setContent(`<style>:root{--line:#ddd;--dim:#656d76;--subtle:#f6f8fa}#cryoet-workspace{height:650px;overflow:auto;width:850px}.cs-review{overflow:visible}</style><section id="cryoet-workspace"><div id="cryo-session"></div><p id="test-status"></p></section>`);
 await page.evaluate(()=>{
  window.$=id=>document.getElementById(id);window.S={folder:{dir:'/data',remote:true},info:null};window.pref={get:(_,d)=>d,set(){}};window.dirname=p=>p.slice(0,p.lastIndexOf('/'));window.saveDir=()=>'/data';window.openHere=()=>{};
  window.T={event:{listen:async()=>()=>{}}};window.calls=[];window.failZ=null;window.ready=false;
  const series={name:'test_TS',stage:'quick',mdoc:'/data/ts.mdoc',stack:'/data/stack.mrc',small:null,aligned:'/data/aligned.mrc',quick:'/data/tomo.mrc',result:null,movies:60,corrected:60,views:60,leftOut:0,chosen:true,take:null,note:null,error:null,residual:1.2};
  const dataset={alignment:{views:Array.from({length:60},(_,i)=>({fileIndex:i,angle:i-30,residual:1,excluded:false}))},checks:[],picks:[],tomograms:[{path:'/data/tomo.mrc',thickness:60}]};
  window.invoke=async(command,args)=>{
   if(command==='tomo_session')return{root:'/data',series:window.ready?[series]:[],warnings:[],gains:[]};
   if(command==='tomo_dataset')return dataset;
   if(command!=='tomo_view')return null;
   window.calls.push(args);await new Promise(r=>setTimeout(r,220));if(window.failZ!=null&&window.failZ===args.z)throw Error('fixture disconnected');
   const title=new TextEncoder().encode(`z ${(args.z??30)+1} / 60`),at=12+Math.ceil(title.length/4)*4,b=new ArrayBuffer(at+16*16*4),v=new DataView(b);v.setUint32(0,16,true);v.setUint32(4,16,true);v.setUint32(8,title.length,true);new Uint8Array(b,12,title.length).set(title);new Uint8Array(b,at).fill(180);return b;
  };
  window.TifMrcViewPro={cryoet:{panel:$('cryoet-workspace'),busy:false,name:p=>p.split('/').pop(),status:t=>$('test-status').textContent=t,load:async()=>dataset,setDataset(){},run:async(_,f)=>f()}};
 });
 await page.addStyleTag({url:origin+'/style.css'});await page.addScriptTag({url:origin+'/rules.js'});await page.addScriptTag({url:origin+'/session.js'});
 await page.evaluate(()=>{window.ready=true;document.dispatchEvent(new CustomEvent('cryoet-open'));});await page.locator('.cs-tile').waitFor();assert.equal(await page.locator('.cs-scope').inputValue(),'test_TS','an empty initial scan must not make all series the default');
 await page.locator('.cs-tile').click();await page.getByRole('button',{name:'对齐',exact:true}).click();
 await page.locator('.cs-preview-state').filter({hasText:'正在'}).waitFor({state:'visible'});
 await page.waitForFunction(()=>document.querySelector('.cs-film').width===16);
 assert.ok(await page.evaluate(()=>calls.filter(c=>c.path==='/data/aligned.mrc').length)<60,'first frame must be usable before fetching the whole movie');
 await page.getByRole('button',{name:'播放',exact:true}).click();await page.waitForFunction(()=>+document.querySelector('.cs-slider').value>0);await page.getByRole('button',{name:'暂停',exact:true}).click();
 await page.evaluate(()=>{window.failZ=25;const s=document.querySelector('.cs-slider');s.value=25;s.dispatchEvent(new Event('input'));});
 await page.getByRole('button',{name:'重试',exact:true}).waitFor({state:'visible'});assert.match(await page.locator('.cs-preview-state').textContent(),/加载失败/);
 await page.evaluate(()=>window.failZ=null);await page.getByRole('button',{name:'重试',exact:true}).click();await page.waitForFunction(()=>document.querySelector('.cs-preview-state').hidden);
 await page.getByRole('button',{name:'断层图',exact:true}).click();await page.locator('.cs-preview-state').filter({hasText:'正在'}).waitFor({state:'visible'});await page.waitForFunction(()=>document.querySelector('.cs-framelab').textContent.includes('31 / 60'));
 assert.equal(errors.length,0,errors.join('\n'));console.log('PASS: first frame before full download, streaming playback, buffering/loading state, failure/retry and tomogram preview');
}finally{await browser.close();server.close();}})().catch(e=>{console.error(e);server.close();process.exitCode=1});
