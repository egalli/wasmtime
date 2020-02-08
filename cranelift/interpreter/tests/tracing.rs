use cranelift_codegen::ir::{FuncRef, Inst};
use cranelift_interpreter::environment::Environment;
use cranelift_interpreter::runner::FileRunner;
use cranelift_interpreter::tracing::{
    FunctionReconstructor, ReconstructedInstruction, Trace, TracedInstruction,
};

#[test]
fn check_trace_produced() {
    let _ = pretty_env_logger::try_init();

    let (_, trace) = interpret_add_with_tracing();

    assert_eq!(
        trace,
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

    let (env, trace) = interpret_add_with_tracing();

    let reconstructed_trace = trace
        .reconstruct(&env)
        .collect::<Vec<ReconstructedInstruction>>();
    println!("{:?}", reconstructed_trace);

    // TODO need to check the contents of the reconstructed trace
    assert_eq!(4, reconstructed_trace.len())
}

#[test]
fn build_function_from_trace() {
    let _ = pretty_env_logger::try_init();

    let (env, trace) = interpret_add_with_tracing();
    let function = FunctionReconstructor::new(&trace, &env).build();
    println!("{:?}", function);
}

/// Helper method for reducing duplication; re-factor as needed.
fn interpret_add_with_tracing() -> (Environment, Trace) {
    // parse file
    let file_name = "tests/add-with-tracing.clif";
    let runner = FileRunner::from_path(file_name).unwrap();
    let (env, mut traces) = runner.run().unwrap();
    (env, traces.pop().unwrap())
    // TODO call by name %add
}
