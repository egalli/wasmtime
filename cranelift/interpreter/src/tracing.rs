//! Tracing functionality

use cranelift_codegen::ir::{FuncRef, Inst};

#[derive(Default, Debug, PartialEq)]
pub struct Trace {
    tracing: bool,
    observed: Vec<TracedInstruction>,
}

impl Trace {
    pub fn start(&mut self) {
        self.tracing = true
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

    // TODO add reconstruct() -> Vec<InstructionData>
}

impl From<Vec<TracedInstruction>> for Trace {
    fn from(observed: Vec<TracedInstruction>) -> Self {
        Self {
            observed,
            ..Default::default()
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum TracedInstruction {
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
