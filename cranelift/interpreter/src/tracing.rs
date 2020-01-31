//! Implements the tracing functionality for the Cranelift interpreter.

use crate::environment::Environment;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir::{AbiParam, FuncRef, Function, Inst, InstructionData, Opcode, Value};
use log::debug;
use std::collections::HashMap;

#[derive(Default, Debug, PartialEq)]
pub struct Trace {
    tracing: bool,
    observed: Vec<TracedInstruction>,
}

impl Trace {
    pub fn start(&mut self, func_ref: FuncRef) {
        self.tracing = true;
        self.observed
            .push(TracedInstruction::StartInFunction(func_ref))
    }

    pub fn end(&mut self) {
        self.tracing = false
    }

    pub fn observe(&mut self, observed: TracedInstruction) {
        if self.tracing {
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
pub fn to_function(trace: Trace, env: &Environment) -> Function {
    type OldValue = Value;
    type NewValue = Value;
    type Renumberings = HashMap<OldValue, NewValue>;
    type CallResults = Vec<Value>;
    type StackFrame = (FuncRef, Renumberings, CallResults);

    let mut renumberings: Renumberings = Renumberings::new();
    let mut input_renumberings = Renumberings::new(); // values with unknown provenance, used in
                                                      // EBB header and function signature
    let mut stack: Vec<StackFrame> = Vec::new();
    let mut old_func = &Function::new(); // default value, promptly replaced in StartInFunction
                                         // or EnterFunction
    let mut new_func = Function::new();
    let mut cursor = FuncCursor::new(&mut new_func);
    let ebb = cursor.func.dfg.make_ebb();
    cursor.insert_ebb(ebb);

    for ti in trace.observed {
        match ti {
            TracedInstruction::StartInFunction(f) => {
                old_func = env
                    .get_by_func_ref(f)
                    .expect("the environment to contain a valid function");
                stack.push((f, Renumberings::new(), CallResults::new()));
            }
            TracedInstruction::EnterFunction(_) => {
                // TODO this could be done by call/return
                //                old_func = env
                //                    .get_by_func_ref(f)
                //                    .expect("the environment to contain a valid function");
            }
            TracedInstruction::ExitFunction => {
                //                // TODO this could be done by call/return
                //                old_func = env
                //                    .get_by_func_ref(stack.last().unwrap().0)
                //                    .expect("the environment to contain a valid function");
            }
            TracedInstruction::Instruction(inst) => {
                let inst_data = old_func.dfg[inst].clone();
                let new_inst = cursor.func.dfg.make_inst(inst_data);

                let results = old_func.dfg.inst_results(inst);
                for old_result in results {
                    // fulfill requirement 1
                    let old_type = old_func.dfg.value_type(*old_result);
                    let new_result = cursor.func.dfg.append_result(new_inst, old_type);
                    renumberings.insert(*old_result, new_result);
                }

                let mut unknown_args = Vec::new();
                for old_arg in cursor.func.dfg.inst_args_mut(new_inst) {
                    // fulfill requirement 4
                    if let Some(new_arg) = renumberings.get(old_arg) {
                        *old_arg = *new_arg;
                    } else if let Some(new_arg) =
                        stack.last().expect("a stack renumbering").1.get(old_arg)
                    {
                        *old_arg = *new_arg;
                    } else if let Some(new_arg) = input_renumberings.get(old_arg) {
                        *old_arg = *new_arg;
                    } else {
                        unknown_args.push(*old_arg);
                    }
                }

                // FIXME this is insane: because we need to `inst_args_mut` and
                // `append_ebb_param` at the same time, we have to store values in unknown args
                // and re-examine all of the instruction arguments...
                for old_arg in unknown_args {
                    let ty = old_func.dfg.value_type(old_arg);
                    let new_arg = cursor.func.dfg.append_ebb_param(ebb, ty);
                    input_renumberings.insert(old_arg, new_arg);
                }
                for old_arg in cursor.func.dfg.inst_args_mut(new_inst) {
                    if let Some(new_arg) = input_renumberings.get(old_arg) {
                        *old_arg = *new_arg;
                    }
                }

                // TODO avoid clone
                match old_func.dfg[inst].clone() {
                    InstructionData::Call { args, func_ref, .. } => {
                        // partially fulfill requirement 2
                        let caller_results = old_func.dfg.inst_results(inst).to_vec();
                        let caller_args = args.as_slice(&old_func.dfg.value_lists);
                        let callee_func = env
                            .get_by_func_ref(func_ref)
                            .expect("the called function to exist");
                        let callee_ebb = callee_func
                            .layout
                            .ebbs()
                            .next()
                            .expect("to have a first ebb");
                        let callee_args = callee_func.dfg.ebb_params(callee_ebb);
                        debug_assert_eq!(caller_args.len(), callee_args.len());

                        let mut local_renumbering: Renumberings = Renumberings::new();
                        for (old_arg, new_arg) in caller_args.iter().zip(callee_args) {
                            local_renumbering.insert(*old_arg, *new_arg);
                        }

                        stack.push((func_ref, local_renumbering, caller_results));

                        // point old_func at the callee
                        old_func = env
                            .get_by_func_ref(func_ref)
                            .expect("the environment to contain a valid function");
                    }
                    InstructionData::Jump {
                        destination, args, ..
                    } => {
                        // partially fulfill requirement 2
                        let caller_args = args.as_slice(&old_func.dfg.value_lists);
                        let destination_args = old_func.dfg.ebb_params(destination);
                        debug_assert_eq!(caller_args.len(), destination_args.len());

                        let mut local_renumbering: Renumberings = Renumberings::new();
                        for (old_arg, new_arg) in caller_args.iter().zip(destination_args) {
                            local_renumbering.insert(*old_arg, *new_arg);
                        }

                        // replace local renumbering with the renumbering from the current block
                        let (func_ref, _, caller_results) =
                            stack.pop().expect("to have something on the stack");
                        stack.push((func_ref, local_renumbering, caller_results));
                    }
                    InstructionData::MultiAry {
                        opcode: Opcode::Return,
                        args,
                    } => {
                        // fulfill requirement 3
                        let (func_ref, _, caller_results) = stack.pop().expect("...");
                        let callee_returns = args.as_slice(&old_func.dfg.value_lists);
                        for (old_value, new_value) in caller_results.iter().zip(callee_returns) {
                            renumberings.insert(*old_value, *new_value);
                        }

                        // point old_func to the caller function
                        old_func = env
                            .get_by_func_ref(func_ref)
                            .expect("the environment to contain a valid function");
                    }
                    _ => {
                        cursor.insert_inst(new_inst);
                        debug!(
                            "Appending instruction: {}",
                            cursor.func.dfg.display_inst(new_inst, None)
                        );
                    }
                }
            }
            TracedInstruction::Guard(_) => {}
        }
    }

    // fixup function signature
    for (_, new_arg) in input_renumberings {
        let ty = cursor.func.dfg.value_type(new_arg);
        let param = AbiParam::new(ty);
        cursor.func.signature.params.push(param);
    }

    new_func
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
        let function = test_file.functions[0].0.clone();
        function
    }

    #[test]
    fn reconstruct_requirement1() {
        let (env, trace) = interpret(
            "
            function %test(i32, i32) -> i32 {
            ebb0(v43: i32, v44: i32):
                trace_start 99
                v42 = iadd.i32 v43, v44
                v45 = isub.i32 v42, v43
                return v45
            }
           ; run: %test(1, 2)",
        );

        let reconstructed = to_function(trace, &env);
        let expected = parse(
            "
            function u0:0(i32, i32) {
            ebb0(v1: i32, v2: i32):
                v0 = iadd.i32 v1, v2
                v3 = isub.i32 v0, v1
            }",
        );

        assert_eq!(
            reconstructed.display(None).to_string(),
            expected.display(None).to_string()
        );
    }
}
