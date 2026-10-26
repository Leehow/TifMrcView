//! Sandbox for WebAssembly format plugins (ABI v1, `docs/format-plugins.md`).
//!
//! Plugins run in `wasmi`, a pure-Rust interpreter. A plugin sees nothing
//! but the imports below: the size and bytes of the one file it was asked
//! about, a native decompressor, and a log. Each instance has its memory
//! capped and every call gets a fuel budget, so a broken or hostile plugin
//! can at worst make one file fail to open: traps, running out of fuel and
//! malformed answers all become ordinary errors.

use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Context, Result};

/// The bytes of the file a plugin reads (usually a memory map).
pub type Bytes = Arc<dyn AsRef<[u8]> + Send + Sync>;
use wasmi::{Caller, Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

use super::recipe;

pub const ABI_VERSION: u32 = 1;
/// Linear memory per instance.
pub const MAX_MEMORY: usize = 512 << 20;
/// Fuel is counted in executed WebAssembly operators; a release build runs
/// roughly 0.5-1.3 billion per second. Opening a file or reading metadata
/// may take a few seconds at most, far more than real formats need (a CZI
/// with 2.5 MB of metadata XML uses about 0.7e9).
pub const FUEL_OPEN: u64 = 3_000_000_000;
/// Fuel for describing one page (or for small calls): well under a second.
pub const FUEL_CALL: u64 = 500_000_000;
/// Largest buffer a plugin may hand back (info JSON, recipes).
const MAX_RESULT: usize = 256 << 20;
/// Largest output of the `decompress` import.
const MAX_DECOMPRESS: usize = 256 << 20;

/// The exports every plugin must have.
const REQUIRED: &[&str] = &["memory", "tmv_abi", "tmv_alloc", "tmv_free"];
const FORMAT_EXPORTS: &[&str] = &["tmv_open", "tmv_info", "tmv_page", "tmv_close"];
const METADATA_EXPORTS: &[&str] = &["tmv_metadata"];

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut c = Config::default();
        c.consume_fuel(true);
        Engine::new(&c)
    })
}

/// What a module offers, checked when it is compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exports {
    pub format: bool,
    pub metadata: bool,
}

/// Compiles (and validates) a plugin module.
pub fn compile(bytes: &[u8]) -> Result<(Module, Exports)> {
    let module = Module::new(engine(), bytes).map_err(|e| anyhow!("WebAssembly 模块无效：{e}"))?;
    let has = |n: &str| module.get_export(n).is_some();
    for n in REQUIRED {
        if !has(n) {
            bail!("WebAssembly 模块缺少导出 {n}");
        }
    }
    for i in module.imports() {
        if i.module() != "tmv" || !IMPORTS.contains(&i.name()) {
            bail!("WebAssembly 模块需要宿主不提供的导入 {}.{}", i.module(), i.name());
        }
    }
    let format = FORMAT_EXPORTS.iter().all(|n| has(n));
    let metadata = METADATA_EXPORTS.iter().all(|n| has(n));
    if !format && !metadata {
        bail!("WebAssembly 模块既没有 tmv_open/tmv_info/tmv_page/tmv_close，也没有 tmv_metadata");
    }
    Ok((module, Exports { format, metadata }))
}

const IMPORTS: &[&str] = &["file_size", "read", "decompress", "log", "error"];

/// `TMV_PLUGIN_DEBUG=1` prints the fuel each plugin call used (for plugin authors).
fn debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TMV_PLUGIN_DEBUG").is_some_and(|v| v != "0"))
}

struct HostState {
    file: Bytes,
    /// The plugin's exported memory, set once the instance exists. Host
    /// functions read it from here instead of `Caller::get_export`, which
    /// crashed on Windows (wasmi 2.0 read an invalid instance pointer).
    memory: Option<Memory>,
    error: Option<String>,
    limits: StoreLimits,
    plugin: String,
}

/// One running plugin bound to one file. Not thread-safe by itself; the
/// caller serialises calls (see `registry::WasmSource`).
pub struct Instance {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
    inst: wasmi::Instance,
}

fn memory_of(caller: &Caller<'_, HostState>) -> Option<Memory> {
    caller.data().memory
}

/// A UTF-8 string from plugin memory, at most 4 KB.
fn string_at(caller: &Caller<'_, HostState>, ptr: u32, len: u32) -> String {
    let Some(mem) = memory_of(caller) else { return String::new() };
    let data = mem.data(caller);
    let (p, l) = (ptr as usize, (len as usize).min(4096));
    data.get(p..p.saturating_add(l)).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
}

