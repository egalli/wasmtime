//! Implements the tracing functionality for the Cranelift interpreter.
//!
//! This tracing module adds the ability to trace sequences of instructions interpreted by
//! [Interpreter], an interpreter of Cranelift IR. The trace can be compiled and run in lieu of the
//! original Cranelift IR; (TODO) this is currently not completely true due to missing instruction
//! implementations and no memory support, but is the eventual goal.
//!
//! Using this tracing currently looks like the following:
//! - Annotate Cranelift with the new `trace_start` and `trace_end` instructions; `trace_start` has
//!   an ID field (stored as an immediate, `imm`) to distinguish between traces.
//! - Interpret the Cranelift IR with the [Interpreter] (e.g. see `test/filetests.rs`).
//! - When the [Interpreter] encounters a `trace_start` instruction (and no compiled trace for its
//!   ID), tracing is turned on. This state change is implemented in the `TraceStart` arm of
//!   [Interpreter::inst].
//! - For each subsequent instruction, the [Interpreter] appends the instruction to a `Trace`.
//! - When a `trace_end` is interpreted (see `TraceEnd` in [Interpreter::inst]), the trace is:
//!   > reconstructed by `FunctionReconstructor`: this renumbers all SSA values and eliminates
//!     jumps and function calls.
//!   > compiled by the Cranelift compiler
//!   > added to the `TraceStore` for the given trace ID
//! - The [Interpreter] continues interpreting the instructions after the `trace_end`.
//! - If the [Interpreter] again interprets the same `trace_start id` pair, it now uses the
//!   compiled version of the trace it extracts from the `TraceStore`.

use crate::environment::Environment;
use crate::interpreter::function_name_of_func_ref;
use cranelift_codegen::ir::{
    AbiParam, Block, FuncRef, Function, Inst, InstructionData, Opcode, Value, ValueList,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings;
use cranelift_filetests::{CompiledCode, FunctionRunner};
use cranelift_native::builder as host_isa_builder;
use cranelift_value::Value as BoxedValue;
use log::debug;
use std::collections::HashMap;
use thiserror::Error;

/// The ways tracing can fail.
#[derive(Error, Debug)]
pub enum TraceError {
    #[error("trace compilation failed: {0}")]
    CompilationFailed(String),
    #[error("trace execution failed: {0}")]
    ExecutionFailed(String),
}

#[derive(Debug, PartialEq, Clone)]
pub enum TracedInstruction {
    // TODO need to decide between storing a FuncRef here or a &'a Function:
    // - a FuncRef means we have to add FuncRef to Frame (or reverse lookup) and pass around an
    // Environment; plus some overhead from lookup in the Function in Environment
    // - a &'a Function means we have larger TracedInstructions (64bit pointer) and may have to
    // deal with borrowing issues (the Frame lifetime will likely not outlive the Trace lifetime
    // and we are borrowing &Function from Frame to Trace)
    StartInFunction(FuncRef),
    EnterFunction(FuncRef), // TODO this may not be very helpful if we already trace call/returns
    ExitFunction,           // TODO this may not be very helpful if we already trace call/returns
    Instruction(Inst),
    Guard(Inst),
    //Loop,
}

#[derive(Default, Debug, PartialEq)]
pub struct Trace {
    tracing: Option<i64>,
    observed: Vec<TracedInstruction>,
}

impl Trace {
    pub fn start(&mut self, id: i64, func_ref: FuncRef) {
        assert!(self.tracing.is_none(), "should not be currently tracing");
        self.tracing.replace(id);
        self.observed
            .push(TracedInstruction::StartInFunction(func_ref))
    }

    pub fn end(&mut self) -> i64 {
        self.tracing
            .take()
            .expect("tracing should have been started")
    }

    pub fn observe(&mut self, observed: TracedInstruction) {
        if self.tracing.is_some() {
            self.observed.push(observed)
        }
    }

    pub fn len(&self) -> usize {
        self.observed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.observed.is_empty()
    }

    pub fn remove_last(&mut self) -> Option<TracedInstruction> {
        self.observed.pop()
    }

    pub fn reconstruct<'a>(&self, env: &'a Environment) -> TraceIterator<'a> {
        TraceIterator {
            cursor: 0,
            stack: vec![],
            trace: self.observed.to_vec(), // TODO avoid cloning
            env,
        }
    }
}

