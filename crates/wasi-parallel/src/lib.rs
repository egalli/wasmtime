//! Implement wasi-parallel.
mod context;
mod device;
mod r#impl;
mod witx;

use anyhow::Result;
use context::WasiParallelContext;
use r#impl::{get_exported_memory, get_exported_table_function, parallel_for};
use std::{cell::RefCell, convert::TryInto};
use wasmparser::Parser;
use wasmtime::{Caller, Trap};

/// This struct solely wraps [context::WasiParallelContext] in a `RefCell`.
pub struct WasiParallel {
    pub(crate) ctx: RefCell<WasiParallelContext>,
}

impl WasiParallel {
    pub fn new(sections: Vec<KernelSection>) -> Self {
        Self {
            ctx: RefCell::new(WasiParallelContext::new(sections)),
        }
    }
}

/// Define the ways wasi-parallel can fail.
pub type WasiParallelError = anyhow::Error;

/// Re-export the Wiggle-generated `add_to_linker` function. This implements the
/// `parallel_for` call manually (see `skip` in `witx.rs`) so that we can get
/// a compiled-for-CPU version of the kernel function (`Func`).
pub fn add_to_linker<T>(
    linker: &mut wasmtime::Linker<T>,
    get_cx: impl Fn(&mut T) -> &mut WasiParallel + Send + Sync + Copy + 'static,
) -> anyhow::Result<()> {
    witx::wasi_ephemeral_parallel::add_to_linker(linker, get_cx)?;

    // This code is mostly auto-generated by
    // `wiggle_generate::wasmtime::generate_func`. It contains several
    // modifications to allow `WasiParallelContext` to get access to a runnable
    // `Func`.
    linker.func_wrap(
        "wasi_ephemeral_parallel",
        "parallel_for",
        move |mut caller: Caller<'_, T>,
              arg0: i32,
              arg1: i32,
              arg2: i32,
              arg3: i32,
              arg4: i32,
              arg5: i32,
              arg6: i32|
              -> Result<i32, Trap> {
            let mem = get_exported_memory(&mut caller)?;
            let func = get_exported_table_function(&mut caller, arg0 as u32)?;

            // Problem #1: we tell Rust that the `Caller`'s data is `()` when in
            // fact it is `T` (i.e., `HostState` if using Wasmtime). The issue
            // here is that Wiggle does not expect `T` to be `Send + Sync` (nor
            // should it) but we need it to be so in the `Send + Sync` kernel
            // closure. Since we won't use the `Caller`'s data, this actually
            // might not be too bad--it is similar to what `Store::opaque` is
            // doing.
            let caller_ref =
                unsafe { std::mem::transmute::<&Caller<'_, T>, &Caller<'_, ()>>(&caller) };

            // Problem #2: here we mutably borrow the caller in each invocation
            // of the kernel function--this is unsafe. We cannot guarantee that
            // one the various threads running the kernel concurrently will not
            // actually mutate the caller: e.g., fuel consumption, `memory.grow`,
            // `store.insert_vmexternref(...)` during trampoline setup.
            let runnable = move |params: &[_]| -> Result<Box<[_]>> {
                #[allow(mutable_transmutes)]
                let caller = unsafe {
                    std::mem::transmute::<&Caller<'_, ()>, &mut Caller<'_, ()>>(caller_ref)
                };
                func.call(caller, params)
            };
            let kernel = Box::new(runnable);

            // Here we continue using the generated Wiggle code:
            let (mem, ctx) = mem.data_and_store_mut(&mut caller);
            let ctx = get_cx(ctx);
            let mem = wiggle::wasmtime::WasmtimeGuestMemory::new(mem);
            match parallel_for(
                &mut ctx.ctx.borrow_mut(),
                &mem,
                (arg0, kernel),
                arg1,
                arg2,
                arg3,
                arg4,
                arg5,
                arg6,
            ) {
                Ok(r) => Ok(<i32>::from(r)),
                Err(wiggle::Trap::String(err)) => Err(Trap::new(err)),
                Err(wiggle::Trap::I32Exit(err)) => Err(Trap::i32_exit(err)),
            }
        },
    )?;

    Ok(())
}

pub struct KernelSection(u32, Vec<u8>);

/// Find any SPIR-V custom sections. These sections should fulfill the following
/// requirements:
/// - the name must be "wasi-parallel"
/// - the first four bytes should correspond to the table index of the Wasm
///   kernel function
/// - the remaining bytes must be the encoded SPIR-V bytes corresponding to the
///   kernel function code.
pub fn find_custom_spirv_sections(bytes: &[u8]) -> Result<Vec<KernelSection>> {
    let mut found_sections = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            wasmparser::Payload::CustomSection {
                name: "wasi-parallel",
                data,
                ..
            } => {
                let function_index = u32::from_le_bytes(data[0..4].try_into()?);
                let spirv = data[4..].to_vec();
                found_sections.push(KernelSection(function_index, spirv))
            }
            // Ignore other sections.
            _ => {}
        }
    }
    Ok(found_sections)
}