fn linker() -> Result<Linker<HostState>> {
    let mut l = Linker::new(engine());
    l.func_wrap("tmv", "file_size", |c: Caller<'_, HostState>| -> u64 { (*c.data().file).as_ref().len() as u64 })?;
    l.func_wrap("tmv", "read", |mut c: Caller<'_, HostState>, offset: u64, len: u32, dst: u32| -> i32 {
        let Some(mem) = memory_of(&c) else { return -1 };
        let (data, state) = mem.data_and_store_mut(&mut c);
        let len = (len as usize).min(1 << 30);
        let Some(out) = data.get_mut(dst as usize..dst as usize + len) else { return -1 };
        let file = (*state.file).as_ref();
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(file.len());
        let n = len.min(file.len() - start);
        out[..n].copy_from_slice(&file[start..start + n]);
        n as i32
    })?;
    l.func_wrap(
        "tmv",
        "decompress",
        |mut c: Caller<'_, HostState>, codec: u32, src: u32, src_len: u32, dst: u32, dst_len: u32| -> i32 {
            let Some(mem) = memory_of(&c) else { return -1 };
            let data = mem.data_mut(&mut c);
            let Some(input) = data.get(src as usize..src as usize + src_len as usize) else { return -1 };
            let cap = (dst_len as usize).min(MAX_DECOMPRESS);
            if codec > u8::MAX as u32 || codec == recipe::codec::JPEG as u32 {
                return -2;
            }
            let Ok(out) = recipe::decompress(codec as u8, input, cap) else { return -3 };
            let Some(target) = data.get_mut(dst as usize..dst as usize + out.len()) else { return -1 };
            target.copy_from_slice(&out);
            out.len() as i32
        },
    )?;
    l.func_wrap("tmv", "log", |c: Caller<'_, HostState>, ptr: u32, len: u32| {
        let msg = string_at(&c, ptr, len);
        match c.get_fuel() {
            Ok(left) if debug() => eprintln!("[plugin {}] {msg} ({left} fuel left)", c.data().plugin),
            _ => eprintln!("[plugin {}] {msg}", c.data().plugin),
        }
    })?;
    l.func_wrap("tmv", "error", |mut c: Caller<'_, HostState>, ptr: u32, len: u32| {
        let msg = string_at(&c, ptr, len);
        c.data_mut().error = Some(msg);
    })?;
    Ok(l)
}

