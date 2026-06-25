//! The skill sandbox.
//!
//! A skill is a WASM module that exports `alloc` (a bump allocator) and `run`. The
//! host writes the input into the guest's linear memory, calls `run(ptr, len)`, and
//! reads back a packed `(out_ptr, out_len)`. The guest can only reach the host
//! through the `engram.*` imports the [`Linker`] provides — and the linker only
//! provides the ones the skill's manifest was granted. Anything else fails to link,
//! so an over-reaching skill never starts. Every run is fuel-bounded, so a runaway
//! skill traps instead of hanging the core.

use std::sync::Arc;
use std::time::Instant;

use engram_core::Taint;
use engram_memory::{Memory, Region, WriteReq};
use wasmi::{Caller, Engine, Extern, Linker, Module, Store};

use crate::capability::Capability;
use crate::manifest::{self, ManifestError, SignedSkill};

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("wasm: {0}")]
    Wasm(String),
    #[error("abi: {0}")]
    Abi(String),
    #[error("manifest: {0}")]
    Manifest(#[from] ManifestError),
}

/// What a single run is allowed to do and with what resources.
pub struct RunCtx {
    pub granted: Vec<Capability>,
    pub memory: Option<Arc<Memory>>,
    pub regions: Vec<Region>,
    pub taint: Taint,
    pub fuel: u64,
}

impl RunCtx {
    /// A pure-compute run: no capabilities, trusted, generously fuelled.
    pub fn pure() -> Self {
        RunCtx {
            granted: vec![],
            memory: None,
            regions: vec![],
            taint: Taint::Trusted,
            fuel: 5_000_000,
        }
    }
    pub fn granted(mut self, caps: Vec<Capability>) -> Self {
        self.granted = caps;
        self
    }
    pub fn memory(mut self, m: Arc<Memory>, regions: Vec<Region>) -> Self {
        self.memory = Some(m);
        self.regions = regions;
        self
    }
    pub fn taint(mut self, t: Taint) -> Self {
        self.taint = t;
        self
    }
    pub fn fuel(mut self, f: u64) -> Self {
        self.fuel = f;
        self
    }
}

/// The result of running a skill, with the instrumentation the learning loop needs.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub output: Vec<u8>,
    pub fuel_used: u64,
    pub duration_us: u128,
    pub host_calls: u64,
    pub logs: Vec<String>,
}

/// Mutable per-run state the host functions read and write.
pub struct HostState {
    memory: Option<Arc<Memory>>,
    regions: Vec<Region>,
    taint: Taint,
    logs: Vec<String>,
    host_calls: u64,
}

/// Owns the WASM engine. One host serves many runs.
pub struct SkillHost {
    engine: Engine,
}

impl Default for SkillHost {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillHost {
    pub fn new() -> Self {
        let mut cfg = wasmi::Config::default();
        cfg.consume_fuel(true);
        SkillHost {
            engine: Engine::new(&cfg),
        }
    }

    /// Verify a signed skill against `vk`, then run it with the capabilities its
    /// manifest declares. This is the only path that should run a skill in prod:
    /// nothing unsigned, nothing with more power than it was signed for.
    pub fn run_signed(
        &self,
        signed: &SignedSkill,
        wasm: &[u8],
        vk: &ed25519_dalek::VerifyingKey,
        input: &[u8],
        ctx: RunCtx,
    ) -> Result<Outcome, SkillError> {
        manifest::verify(signed, wasm, vk)?;
        let ctx = ctx.granted(signed.manifest.capabilities.clone());
        self.run(wasm, input, ctx)
    }

