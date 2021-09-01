use anyhow::{Context, Result};
use std::path::Path;
use structopt::StructOpt;
use wasmtime_cli::commands::RunCommand;

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