impl From<Vec<TracedInstruction>> for Trace {
    fn from(observed: Vec<TracedInstruction>) -> Self {
        Self {
            observed,
            ..Default::default()
        }
    }
}

pub struct TraceIterator<'a> {
    cursor: usize,
    stack: Vec<FuncRef>,
    trace: Vec<TracedInstruction>, // TODO make a reference?
    env: &'a Environment,
}

#[derive(Debug)]
pub struct ReconstructedInstruction {
    results: Vec<Value>,
    instruction: InstructionData,
}

impl<'a> Iterator for TraceIterator<'a> {
    type Item = ReconstructedInstruction;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.trace.len() {
            let inst = &self.trace[self.cursor];
            self.cursor += 1;
            match inst {
                TracedInstruction::StartInFunction(f) | TracedInstruction::EnterFunction(f) => {
                    self.stack.push(*f);
                }
                TracedInstruction::ExitFunction => {
                    self.stack.pop();
                }
                TracedInstruction::Instruction(i) => {
                    let func_ref = self
                        .stack
                        .last()
                        .expect("the stack to contain a valid func ref");
                    let func = self
                        .env
                        .get_by_func_ref(*func_ref)
                        .expect("the environment to contain a valid function");
                    let instruction = func.dfg[*i].clone();
                    let results = func.dfg.inst_results(*i).to_vec();
                    return Some(ReconstructedInstruction {
                        results,
                        instruction,
                    });
                }
                TracedInstruction::Guard(_) => unimplemented!(),
            }
            continue;
        }
        None
    }
}

/// Helpful reconstruction type aliases.
type OldValue = Value;
type NewValue = Value;
type CallResults = Vec<Value>;
type StackFrame = (FuncRef, Renumberings, CallResults);

/// Track the mapping of old SSA values to new SSA values.
///
/// Because of how `Function` tracks its values, we must renumber all of the SSA values in the
/// instructions it contains. This struct maintains the mapping between old and new SSA values.
#[derive(Debug)]
struct Renumberings(HashMap<OldValue, NewValue>);

impl Renumberings {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn get(&self, old: &OldValue) -> Option<&NewValue> {
        self.0.get(old)
    }

    fn renumber(&mut self, old: OldValue, new: NewValue) {
        debug!("Renumbering {} to {}", old, new);
        self.0.insert(old, new);
    }

    fn iter(&self) -> impl Iterator<Item = (&OldValue, &NewValue)> {
        self.0.iter()
    }
}

/// Reconstruct a `Trace` into a `Function`.
///
/// At first glance, this may seem simple: just append the instructions from the trace into the
/// function and be done. Due to `Function`'s implementation, however, we run into issues with SSA
/// `Value`s that are duplicated or non-existent. Initially I thought that I could use the original
/// `Value` numbering and use aliases (see `DataFlowGraph`) to avoid duplication and handle
/// calls/jumps; I don't think this will work because there simply can be no duplicates in
/// `DataFlowGraph::values`.
///
/// Instead, I need to renumber all instruction arguments and results. There are four requirements
/// for this to work:
///  - requirement 1: renumber instruction results and record the mapping from old `Value` to new
/// `Value`
///  - requirement 2: when we call/jump/branch, record the mapping from caller-side arguments to
/// callee-side arguments (but only for the duration of the call/jump/branch -- TODO why?)
///  - requirement 3: when we return, record the mapping of the caller result values to the
/// return values (this implies that we need to keep the caller results around until we observe a
/// return).
///  - requirement 4: renumber instruction arguments using either the mapping from requirement 1 or
/// requirement 2; any non-mapped arguments should be inputs to the trace function (TODO correct?)
///
pub struct FunctionReconstructor<'a> {
    inputs: Renumberings,
    stack: Vec<StackFrame>,
    trace: &'a Trace,
    env: &'a Environment,
}

impl<'a> FunctionReconstructor<'a> {
    pub fn new(trace: &'a Trace, env: &'a Environment) -> Self {
        Self {
            inputs: Renumberings::new(),
            stack: Vec::new(),
            trace,
            env,
        }
    }

