use cranelift_codegen::ir::{FuncRef, Inst, InstructionData};
use cranelift_interpreter::environment::Environment;
use cranelift_interpreter::interpreter::Interpreter;
use cranelift_interpreter::runner::FileRunnerFailure;
use cranelift_interpreter::tracing::{Trace, TracedInstruction};
use cranelift_reader::{parse_test, ParseOptions};
use std::fs::read_to_string;

#[test]
fn check_trace_produced() {
    let _ = pretty_env_logger::try_init();

    let interpreter = interpret_add_with_tracing();

    assert_eq!(
        *interpreter.trace.borrow(),
        Trace::from(vec![
            TracedInstruction::StartInFunction(FuncRef::from_u32(0)),
            TracedInstruction::Instruction(Inst::from_u32(1)),
            TracedInstruction::Instruction(Inst::from_u32(2)),
            TracedInstruction::Instruction(Inst::from_u32(3)),
            TracedInstruction::Instruction(Inst::from_u32(4)),
        ])
    );
}

#[test]
fn reconstruct_trace() {
    let _ = pretty_env_logger::try_init();

    let interpreter = interpret_add_with_tracing();

    let reconstructed_trace = interpreter
        .trace
        .borrow()
        .reconstruct(interpreter.env)
        .collect::<Vec<InstructionData>>();
    println!("{:?}", reconstructed_trace);

    // TODO need to check the contents of the reconstructed trace
    assert_eq!(4, reconstructed_trace.len())
}

/// Helper method for reducing duplication; re-factor as needed.
fn interpret_add_with_tracing() -> Interpreter {
    // parse file
    let file_name = "tests/add-with-tracing.clif";
    let file_contents = read_to_string(&file_name).unwrap();
    let test = parse_test(&file_contents, ParseOptions::default())
        .map_err(|e| FileRunnerFailure::ParsingClif(file_name.to_string(), e))
        .unwrap();

    // collect functions
    let mut env = Environment::default();
    for (func, _) in test.functions.into_iter() {
        env.add(func.name.to_string(), func.clone());
    }

    // interpret
    let interpreter = Interpreter::new(env);
    let _ = interpreter.call_by_name("%add", &[]).unwrap();
    interpreter
}
