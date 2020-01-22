use cranelift_codegen::ir::Inst;
use cranelift_interpreter::environment::Environment;
use cranelift_interpreter::interpreter::Interpreter;
use cranelift_interpreter::runner::{FileRunner, FileRunnerFailure};
use cranelift_interpreter::tracing::{Trace, TracedInstruction};
use cranelift_reader::{parse_test, ParseOptions};
use std::fs::read_to_string;
use std::path::PathBuf;
use walkdir::WalkDir;

#[test]
fn filetests() {
    let _ = pretty_env_logger::try_init();
    for path in iterate_files(vec!["tests".to_string()]) {
        println!("{}:", path.to_string_lossy());
        FileRunner::from_path(path).unwrap().run().unwrap();
    }
}

/// Iterate over all of the files passed as arguments, recursively iterating through directories.
fn iterate_files(files: Vec<String>) -> impl Iterator<Item = PathBuf> {
    files
        .into_iter()
        .flat_map(WalkDir::new)
        .filter(|f| match f {
            Ok(d) => d.path().extension().filter(|&e| e.eq("clif")).is_some(),
            _ => false,
        })
        .map(|f| {
            f.expect("This should not happen: we have already filtered out the errors")
                .into_path()
        })
}

#[test]
fn tracing() {
    let _ = pretty_env_logger::try_init();

    // parse file
    let file_name = "tests/add-with-tracing.clif";
    let file_contents = read_to_string(&file_name).unwrap();
    let test = parse_test(&file_contents, ParseOptions::default())
        .map_err(|e| FileRunnerFailure::ParsingClif(file_name.to_string(), e))
        .unwrap();

    // collect functions
    let mut env = Environment::default();
    for (func, _) in test.functions.into_iter() {
        env.add(func.name.to_string(), func);
    }

    // interpret
    let interpreter = Interpreter::new(env);
    let _ = interpreter.call_by_name("%add", &[]).unwrap();

    assert_eq!(
        *interpreter.trace.borrow(),
        Trace::from(vec![
            TracedInstruction::Instruction(Inst::from_u32(1)),
            TracedInstruction::Instruction(Inst::from_u32(2)),
            TracedInstruction::Instruction(Inst::from_u32(3)),
            TracedInstruction::Instruction(Inst::from_u32(4)),
        ])
    );
}