    /// Run raw WASM with an explicit capability grant (used in tests and tooling).
    pub fn run(&self, wasm: &[u8], input: &[u8], ctx: RunCtx) -> Result<Outcome, SkillError> {
        let module = Module::new(&self.engine, wasm).map_err(|e| SkillError::Wasm(e.to_string()))?;

        let state = HostState {
            memory: ctx.memory.clone(),
            regions: ctx.regions.clone(),
            taint: ctx.taint,
            logs: Vec::new(),
            host_calls: 0,
        };
        let mut store = Store::new(&self.engine, state);
        store
            .set_fuel(ctx.fuel)
            .map_err(|e| SkillError::Wasm(e.to_string()))?;

        // Deny-by-default linking, with egress capabilities revoked under taint.
        let egress_revoked = ctx.taint.is_untrusted();
        let effective: Vec<Capability> = ctx
            .granted
            .iter()
            .copied()
            .filter(|c| !(egress_revoked && c.is_egress()))
            .collect();

        let mut linker = Linker::<HostState>::new(&self.engine);
        add_log(&mut linker)?;
        if effective.contains(&Capability::MemoryRead) {
            add_recall(&mut linker)?;
        }
        if effective.contains(&Capability::MemoryWrite) {
            add_remember(&mut linker)?;
        }
        if effective.contains(&Capability::Net) {
            add_net(&mut linker)?;
        }

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| SkillError::Wasm(e.to_string()))?
            .start(&mut store)
            .map_err(|e| SkillError::Wasm(e.to_string()))?;

        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| SkillError::Abi("skill exports no `memory`".into()))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "alloc")
            .map_err(|e| SkillError::Abi(format!("missing `alloc`: {e}")))?;
        let run = instance
            .get_typed_func::<(i32, i32), i64>(&store, "run")
            .map_err(|e| SkillError::Abi(format!("missing `run`: {e}")))?;

        let started = Instant::now();
        let in_ptr = alloc
            .call(&mut store, input.len() as i32)
            .map_err(|e| SkillError::Wasm(e.to_string()))?;
        memory
            .write(&mut store, in_ptr as usize, input)
            .map_err(|e| SkillError::Abi(e.to_string()))?;
        let packed = run
            .call(&mut store, (in_ptr, input.len() as i32))
            .map_err(|e| SkillError::Wasm(e.to_string()))?;
        let duration_us = started.elapsed().as_micros();

        let out_ptr = ((packed >> 32) & 0xffff_ffff) as usize;
        let out_len = (packed & 0xffff_ffff) as usize;
        let mut output = vec![0u8; out_len];
        memory
            .read(&store, out_ptr, &mut output)
            .map_err(|e| SkillError::Abi(e.to_string()))?;

        let fuel_left = store.get_fuel().unwrap_or(0);
        let st = store.data();
        Ok(Outcome {
            output,
            fuel_used: ctx.fuel.saturating_sub(fuel_left),
            duration_us,
            host_calls: st.host_calls,
            logs: st.logs.clone(),
        })
    }
}

/// Read a UTF-8 string out of guest memory; returns None if out of bounds.
fn read_str(caller: &Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let data = mem.data(caller);
    let (s, e) = (ptr as usize, ptr as usize + len.max(0) as usize);
    if e > data.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&data[s..e]).to_string())
}

fn add_log(linker: &mut Linker<HostState>) -> Result<(), SkillError> {
    linker
        .func_wrap(
            "engram",
            "log",
            |mut caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32| {
                if let Some(msg) = read_str(&caller, ptr, len) {
                    let st = caller.data_mut();
                    st.logs.push(format!("[{level}] {msg}"));
                    st.host_calls += 1;
                }
            },
        )
        .map_err(|e| SkillError::Wasm(e.to_string()))?;
    Ok(())
}