    pub fn build(&mut self) -> Function {
        let mut new_func = Function::new();

        // Use the host system's calling convention.
        let flags = settings::Flags::new(settings::builder());
        let builder = host_isa_builder().expect("Unable to build a TargetIsa for the current host");
        let isa = builder.finish(flags);
        new_func.signature.call_conv = isa.default_call_conv();

        // Add the initial block for the reconstructed trace function (there may be more later to
        // handle guards).
        let block = new_func.dfg.make_block();
        new_func.layout.append_block(block);

        // Set up the stack using the first meta-instruction.
        if let TracedInstruction::StartInFunction(func_ref) = self.trace.observed[0] {
            self.setup(func_ref)
        } else {
            panic!(
                "we expect the first instruction of the trace is expected to be a StartInFunction"
            );
        }

        // Iterate over the remaining meta-instructions, building the function as we go.
        for i in &self.trace.observed[1..] {
            match i {
                TracedInstruction::StartInFunction(_) => {
                    panic!("found another StartInFunction in the trace")
                }
                TracedInstruction::EnterFunction(_) | TracedInstruction::ExitFunction => {
                    /* Ignore, we handle calls/returns using the InstructionData directly */
                }
                TracedInstruction::Instruction(i) => {
                    self.reconstruct_instruction(*i, &mut new_func, block);
                }
                TracedInstruction::Guard(_) => { /* TODO */ }
            }
        }

        // Return from the function. TODO need to figure out which values to return if necessary
        let return_inst = new_func.dfg.make_inst(InstructionData::MultiAry {
            opcode: Opcode::Return,
            args: ValueList::new(),
        });
        new_func.layout.append_inst(return_inst, block);

        // Add the discovered trace inputs (free SSA values) to the function signature.
        for (_, new_arg) in self.inputs.iter() {
            let ty = new_func.dfg.value_type(*new_arg);
            let param = AbiParam::new(ty);
            new_func.signature.params.push(param);
        }

        new_func
    }

    fn setup(&mut self, func_ref: FuncRef) {
        self.stack
            .push((func_ref, Renumberings::new(), CallResults::new()));
    }

