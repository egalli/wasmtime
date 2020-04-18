//! Cranelift IR interpreter.
//!
//! This module contains the logic for interpreting Cranelift instructions.

use crate::environment::Environment;
use crate::frame::Frame;
use crate::interpreter::Trap::InvalidType;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::{
    Block, FuncRef, Function, Inst, InstructionData, InstructionData::*, Opcode, Opcode::*, Type,
    Value as ValueRef, ValueList,
};
use cranelift_reader::DataValue;
use log::debug;
use std::ops::{Add, Sub};
use thiserror::Error;

/// The valid control flow states.
pub enum ControlFlow {
    Continue,
    ContinueAt(Block, Vec<ValueRef>),
    Return(Vec<DataValue>),
}

/// The ways interpretation can fail.
#[derive(Error, Debug)]
pub enum Trap {
    #[error("unknown trap")]
    Unknown,
    #[error("invalid type for {1}: expected {0}")]
    InvalidType(String, ValueRef),
    #[error("reached an unreachable statement")]
    Unreachable,
    #[error("invalid control flow: {0}")]
    InvalidControlFlow(String),
    #[error("invalid function reference: {0}")]
    InvalidFunctionReference(FuncRef),
    #[error("invalid function name: {0}")]
    InvalidFunctionName(String),
}

/// The Cranelift interpreter; it contains immutable elements such as the function environment and
/// implements the Cranelift IR semantics.
#[derive(Default)]
pub struct Interpreter {
    pub env: Environment,
}

impl Interpreter {
    pub fn new(env: Environment) -> Self {
        Self { env }
    }

    pub fn call_by_name(
        &self,
        func_name: &str,
        arguments: &[DataValue],
    ) -> Result<ControlFlow, Trap> {
        let func_ref = self
            .env
            .index_of(func_name)
            .ok_or_else(|| Trap::InvalidFunctionName(func_name.to_string()))?;
        self.call_by_index(func_ref, arguments)
    }

    pub fn call_by_index(
        &self,
        func_ref: FuncRef,
        arguments: &[DataValue],
    ) -> Result<ControlFlow, Trap> {
        match self.env.get_by_func_ref(func_ref) {
            None => Err(Trap::InvalidFunctionReference(func_ref)),
            Some(func) => self.call(func, arguments),
        }
    }

    fn call(&self, function: &Function, arguments: &[DataValue]) -> Result<ControlFlow, Trap> {
        debug!("Call: {}({:?})", function.name, arguments);
        let first_block = function
            .layout
            .blocks()
            .next()
            .expect("to have a first block");
        let parameters = function.dfg.block_params(first_block);
        let mut frame = Frame::new(function).with_parameters(parameters, arguments);
        self.block(&mut frame, first_block)
    }

    fn block(&self, frame: &mut Frame, block: Block) -> Result<ControlFlow, Trap> {
        debug!("Block: {}", block);
        for inst in frame.function.layout.block_insts(block) {
            match self.inst(frame, inst)? {
                ControlFlow::Continue => continue,
                ControlFlow::ContinueAt(block, old_names) => {
                    let new_names = frame.function.dfg.block_params(block);
                    frame.rename(&old_names, new_names);
                    return self.block(frame, block); // TODO check that TCO happens
                }
                ControlFlow::Return(rs) => return Ok(ControlFlow::Return(rs)),
            }
        }
        Err(Trap::Unreachable)
    }

    fn binary(
        &self,
        frame: &mut Frame,
        op: fn(DataValue, DataValue) -> DataValue,
        a: ValueRef,
        b: ValueRef,
        r: ValueRef,
    ) {
        let a = frame.get(&a);
        let b = frame.get(&b);
        let c = op(a.clone(), b.clone());
        frame.set(r, c);
    }

    // TODO refactor to only one `binary` method
    fn binary_imm(
        &self,
        frame: &mut Frame,
        op: fn(DataValue, DataValue) -> DataValue,
        a: ValueRef,
        b: DataValue,
        r: ValueRef,
    ) {
        let a = frame.get(&a);
        let c = op(a.clone(), b);
        frame.set(r, c);
    }

    fn iconst(&self, frame: &mut Frame, imm: Imm64, r: ValueRef) {
        let imm_value = cast(imm, type_of(r, frame.function)).expect("an integer");
        frame.set(r, imm_value);
    }

    fn bconst(&self, frame: &mut Frame, imm: bool, r: ValueRef) {
        frame.set(r, DataValue::B(imm));
    }