fn add_recall(linker: &mut Linker<HostState>) -> Result<(), SkillError> {
    linker
        .func_wrap(
            "engram",
            "recall",
            |mut caller: Caller<'_, HostState>,
             q_ptr: i32,
             q_len: i32,
             out_ptr: i32,
             out_cap: i32|
             -> i32 {
                let query = match read_str(&caller, q_ptr, q_len) {
                    Some(q) => q,
                    None => return -1,
                };
                let (memory, regions) = {
                    let st = caller.data();
                    (st.memory.clone(), st.regions.clone())
                };
                let Some(memory) = memory else { return -1 };
                let hits = match memory.recall(&query, &regions, 5) {
                    Ok(h) => h,
                    Err(_) => return -1,
                };
                let json = serde_json::to_vec(&hits).unwrap_or_default();
                let mem = match caller.get_export("memory") {
                    Some(Extern::Memory(m)) => m,
                    _ => return -1,
                };
                let n = json.len().min(out_cap.max(0) as usize);
                let (data, state) = mem.data_and_store_mut(&mut caller);
                let (s, e) = (out_ptr as usize, out_ptr as usize + n);
                if e > data.len() {
                    return -1;
                }
                data[s..e].copy_from_slice(&json[..n]);
                state.host_calls += 1;
                json.len() as i32
            },
        )
        .map_err(|e| SkillError::Wasm(e.to_string()))?;
    Ok(())
}

fn add_remember(linker: &mut Linker<HostState>) -> Result<(), SkillError> {
    linker
        .func_wrap(
            "engram",
            "remember",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let raw = match read_str(&caller, ptr, len) {
                    Some(s) => s,
                    None => return -1,
                };
                let parsed: serde_json::Value = match serde_json::from_str(&raw) {
                    Ok(v) => v,
                    Err(_) => return -1,
                };
                let (memory, taint) = {
                    let st = caller.data();
                    (st.memory.clone(), st.taint)
                };
                let Some(memory) = memory else { return -1 };
                let text = parsed["text"].as_str().unwrap_or("").to_string();
                if text.is_empty() {
                    return -1;
                }
                // A skill writing memory inherits the run's taint, so injected content
                // can never launder itself into a "trusted" fact.
                let req = WriteReq::new(Region::Procedural, text)
                    .taint(taint)
                    .actor("skill");
                let id = match memory.remember(req) {
                    Ok(r) => r.id,
                    Err(_) => return -1,
                };
                caller.data_mut().host_calls += 1;
                id as i32
            },
        )
        .map_err(|e| SkillError::Wasm(e.to_string()))?;
    Ok(())
}