    fn reconstruct_instruction(&mut self, inst: Inst, new_func: &mut Function, block: Block) {
        //let old_func = self.old_func();
        let inst_data = self.old_func().dfg[inst].clone();
        match inst_data {
            InstructionData::Call {
                args,
                func_ref: sig_ref,
                ..
            } => {
                // partially fulfill requirement 2
                let caller_results = self.old_func().dfg.inst_results(inst).to_vec();
                let caller_args = args.as_slice(&self.old_func().dfg.value_lists);
                let callee_name = function_name_of_func_ref(sig_ref, &self.old_func());
                // TODO there must be a better way to do this than string comparison
                let callee_func_ref = self
                    .env
                    .get_func_ref_by_name(&callee_name)
                    .expect("a function with this name");
                let callee_func = self
                    .env
                    .get_by_func_ref(callee_func_ref)
                    .expect("a function with this index");
                let callee_block = callee_func
                    .layout
                    .blocks()
                    .next()
                    .expect("to have a first block");
                let callee_args = callee_func.dfg.block_params(callee_block);
                debug_assert_eq!(caller_args.len(), callee_args.len());

                // TODO deduplicate this with jump
                let mut local_renumbering: Renumberings = Renumberings::new();
                for (old_arg, new_arg) in caller_args.iter().zip(callee_args) {
                    if let Some(renumbered_arg) = self.current_renumbering().get(old_arg) {
                        local_renumbering.renumber(*new_arg, *renumbered_arg);
                    } else if let Some(renumbered_arg) = self.inputs.get(old_arg) {
                        assert_eq!(
                            self.stack.len(),
                            1,
                            "we only need input numberings if we are in the first stack frame"
                        );
                        local_renumbering.renumber(*new_arg, *renumbered_arg);
                    } else {
                        panic!("don't yet know what to do here");
                    }
                }

                self.stack
                    .push((callee_func_ref, local_renumbering, caller_results));
            }
            InstructionData::Jump {
                destination, args, ..
            } => {
                // partially fulfill requirement 2
                // we need to figure out what the jump renumbers the args as and use this
                // in our renumbering
                let (func_ref, old_renumbering, caller_results) =
                    self.stack.pop().expect("to have something on the stack");
                let caller_args = args.as_slice(&self.func(func_ref).dfg.value_lists);
                let destination_args = self.func(func_ref).dfg.block_params(destination);
                debug_assert_eq!(caller_args.len(), destination_args.len());

                let mut new_renumbering: Renumberings = Renumberings::new();
                for (old_arg, new_arg) in caller_args.iter().zip(destination_args) {
                    if let Some(renumbered_arg) = old_renumbering.get(old_arg) {
                        new_renumbering.renumber(*new_arg, *renumbered_arg);
                    } else if let Some(renumbered_arg) = self.inputs.get(old_arg) {
                        new_renumbering.renumber(*new_arg, *renumbered_arg);
                    } else {
                        panic!("don't yet know what to do here");
                    }
                }

                // replace local renumbering with the renumbering from the current block
                self.stack.push((func_ref, new_renumbering, caller_results));
            }
            InstructionData::MultiAry {
                opcode: Opcode::Return,
                args,
            } => {
                // fulfill requirement 3
                let (func_ref, mut caller_renumbering, caller_results) =
                    self.stack.pop().expect("...");
                let callee_returns = args.as_slice(&self.func(func_ref).dfg.value_lists);
                for (old_value, new_value) in caller_results.iter().zip(callee_returns) {
                    caller_renumbering.renumber(*old_value, *new_value);
                }
            }
            _ => {
                // This instruction must be appended so we fixup its SSA values
                let new_inst = new_func.dfg.make_inst(inst_data);

                // First, the instruction results:
                let results = self.old_func().dfg.inst_results(inst).to_vec(); // TODO avoid allocation
                for old_result in results {
                    // fulfill requirement 1
                    let old_type = self.old_func().dfg.value_type(old_result);
                    let new_result = new_func.dfg.append_result(new_inst, old_type);
                    self.current_renumbering_mut()
                        .renumber(old_result, new_result);
                }

                // Then, the instruction arguments:
                let old_args = new_func.dfg.inst_args(new_inst).to_vec(); // TODO avoid allocation
                let new_args = old_args
                    .iter()
                    .map(|&old_arg| {
                        match self.find_argument(old_arg) {
                            None => {
                                // If we have never observed this SSA value, add it as a free input.
                                let ty = self.old_func().dfg.value_type(old_arg);
                                let new_arg = new_func.dfg.append_block_param(block, ty);
                                self.inputs.renumber(old_arg, new_arg);
                                new_arg
                            }
                            Some(a) => a,
                        }
                    })
                    .collect::<Vec<NewValue>>();
                // Now we can actually replace the arguments:
                for (old_arg, new_arg) in new_func
                    .dfg
                    .inst_args_mut(new_inst)
                    .iter_mut()
                    .zip(new_args)
                {
                    *old_arg = new_arg;
                }

                // Finally, append the instruction to the reconstructed function.
                new_func.layout.append_inst(new_inst, block);
                debug!(
                    "Appending instruction: {}",
                    new_func.dfg.display_inst(new_inst, None)
                );
            }
        }
    }

    fn func(&self, func_ref: FuncRef) -> &Function {
        self.env
            .get_by_func_ref(func_ref)
            .expect("the environment to contain a valid function")
    }

    fn old_func(&self) -> &Function {
        let func_ref = self
            .stack
            .last()
            .expect("there must be at least one stack frame pushed")
            .0;
        self.func(func_ref)
    }

    fn current_renumbering(&self) -> &Renumberings {
        &self
            .stack
            .last()
            .expect("there must be at least one stack frame pushed")
            .1
    }

    fn current_renumbering_mut(&mut self) -> &mut Renumberings {
        &mut self
            .stack
            .last_mut()
            .expect("there must be at least one stack frame pushed")
            .1
    }

    fn find_argument(&self, old_arg: OldValue) -> Option<NewValue> {
        if let Some(new_arg) = self.current_renumbering().get(&old_arg) {
            Some(*new_arg)
        } else if let Some(new_arg) = self.inputs.get(&old_arg) {
            Some(*new_arg)
        } else {
            None
        }
    }
}

/// Find the host machine's default calling convention.
fn host_calling_convention() -> CallConv {
    let flags = settings::Flags::new(settings::builder());
    let builder = host_isa_builder().expect("Unable to build a TargetIsa for the current host");
    let isa = builder.finish(flags);
    isa.default_call_conv()
}

#[derive(Default)]
pub struct TraceStore(HashMap<i64, CompiledCode>);