    // TODO add load/store
    fn inst(&self, frame: &mut Frame, inst: Inst) -> Result<ControlFlow, Trap> {
        use ControlFlow::{Continue, ContinueAt};
        debug!("Inst: {}", &frame.function.dfg.display_inst(inst, None));

        let data = &frame.function.dfg[inst];
        match data {
            Binary { opcode, args } => match opcode {
                Iadd => {
                    // TODO trap if arguments are of the wrong type; here and below
                    let res = first_result(frame.function, inst);
                    self.binary(frame, Add::add, args[0], args[1], res);
                    Ok(Continue)
                }
                _ => unimplemented!(),
            },
            BinaryImm { opcode, arg, imm } => match opcode {
                IrsubImm => {
                    let res = first_result(frame.function, inst);
                    let imm = DataValue::from((*imm).into());
                    self.binary_imm(frame, Sub::sub, *arg, imm, res);
                    Ok(Continue)
                }
                _ => unimplemented!(),
            },
            Branch {
                opcode,
                args,
                destination,
            } => match opcode {
                Brnz => {
                    let mut args = value_refs(frame.function, args);
                    let first = args.remove(0);
                    match frame.get(&first) {
                        DataValue::B(false)
                        | DataValue::I8(0)
                        | DataValue::I16(0)
                        | DataValue::I32(0)
                        | DataValue::I64(0) => Ok(Continue),
                        DataValue::B(true)
                        | DataValue::I8(_)
                        | DataValue::I16(_)
                        | DataValue::I32(_)
                        | DataValue::I64(_) => Ok(ContinueAt(*destination, args)),
                        _ => Err(Trap::InvalidType("boolean or integer".to_string(), args[0])),
                    }
                }
                _ => unimplemented!(),
            },
            InstructionData::Call { args, func_ref, .. } => {
                // Find the function to call.
                let func_name = function_name_of_func_ref(*func_ref, frame.function);

                // Call function.
                let args = frame.get_all(args.as_slice(&frame.function.dfg.value_lists));
                let result = self.call_by_name(&func_name, &args)?;

                // Save results.
                if let ControlFlow::Return(returned_values) = result {
                    let ssa_values = frame.function.dfg.inst_results(inst);
                    assert_eq!(
                        ssa_values.len(),
                        returned_values.len(),
                        "expected result length ({}) to match SSA values length ({}): {}",
                        returned_values.len(),
                        ssa_values.len(),
                        frame.function.dfg.display_inst(inst, None)
                    );
                    frame.set_all(ssa_values, returned_values);
                    Ok(Continue)
                } else {
                    Err(Trap::InvalidControlFlow(format!(
                        "did not return from: {}",
                        frame.function.dfg.display_inst(inst, None)
                    )))
                }
            }
            InstructionData::Jump {
                opcode,
                destination,
                args,
            } => match opcode {
                Opcode::Fallthrough => {
                    Ok(ContinueAt(*destination, value_refs(frame.function, args)))
                }
                Opcode::Jump => Ok(ContinueAt(*destination, value_refs(frame.function, args))),
                _ => unimplemented!(),
            },
            IntCompareImm {
                opcode,
                arg,
                cond,
                imm,
            } => match opcode {
                IcmpImm => {
                    let arg_value = match *frame.get(arg) {
                        // TODO implement TryInto for DataValue
                        DataValue::I8(i)
                        | DataValue::I16(i)
                        | DataValue::I32(i)
                        | DataValue::I64(i) => Ok(i as u64),
                        _ => Err(InvalidType("integer".to_string(), arg)),
                    }?;
                    let imm_value = (*imm).into();
                    let result = match cond {
                        IntCC::UnsignedLessThanOrEqual => arg_value <= imm_value,
                        IntCC::Equal => arg_value == imm_value,
                        _ => unimplemented!(),
                    };
                    let res = first_result(frame.function, inst);
                    frame.set(res, DataValue::B(result));
                    Ok(Continue)
                }
                _ => unimplemented!(),
            },
            MultiAry { opcode, args } => match opcode {
                Return => {
                    let rs: Vec<DataValue> = args
                        .as_slice(&frame.function.dfg.value_lists)
                        .iter()
                        .map(|r| frame.get(r).clone())
                        .collect();
                    Ok(ControlFlow::Return(rs))
                }
                _ => unimplemented!(),
            },
            NullAry { opcode } => match opcode {
                Nop => Ok(Continue),
                _ => unimplemented!(),
            },
            UnaryImm { opcode, imm } => match opcode {
                Iconst => {
                    let res = first_result(frame.function, inst);
                    self.iconst(frame, *imm, res);
                    Ok(Continue)
                }
                _ => unimplemented!(),
            },
            UnaryBool { opcode, imm } => match opcode {
                Bconst => {
                    let res = first_result(frame.function, inst);
                    self.bconst(frame, *imm, res);
                    Ok(Continue)
                }
                _ => unimplemented!(),
            },

            _ => unimplemented!("{:?}", data),
        }
    }
}

/// Return the first result of an instruction.
///
/// This helper cushions the interpreter from changes to the [Function] API.
#[inline]
fn first_result(function: &Function, inst: Inst) -> ValueRef {
    function.dfg.first_result(inst)
}

/// Return a list of IR values as a vector.
///
/// This helper cushions the interpreter from changes to the [Function] API.
#[inline]
fn value_refs(function: &Function, args: &ValueList) -> Vec<ValueRef> {
    args.as_slice(&function.dfg.value_lists).to_vec()
}

/// Return the (external) function name of `func_ref` in a local `function`. Note that this may
/// be truncated.
///
/// This helper cushions the interpreter from changes to the [Function] API.
#[inline]
fn function_name_of_func_ref(func_ref: FuncRef, function: &Function) -> String {
    function
        .dfg
        .ext_funcs
        .get(func_ref)
        .expect("function to exist")
        .name
        .to_string()
}

/// Cast an immediate integer to its correct [DataValue] representation.
///
/// TODO move to `impl DataValue`; parameterize on other input types? e.g. TryProm
#[inline]
fn cast(input: Imm64, ty: Type) -> Result<DataValue, Trap> {
    match ty {
        I8 => Ok(DataValue::I8(input.bits() as i8)),
        I16 => Ok(DataValue::I16(input.bits() as i16)),
        I32 => Ok(DataValue::I32(input.bits() as i32)),
        I64 => Ok(DataValue::I64(input.bits() as i64)),
        _ => unimplemented!(), // Err(Trap::InvalidType("integer".to_string())) // TO
    }
}

/// Helper for calculating the type of an IR value.
#[inline]
fn type_of(value: ValueRef, function: &Function) -> Type {
    function.dfg.value_type(value)
}

#[cfg(test)]
mod tests {}