fn add_net(linker: &mut Linker<HostState>) -> Result<(), SkillError> {
    linker
        .func_wrap(
            "engram",
            "net",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                // The Net capability is an *egress* capability. It is only ever linked
                // for a trusted run (taint revokes it before we get here), so reaching
                // this function already means the taint gate let the run through. In
                // v0.1 the call itself is a no-op stub — real outbound fetch lands with
                // the egress proxy — but the gating it demonstrates is the point.
                if let Some(url) = read_str(&caller, ptr, len) {
                    let st = caller.data_mut();
                    st.logs.push(format!("net: {url}"));
                    st.host_calls += 1;
                }
                -1
            },
        )
        .map_err(|e| SkillError::Wasm(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::Ledger;
    use engram_memory::TrigramHashEmbedder;

    // A bump allocator shared by the test skills.
    const ALLOC: &str = r#"
        (global $heap (mut i32) (i32.const 16384))
        (func (export "alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $heap))
            (global.set $heap (i32.add (global.get $heap) (local.get $n)))
            (local.get $p))"#;

    fn echo_wat() -> Vec<u8> {
        let wat = format!(
            r#"(module
                (memory (export "memory") 1)
                {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (i64.or
                        (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
                        (i64.extend_i32_u (local.get $len)))))"#
        );
        wat::parse_str(&wat).unwrap()
    }

    #[test]
    fn runs_echo_skill() {
        let host = SkillHost::new();
        let out = host.run(&echo_wat(), b"hello brain", RunCtx::pure()).unwrap();
        assert_eq!(out.output, b"hello brain");
        assert!(out.fuel_used > 0, "fuel should be consumed");
    }

    #[test]
    fn fuel_limit_traps_runaway() {
        let wat = format!(
            r#"(module
                (memory (export "memory") 1)
                {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (loop $l (br $l))
                    (i64.const 0)))"#
        );
        let wasm = wat::parse_str(&wat).unwrap();
        let host = SkillHost::new();
        let r = host.run(&wasm, b"", RunCtx::pure().fuel(50_000));
        assert!(r.is_err(), "infinite loop must exhaust fuel and trap");
    }

    #[test]
    fn deny_by_default_blocks_ungranted_import() {
        // Imports engram.recall but is granted nothing — must fail to instantiate.
        let wat = format!(
            r#"(module
                (import "engram" "recall" (func $recall (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (i64.const 0)))"#
        );
        let wasm = wat::parse_str(&wat).unwrap();
        let host = SkillHost::new();
        let r = host.run(&wasm, b"", RunCtx::pure()); // no MemoryRead granted
        assert!(r.is_err(), "ungranted import must be denied at link time");
    }

    #[test]
    fn recall_capability_reaches_memory() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Arc::new(
            Memory::open(
                dir.path().join("brain.db"),
                Arc::new(TrigramHashEmbedder::default()),
                ledger,
            )
            .unwrap(),
        );
        memory
            .remember(WriteReq::new(Region::Semantic, "the sky is blue today"))
            .unwrap();

        // Skill: treat input as the query, recall into a fixed buffer, return it.
        let wat = format!(
            r#"(module
                (import "engram" "recall" (func $recall (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 2)
                {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (local $n i32)
                    (local.set $n
                        (call $recall (local.get $ptr) (local.get $len) (i32.const 4096) (i32.const 4096)))
                    (if (i32.gt_s (local.get $n) (i32.const 4096))
                        (then (local.set $n (i32.const 4096))))
                    (if (i32.lt_s (local.get $n) (i32.const 0))
                        (then (local.set $n (i32.const 0))))
                    (i64.or
                        (i64.shl (i64.extend_i32_u (i32.const 4096)) (i64.const 32))
                        (i64.extend_i32_u (local.get $n)))))"#
        );
        let wasm = wat::parse_str(&wat).unwrap();
        let host = SkillHost::new();
        let ctx = RunCtx::pure()
            .granted(vec![Capability::MemoryRead])
            .memory(memory, vec![Region::Semantic]);
        let out = host.run(&wasm, b"sky", ctx).unwrap();
        let text = String::from_utf8_lossy(&out.output);
        assert!(text.contains("blue"), "recall should surface the stored fact, got: {text}");
        assert_eq!(out.host_calls, 1);
    }

    // A skill that imports the egress capability `engram.net`.
    fn net_skill() -> Vec<u8> {
        let wat = format!(
            r#"(module
                (import "engram" "net" (func $net (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                {ALLOC}
                (func (export "run") (param $ptr i32) (param $len i32) (result i64)
                    (drop (call $net (local.get $ptr) (local.get $len)))
                    (i64.const 0)))"#
        );
        wat::parse_str(&wat).unwrap()
    }

    #[test]
    fn taint_revokes_egress_at_the_sandbox_boundary() {
        let host = SkillHost::new();
        let wasm = net_skill();
        // Trusted run with Net granted: the egress import is satisfied, so it runs.
        let trusted = RunCtx::pure().granted(vec![Capability::Net]);
        assert!(host.run(&wasm, b"http://example.com", trusted).is_ok());

        // Untrusted run (it read web/memory content): Net is revoked, the import is
        // unsatisfied, and the skill is denied at instantiation — injection cannot
        // reach the network. This is the no-egress half of the taint rule, enforced
        // at the boundary, not by trusting the skill to behave.
        let tainted = RunCtx::pure()
            .granted(vec![Capability::Net])
            .taint(Taint::Untrusted);
        assert!(host.run(&wasm, b"http://example.com", tainted).is_err());
    }
}
