use anyhow::{anyhow, Context as _, Result};
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use wasmtime::{Config, Engine, Linker, Module, Store, Val};
use wasmtime_cli::commands::RunCommand;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_parallel::WasiParallel;

/// Run a Wasm module as if from the Wasmtime CLI application. This is quite
/// helpful for testing and benchmarking but it is expected that users will
/// actually run the following from a shell: `wasmtime run --wasi-modules
/// experimental-wasi-parallel <MODULE>`.
#[cfg(test)]
pub fn run<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path
        .as_ref()
        .to_str()
        .context("unable to convert path to string")?;
    let command =
        RunCommand::from_iter_safe(&["run", "--wasi-modules", "experimental-wasi-parallel", path])?;
    command.execute()
}

#[derive(Default)]
struct Host {
    wasi: Option<wasmtime_wasi::WasiCtx>,
    wasi_parallel: Option<WasiParallel>,
}

pub struct BenchContext {
    store: Store<Host>,
    linker: Linker<Host>,
}

impl BenchContext {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<BenchContext> {
        let path = PathBuf::from(
            path.as_ref()
                .to_str()
                .context("unable to convert path to string")?,
        );
        pretty_env_logger::init();
        let config: Config = Config::new();
        let engine = Engine::new(&config)?;
        let mut store = Store::new(&engine, Host::default());
        let mut linker = Linker::new(&engine);

        populate_with_wasi(&mut store, &mut linker, &path)?;

        let module = Module::from_file(linker.engine(), &path)?;
        linker
            .module(&mut store, "", &module)
            .context(format!("failed to instantiate {:?}", path))?;

        let func = linker.get_default(&mut store, "")?;
        func.call(&mut store, &Vec::new())?;

        Ok(BenchContext { store, linker })
    }

    pub fn invoke(&mut self, name: &str, args: Option<Vec<Val>>) -> Result<Box<[Val]>> {
        let func = self
            .linker
            .get(&mut self.store, "", Some(name))
            .ok_or_else(|| anyhow!("no export named `{}` found", name))?
            .into_func().context(format!("export `{}` wasn't a function",name))?;
        
            func.call(&mut self.store, &args.unwrap_or(vec![]))
    }
}

fn populate_with_wasi(
    store: &mut Store<Host>,
    linker: &mut Linker<Host>,
    module_path: &PathBuf,
) -> Result<()> {
    // Add wasi-common
    wasmtime_wasi::add_to_linker(linker, |host| host.wasi.as_mut().unwrap())?;

    let builder = WasiCtxBuilder::new().inherit_stdio();

    store.data_mut().wasi = Some(builder.build());

    // Add wasi-parallel
    wasmtime_wasi_parallel::add_to_linker(linker, |host| host.wasi_parallel.as_mut().unwrap())?;
    let module_bytes = std::fs::read(module_path)?;
    let spirv_sections =
        if let Ok(sections) = wasmtime_wasi_parallel::find_custom_spirv_sections(&module_bytes) {
            sections
        } else {
            log::warn!("unable to find wasi-parallel custom sections");
            Vec::new()
        };
    store.data_mut().wasi_parallel = Some(WasiParallel::new(spirv_sections));

    Ok(())
}