impl Instance {
    /// Instantiates `module` for reading `file`. `plugin` names it in messages.
    pub fn new(module: &Module, file: Bytes, plugin: &str) -> Result<Instance> {
        let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY).memories(1).tables(4).instances(1).build();
        let mut store = Store::new(engine(), HostState { file, memory: None, error: None, limits, plugin: plugin.into() });
        store.limiter(|s| &mut s.limits);
        store.set_fuel(FUEL_CALL)?;
        let inst = linker()?.instantiate_and_start(&mut store, module).map_err(|e| trap(e, plugin, None))?;
        let memory = inst.get_memory(&store, "memory").context("插件没有导出 memory")?;
        store.data_mut().memory = Some(memory);
        let alloc = inst.get_typed_func(&store, "tmv_alloc").context("插件的 tmv_alloc 签名不对")?;
        let free = inst.get_typed_func(&store, "tmv_free").context("插件的 tmv_free 签名不对")?;
        let mut me = Instance { store, memory, alloc, free, inst };
        let abi: u32 = me.call("tmv_abi", (), FUEL_CALL)?;
        if abi != ABI_VERSION {
            bail!("插件接口版本是 {abi}，这个版本的 TifMrcView 只支持 {ABI_VERSION}，请更新插件或 TifMrcView");
        }
        Ok(me)
    }

    fn call<P: wasmi::WasmParams, R: wasmi::WasmResults>(&mut self, name: &str, args: P, fuel: u64) -> Result<R> {
        let f = self
            .inst
            .get_typed_func::<P, R>(&self.store, name)
            .map_err(|_| anyhow!("插件没有导出 {name}，或签名不对"))?;
        self.store.set_fuel(fuel)?;
        self.store.data_mut().error = None;
        let r = f.call(&mut self.store, args);
        if debug() {
            let used = fuel - self.store.get_fuel().unwrap_or(0);
            eprintln!("[plugin {}] {name}: {used} fuel", self.store.data().plugin);
        }
        r.map_err(|e| {
            let msg = self.store.data_mut().error.take();
            trap(e, &self.store.data().plugin, msg)
        })
    }

    /// The error the plugin reported for the call that just failed.
    fn reported(&mut self, what: &str) -> anyhow::Error {
        let msg = self.store.data_mut().error.take();
        match msg {
            Some(m) if !m.is_empty() => anyhow!(m),
            _ => anyhow!("插件 {} 无法{what}", self.store.data().plugin),
        }
    }

    /// Copies a string into plugin memory; returns (ptr, len).
    fn put(&mut self, s: &str) -> Result<(u32, u32)> {
        let len = u32::try_from(s.len()).context("路径太长")?;
        let ptr: u32 = self.alloc.call(&mut self.store, len).map_err(|e| trap(e, "?", None))?;
        if ptr == 0 {
            bail!("插件内存不足");
        }
        self.memory.write(&mut self.store, ptr as usize, s.as_bytes()).map_err(|_| anyhow!("插件内存越界"))?;
        Ok((ptr, len))
    }

    fn release(&mut self, (ptr, len): (u32, u32)) {
        let _ = self.store.set_fuel(FUEL_CALL);
        let _ = self.free.call(&mut self.store, (ptr, len));
    }

    /// Copies out a buffer returned as `ptr | len << 32`.
    fn take(&mut self, packed: u64) -> Result<Vec<u8>> {
        let (ptr, len) = ((packed & 0xffff_ffff) as usize, (packed >> 32) as usize);
        if len > MAX_RESULT {
            bail!("插件返回的数据太大");
        }
        self.memory
            .data(&self.store)
            .get(ptr..ptr + len)
            .map(<[u8]>::to_vec)
            .context("插件返回的数据位置越界")
    }

    pub fn open(&mut self, path: &str) -> Result<i32> {
        let arg = self.put(path)?;
        let r = self.call::<(u32, u32), i32>("tmv_open", arg, FUEL_OPEN);
        self.release(arg);
        match r? {
            h if h >= 0 => Ok(h),
            _ => Err(self.reported("打开这个文件")),
        }
    }

    pub fn info(&mut self, handle: i32) -> Result<Vec<u8>> {
        match self.call::<i32, u64>("tmv_info", handle, FUEL_OPEN)? {
            0 => Err(self.reported("读取图像信息")),
            p => self.take(p),
        }
    }

    pub fn page(&mut self, handle: i32, page: u32) -> Result<Vec<u8>> {
        match self.call::<(i32, u32), u64>("tmv_page", (handle, page), FUEL_CALL)? {
            0 => Err(self.reported("读取这一页")),
            p => self.take(p),
        }
    }

    pub fn close(&mut self, handle: i32) {
        let _ = self.call::<i32, ()>("tmv_close", handle, FUEL_CALL);
    }

    pub fn metadata(&mut self, path: &str) -> Result<Vec<u8>> {
        let arg = self.put(path)?;
        let r = self.call::<(u32, u32), u64>("tmv_metadata", arg, FUEL_OPEN);
        self.release(arg);
        match r? {
            0 => Err(self.reported("读取元数据")),
            p => self.take(p),
        }
    }
}

