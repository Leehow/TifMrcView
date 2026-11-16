// Immutable Linux engine package embedded in desktop builds. No dependency downloads.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';
const [bin,cuda,out]=process.argv.slice(2);
if(!bin||!cuda||!out)throw Error('usage: pack-ssh-engine.mjs <Linux tmv> <cuda dir> <out.tar.gz>');
const elf=fs.readFileSync(bin);if(!elf.subarray(0,6).equals(Buffer.from([127,69,76,70,2,1]))||elf.readUInt16LE(18)!==62)throw Error('Linux x86_64 engine required');
for(const f of ['lib64/libnvrtc.so.12','lib64/libnvrtc-builtins.so.12.8','LICENSE_NVRTC'])if(!fs.statSync(path.join(cuda,f)).isFile())throw Error('Missing portable CUDA runtime: '+f);
fs.mkdirSync(path.dirname(out),{recursive:true});const stage=fs.mkdtempSync(path.join(path.dirname(out),'.ssh-engine-stage-'));
const cleanup=()=>fs.rmSync(stage,{recursive:true,force:true});for(const sig of ['SIGINT','SIGTERM','SIGHUP'])process.once(sig,()=>{cleanup();process.exit(1)});
try{
 fs.copyFileSync(bin,path.join(stage,'tmv'));fs.chmodSync(path.join(stage,'tmv'),0o700);
 fs.cpSync(cuda,path.join(stage,'cuda'),{recursive:true});fs.copyFileSync(new URL('../LICENSE',import.meta.url),path.join(stage,'LICENSE'));
 fs.mkdirSync(path.join(stage,'licenses'));fs.copyFileSync(new URL('../src-tauri/sam/SAM3-LICENSE',import.meta.url),path.join(stage,'licenses/SAM3-LICENSE'));fs.copyFileSync(new URL('../src-tauri/sam/LICENSES.md',import.meta.url),path.join(stage,'licenses/tmv-sam-LICENSES.md'));
 const files=[];function walk(dir){for(const name of fs.readdirSync(dir)){const p=path.join(dir,name);if(fs.statSync(p).isDirectory())walk(p);else files.push(path.relative(stage,p));}}walk(stage);
 fs.writeFileSync(path.join(stage,'SHA256SUMS'),files.sort().map(f=>`${crypto.createHash('sha256').update(fs.readFileSync(path.join(stage,f))).digest('hex')}  ${f}`).join('\n')+'\n');
 const temp=path.join(stage,'engine.tar.gz');const p=spawnSync('tar',['-czf',temp,'-C',stage,...files,'SHA256SUMS'],{stdio:'inherit'});if(p.status!==0)throw Error('engine archive failed');fs.renameSync(temp,out);
 console.log(`${out} ${fs.statSync(out).size} bytes`);
}finally{cleanup();}