impl TraceStore {
    /// Check if the trace exists in the environment.
    pub fn contains(&self, id: i64) -> bool {
        self.0.contains_key(&id)
    }

    /// Add a trace.
    pub fn insert(&mut self, id: i64, code: CompiledCode) {
        self.0.insert(id, code);
    }

    /// Execute the compiled trace.
    pub fn execute(&self, id: i64, args: &[BoxedValue]) -> Result<BoxedValue, TraceError> {
        let code = self.0.get(&id).expect("trace must exist");
        FunctionRunner::execute(code.as_slice(), args).map_err(|e| TraceError::ExecutionFailed(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::FileRunner;
    use cranelift_reader::{parse_test, ParseOptions};

    #[test]
    fn size_of() {
        assert_eq!(std::mem::size_of::<TracedInstruction>(), 8);
    }

    fn interpret(clif: &str) -> (Environment, Trace) {
        let runner = FileRunner::from_inline_code(clif.to_string());
        let (env, mut traces) = runner.run().unwrap();
        (env, traces.pop().unwrap())
    }

    fn parse(clif: &str) -> Function {
        let test_file = parse_test(clif, ParseOptions::default()).unwrap();
        assert_eq!(1, test_file.functions.len());
        let mut function = test_file.functions[0].0.clone(); // the first function
        function.signature.call_conv = host_calling_convention();
        function
    }

    fn to_function(trace: Trace, environment: Environment) -> Function {
        FunctionReconstructor::new(&trace, &environment).build()
    }

    #[test]
    fn reconstruct_single_block() {
        let _ = pretty_env_logger::try_init();
        let (env, trace) = interpret(
            "
            function %test(i32, i32) -> i32 {
            block0(v43: i32, v44: i32):
                trace_start 99
                v42 = iadd.i32 v43, v44
                v45 = isub.i32 v42, v43
                return v45
            }
           ; run: %test(1, 2)",
        );

        let reconstructed = to_function(trace, env);
        let expected = parse(
            "
            function u0:0(i32, i32) {
            block0(v1: i32, v2: i32):
                v0 = iadd.i32 v1, v2
                v3 = isub.i32 v0, v1
                return
            }",
        );

        assert_eq!(
            reconstructed.display(None).to_string(),
            expected.display(None).to_string()
        );
    }

    #[test]
    fn reconstruct_multiple_blocks() {
        let _ = pretty_env_logger::try_init();
        let (env, trace) = interpret(
            "
            function %mul3(i32) {
            block0(v20: i32):
                trace_start 99
                v21 = iadd.i32 v20, v20
                fallthrough block1(v20, v21)
            block1(v10: i32, v11: i32):
                v12 = iadd.i32 v11, v10
                fallthrough block2(v10, v12)
            block2(v0: i32, v1: i32):
                v2 = iadd.i32 v1, v0
                trace_end 99
                return
            }
           ; run: %mul3(5)",
        );

        let reconstructed = to_function(trace, env);
        let expected = parse(
            "
            function u0:0(i32) {
            block0(v1: i32):
                v0 = iadd.i32 v1, v1
                v2 = iadd.i32 v0, v1
                v3 = iadd.i32 v2, v1
                return
            }",
        );

        assert_eq!(
            reconstructed.display(None).to_string(),
            expected.display(None).to_string()
        );
    }

    #[test]
    fn reconstruct_jump() {
        let _ = pretty_env_logger::try_init();
        let (env, trace) = interpret(
            "
            function %mul3(i32) -> i32 {
            fn0 = %add(i32, i32) -> i32
            block0(v20: i32):
                trace_start 99
                v21 = iadd.i32 v20, v20
                v22 = call fn0(v20, v21)
                return v22
            }
            ; run: %mul3(5)
            
            function %add(i32, i32) -> i32 {
            block0(v20: i32, v21: i32):
                v19 = iadd.i32 v20, v21
                return v19
            }
            ",
        );

        let reconstructed = to_function(trace, env);
        let expected = parse(
            "
            function u0:0(i32) {
            block0(v1: i32):
                v0 = iadd.i32 v1, v1
                v2 = iadd.i32 v1, v0
                return
            }",
        );

        assert_eq!(
            reconstructed.display(None).to_string(),
            expected.display(None).to_string()
        );
    }

    // TODO add more tests: through function call, through jump, through branch
}