/// Turns a wasm error into a message for the user.
fn trap(e: wasmi::Error, plugin: &str, reported: Option<String>) -> anyhow::Error {
    if e.as_trap_code() == Some(wasmi::TrapCode::OutOfFuel) || e.to_string().contains("fuel") {
        return anyhow!("插件 {plugin} 运行超时，已中止");
    }
    match reported {
        Some(m) if !m.is_empty() => anyhow!("{m}"),
        _ => anyhow!("插件 {plugin} 出错：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembles a module from hand-written bytecode (no `wat` dependency).
    pub(crate) mod asm {
        fn leb(mut v: u64, out: &mut Vec<u8>) {
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        fn section(id: u8, body: Vec<u8>, out: &mut Vec<u8>) {
            out.push(id);
            leb(body.len() as u64, out);
            out.extend(body);
        }
        fn name(s: &str, out: &mut Vec<u8>) {
            leb(s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        pub const I32: u8 = 0x7f;
        pub const I64: u8 = 0x7e;

        /// `funcs`: (export name, params, results, body without locals/end).
        /// (export name, params, results, body without locals/end)
        pub type Func<'a> = (&'a str, &'a [u8], &'a [u8], &'a [u8]);

        pub fn module(funcs: &[Func]) -> Vec<u8> {
            let mut m = b"\0asm\x01\0\0\0".to_vec();
            let mut types = vec![];
            leb(funcs.len() as u64, &mut types);
            for (_, p, r, _) in funcs {
                types.push(0x60);
                leb(p.len() as u64, &mut types);
                types.extend_from_slice(p);
                leb(r.len() as u64, &mut types);
                types.extend_from_slice(r);
            }
            section(1, types, &mut m);
            let mut f = vec![];
            leb(funcs.len() as u64, &mut f);
            for i in 0..funcs.len() {
                leb(i as u64, &mut f);
            }
            section(3, f, &mut m);
            // One memory: 1 page min, no max.
            section(5, vec![1, 0, 1], &mut m);
            let mut ex = vec![];
            leb(funcs.len() as u64 + 1, &mut ex);
            name("memory", &mut ex);
            ex.extend([2, 0]);
            for (i, (n, ..)) in funcs.iter().enumerate() {
                name(n, &mut ex);
                ex.push(0);
                leb(i as u64, &mut ex);
            }
            section(7, ex, &mut m);
            let mut code = vec![];
            leb(funcs.len() as u64, &mut code);
            for (.., body) in funcs {
                let mut b = vec![0]; // no locals
                b.extend_from_slice(body);
                b.push(0x0b);
                leb(b.len() as u64, &mut code);
                code.extend(b);
            }
            section(10, code, &mut m);
            m
        }

        /// The ABI basics plus the given extra functions.
        pub fn plugin(extra: &[Func]) -> Vec<u8> {
            let mut f: Vec<Func> = vec![
                ("tmv_abi", &[], &[I32], &[0x41, 1]),            // i32.const 1
                ("tmv_alloc", &[I32], &[I32], &[0x41, 0x80, 8]), // i32.const 1024
                ("tmv_free", &[I32, I32], &[], &[]),
            ];
            f.extend_from_slice(extra);
            module(&f)
        }
    }

    fn file() -> Bytes {
        Arc::new(b"hello".to_vec())
    }

    #[test]
    fn rejects_modules_without_the_abi() {
        assert!(compile(b"not wasm").is_err());
        let m = asm::module(&[("tmv_abi", &[], &[asm::I32], &[0x41, 1])]);
        assert!(format!("{:#}", compile(&m).unwrap_err()).contains("缺少导出"));
        // The basics but neither format nor metadata exports.
        assert!(compile(&asm::plugin(&[])).is_err());
    }

    #[test]
    fn infinite_loop_runs_out_of_fuel() {
        // loop { br 0 } then unreachable.
        let spin: &[u8] = &[0x03, 0x40, 0x0c, 0x00, 0x0b, 0x00];
        let m = asm::plugin(&[("tmv_metadata", &[asm::I32, asm::I32], &[asm::I64], spin)]);
        let (module, ex) = compile(&m).unwrap();
        assert_eq!(ex, Exports { format: false, metadata: true });
        let mut inst = Instance::new(&module, file(), "spin").unwrap();
        // A smaller budget keeps the test quick; the mechanism is the same.
        let arg = inst.put("x").unwrap();
        let t = std::time::Instant::now();
        let err = inst.call::<(u32, u32), u64>("tmv_metadata", arg, 50_000_000).unwrap_err();
        assert!(format!("{err:#}").contains("超时"), "{err:#}");
        eprintln!("50M fuel took {:?}", t.elapsed());
    }

    #[test]
    fn traps_and_bad_results_are_errors() {
        let m = asm::plugin(&[
            // unreachable
            ("tmv_metadata", &[asm::I32, asm::I32], &[asm::I64], &[0x00]),
        ]);
        let (module, _) = compile(&m).unwrap();
        let mut inst = Instance::new(&module, file(), "t").unwrap();
        assert!(inst.metadata("a").is_err());

        // Returns ptr=0xffff_0000, len=16: outside the 64 KiB memory.
        let bad: &[u8] = &[0x42, 0x80, 0x80, 0xfc, 0xff, 0x8f, 0x02];
        let m = asm::plugin(&[("tmv_metadata", &[asm::I32, asm::I32], &[asm::I64], bad)]);
        let (module, _) = compile(&m).unwrap();
        let mut inst = Instance::new(&module, file(), "t").unwrap();
        assert!(format!("{:#}", inst.metadata("a").unwrap_err()).contains("越界"));
    }

    #[test]
    fn wrong_abi_version_is_refused() {
        let m = asm::module(&[
            ("tmv_abi", &[], &[asm::I32], &[0x41, 2]),
            ("tmv_alloc", &[asm::I32], &[asm::I32], &[0x41, 0]),
            ("tmv_free", &[asm::I32, asm::I32], &[], &[]),
            ("tmv_metadata", &[asm::I32, asm::I32], &[asm::I64], &[0x42, 0]),
        ]);
        let (module, _) = compile(&m).unwrap();
        assert!(format!("{:#}", Instance::new(&module, file(), "t").err().unwrap()).contains("接口版本"));
    }
}
