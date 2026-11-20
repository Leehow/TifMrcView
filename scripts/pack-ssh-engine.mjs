// Immutable Linux engine package for Pro's SSH processing: published beside the
// installers (make-release.mjs) and downloaded by Pro on first use (build.rs pins
// its SHA-256). Nothing is downloaded on the server.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';
const [bin,cuda,out]=process.argv.slice(2);
if(!bin||!cuda||!out)throw Error('usage: pack-ssh-engine.mjs <Linux tmv> <cuda dir> <out.tar.gz>');
const elf=fs.readFileSync(bin);if(!elf.subarray(0,6).equals(Buffer.from([127,69,76,70,2,1]))||elf.readUInt16LE(18)!==62)throw Error('Linux x86_64 engine required');
// Only these: the CUDA folder also holds build leftovers, such as libdevice.mjs's
// 81 MB nvcc download, which once went into every Pro installer.
const CUDA_FILES=['lib64/libnvrtc.so.12','lib64/libnvrtc-builtins.so.12.8','LICENSE_NVRTC','nvvm/libdevice/libdevice.10.bc','LICENSE'];
for(const f of CUDA_FILES)if(!fs.statSync(path.join(cuda,f)).isFile())throw Error('Missing portable CUDA runtime: '+f);
fs.mkdirSync(path.dirname(out),{recursive:true});const stage=fs.mkdtempSync(path.join(path.dirname(out),'.ssh-engine-stage-'));
const cleanup=()=>fs.rmSync(stage,{recursive:true,force:true});for(const sig of ['SIGINT','SIGTERM','SIGHUP'])process.once(sig,()=>{cleanup();process.exit(1)});
try{
 fs.copyFileSync(bin,path.join(stage,'tmv'));fs.chmodSync(path.join(stage,'tmv'),0o700);
 for(const f of CUDA_FILES){fs.mkdirSync(path.dirname(path.join(stage,'cuda',f)),{recursive:true});fs.copyFileSync(path.join(cuda,f),path.join(stage,'cuda',f));}fs.copyFileSync(new URL('../LICENSE',import.meta.url),path.join(stage,'LICENSE'));
 fs.mkdirSync(path.join(stage,'licenses'));fs.copyFileSync(new URL('../src-tauri/sam/SAM3-LICENSE',import.meta.url),path.join(stage,'licenses/SAM3-LICENSE'));fs.copyFileSync(new URL('../src-tauri/sam/LICENSES.md',import.meta.url),path.join(stage,'licenses/tmv-sam-LICENSES.md'));
 const files=[];function walk(dir){for(const name of fs.readdirSync(dir)){const p=path.join(dir,name);if(fs.statSync(p).isDirectory())walk(p);else files.push(path.relative(stage,p));}}walk(stage);
 fs.writeFileSync(path.join(stage,'SHA256SUMS'),files.sort().map(f=>`${crypto.createHash('sha256').update(fs.readFileSync(path.join(stage,f))).digest('hex')}  ${f}`).join('\n')+'\n');
 const temp=path.join(stage,'engine.tar.gz');const p=spawnSync('tar',['-czf',temp,'-C',stage,...files,'SHA256SUMS'],{stdio:'inherit'});if(p.status!==0)throw Error('engine archive failed');fs.renameSync(temp,out);
 console.log(`${out} ${fs.statSync(out).size} bytes`);
}finally{cleanup();}
