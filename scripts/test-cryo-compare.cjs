// The 对照 pane (ui/pro/cryoet/compare.js) beside the main window, with a fake viewer.
const assert=require('node:assert/strict'),fs=require('node:fs'),http=require('node:http');
const {chromium}=require('playwright');
const files={'rules.js':'ui/pro/cryoet/rules.js','compare.js':'ui/pro/cryoet/compare.js','style.css':'ui/pro/pro.css'};
const server=http.createServer((req,res)=>{const f=files[req.url.slice(1)];if(!f){res.writeHead(404);return res.end();}res.setHeader('Content-Type',f.endsWith('css')?'text/css':'application/javascript');res.end(fs.readFileSync(f));});
(async()=>{await new Promise(r=>server.listen(0,'127.0.0.1',r));const browser=await chromium.launch({executablePath:process.env.TMV_TEST_CHROMIUM});try{
const page=await browser.newPage({viewport:{width:1200,height:800}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
await page.setContent('<style>:root{--line:#ddd;--surface:#fff;--accent:#e21b13}#stage{position:relative;width:1000px;height:700px}</style><main id="stage"></main>');
await page.evaluate(()=>{
 window.$=id=>document.getElementById(id);window.pref={get:(_,d)=>d,set(){}};window.toast=t=>{throw Error(t)};window.calls=[];window.requestRender=()=>{};
 window.S={info:null,z:0,axis:'xy'};
 const fake=(path,z0)=>{S.info={path,sizeZ:path.includes('tomo')?300:60};S.z=z0??Math.floor(S.info.sizeZ/2);document.dispatchEvent(new Event('tmv-opened'));};
 window.setZ=z=>{S.z=z;};
 window.openHere=async p=>{calls.push({command:'openHere',p});fake(p);};
 window.TifMrcViewPro={cryoet:{name:p=>p.split('/').pop()},documents:{current:()=>({type:'image'}),open:async(p,f,s,o)=>{calls.push({command:'docs.open',p,o});fake(p);}}};
 window.invoke=async(command,args)=>{calls.push({command,...args});if(command==='tomo_preview_file')return '';if(command!=='tomo_view')return null;
  const depth=args.path.includes('tomo')?300:60,title=new TextEncoder().encode(`z ${(args.z??Math.floor(depth/2))+1} / ${depth}`),at=12+Math.ceil(title.length/4)*4,b=new ArrayBuffer(at+8*8*4),v=new DataView(b);v.setUint32(0,8,true);v.setUint32(4,8,true);v.setUint32(8,title.length,true);new Uint8Array(b,12,title.length).set(title);new Uint8Array(b,at).fill(100);return b;};
 window.fake=fake;
});
const origin=`http://127.0.0.1:${server.address().port}`;await page.addStyleTag({url:origin+'/style.css'});await page.addScriptTag({url:origin+'/rules.js'});await page.addScriptTag({url:origin+'/compare.js'});
const s={name:'TS_A',stack:'/d/TS_A/stack.mrc',aligned:'/d/TS_A/aligned.mrc',quick:'/d/TS_A/tomo.mrc',result:null};
await page.evaluate(s=>{TifMrcViewPro.cryoCompare.learn([s]);fake('/d/other.mrc');},s);
await page.waitForTimeout(30);assert.equal(await page.locator('#cryo-compare').isVisible(),false,'hidden for an image of no series');
await page.evaluate(s=>TifMrcViewPro.cryoCompare.open(s,'tomo'),s);
await page.locator('#cryo-compare .cc-lab').filter({hasText:'31 / 60'}).waitFor();
assert.equal(await page.locator('#cryo-compare .cc-title').textContent(),'对齐后的倾转序列');
await page.locator('#cryo-compare .cc-slider').fill('10');await page.locator('#cryo-compare .cc-lab').filter({hasText:'11 / 60'}).waitFor();
await page.evaluate(()=>{S.z=200;});
await page.locator('#cryo-compare .cc-pic').click();
await page.locator('#cryo-compare .cc-title').filter({hasText:'快速断层图'}).waitFor();
assert.equal(await page.evaluate(()=>S.info.path),'/d/TS_A/aligned.mrc');assert.equal(await page.evaluate(()=>S.z),10,'main goes to the section the pane showed');
assert.deepEqual(await page.evaluate(()=>calls.filter(c=>c.command==='docs.open').at(-1).o),{replace:false},'swap keeps both tabs');
await page.locator('#cryo-compare .cc-lab').filter({hasText:'201 / 300'}).waitFor();
await page.locator('#cryo-compare .cc-fold').click();assert.equal(await page.locator('#cryo-compare .cc-body').isVisible(),false);await page.locator('#cryo-compare .cc-fold').click();assert.equal(await page.locator('#cryo-compare .cc-body').isVisible(),true);
await page.evaluate(()=>fake('/d/TS_A/stack.mrc'));await page.locator('#cryo-compare .cc-title').filter({hasText:'快速断层图'}).waitFor();
await page.evaluate(()=>fake('/d/other.mrc'));await page.waitForTimeout(30);assert.equal(await page.locator('#cryo-compare').isVisible(),false);
assert.equal(errors.length,0,errors.join('\n'));
console.log('PASS: pane shows the counterpart, slider, swap with section carried over, fold, raw stack matches, hides elsewhere');
}finally{await browser.close();server.close();}})().catch(e=>{console.error(e);server.close();process.exitCode=1});
