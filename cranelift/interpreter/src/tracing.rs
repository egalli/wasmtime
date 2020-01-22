//! Implements the tracing functionality for the Cranelift interpreter.

use crate::environment::Environment;
use cranelift_codegen::ir::{FuncRef, Inst, InstructionData};

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

    pub fn remove_last(&mut self) -> Option<TracedInstruction> {
        self.observed.pop()
    }

    pub fn reconstruct(&self, env: Environment) -> impl Iterator<Item = InstructionData> {
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

struct TraceIterator {
    cursor: usize,
    stack: Vec<FuncRef>,
    trace: Vec<TracedInstruction>, // TODO make a reference?
    env: Environment,
}
impl Iterator for TraceIterator {
    type Item = InstructionData;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.trace.len() {
            let inst = &self.trace[self.cursor];
            self.cursor += 1;
            match inst {
                TracedInstruction::StartInFunction(f) | TracedInstruction::EnterFunction(f) => {
                    self.stack.push(*f)
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
                    let inst_data = func.dfg[*i].clone();
                    return Some(inst_data);
                }
                TracedInstruction::Guard(_) => unimplemented!(),
            }
            continue;
        }
        None
    }
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
    EnterFunction(FuncRef),
    ExitFunction,
    Instruction(Inst),
    Guard(Inst),
    //Loop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_of() {
        assert_eq!(std::mem::size_of::<TracedInstruction>(), 8);
    }
}
