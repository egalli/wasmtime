//! This build script:
//!  - has the configuration necessary for the wiggle and witx macros
//!  - generates Wasm from the files in `tests/rust` to `tests/wasm`
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    // This is necessary for wiggle/witx macros.
    let cwd = std::env::current_dir().unwrap();
    let wasi_root = cwd.join("spec");
    println!("cargo:rustc-env=WASI_ROOT={}", wasi_root.display());

    // Also automatically rebuild if the Witx files change.
    for entry in walkdir::WalkDir::new(wasi_root) {
        println!("cargo:rerun-if-changed={}", entry.unwrap().path().display());
    }

    // Automatically rebuild any Rust tests.
    for entry in walkdir::WalkDir::new("tests/rust/") {
        let entry = entry.unwrap();
        println!("cargo:rerun-if-changed={}", entry.path().display());
        if entry.path().is_file() && entry.file_name() != "wasi_parallel.rs" {
            compile_wasm(entry.path(), "tests/wasm")
        }
    }
}

/// Use rustc to compile a Rust file to a Wasm file that uses the wasi-parallel
/// API.
fn compile_wasm<P1: AsRef<Path>, P2: AsRef<Path>>(source_file: P1, destination_dir: P2) {
    let stem = source_file.as_ref().file_stem().unwrap();
    let mut destination_file: PathBuf = [destination_dir.as_ref().as_os_str(), stem]
        .iter()
        .collect();
    destination_file.set_extension("wasm");

    let mut command = Command::new("rustc");
    command
        .arg("--target")
        .arg("wasm32-wasi")
        .arg(source_file.as_ref().to_str().unwrap())
        .arg("-o")
        .arg(destination_file.to_str().unwrap());

    let status = command
        .status()
        .expect("Failed to execute 'rustc' command to generate Wasm file.");

    assert!(
        status.success(),
        "Failed to compile test program: {:?}",
        command
    )
}
