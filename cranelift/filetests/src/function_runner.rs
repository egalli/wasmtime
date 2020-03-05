//! A simplistic mechanism for running functions. TODO this could be merged with SimpleJIT?

use core::mem;
use cranelift_codegen::binemit::{NullRelocSink, NullStackmapSink, NullTrapSink};
use cranelift_codegen::ir::Function;
use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::{settings, Context};
use cranelift_native::builder as host_isa_builder;
use cranelift_value::Value;
use memmap::{Mmap, MmapMut}; // TODO break this cyclic dependency with cranelift-interpreter (FunctionRunner uses Value, tracing uses FunctionRunner)

/// Container for compiled code to avoid exposing the underlying Mmap implementation.
pub struct CompiledCode {
    page: Mmap,
}

impl CompiledCode {
    /// Return a pointer to the compiled code.
    pub fn as_ptr(&self) -> *const u8 {
        self.page.as_ptr()
    }

    /// Borrow the compiled code as a slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.page
    }
}

impl From<Mmap> for CompiledCode {
    fn from(page: Mmap) -> Self {
        CompiledCode { page }
    }
}

/// Run a function on a host
pub struct FunctionRunner {
    function: Function,
    isa: Box<dyn TargetIsa>,
}

impl FunctionRunner {
    /// Build a function runner from a function and the ISA to run on (must be the host machine's ISA)
    pub fn new(function: Function, isa: Box<dyn TargetIsa>) -> Self {
        Self { function, isa }
    }

    /// Build a function runner using the host machine's ISA and the passed flags
    pub fn with_host_isa(function: Function, flags: settings::Flags) -> Self {
        let builder = host_isa_builder().expect("Unable to build a TargetIsa for the current host");
        let isa = builder.finish(flags);
        Self::new(function, isa)
    }

    /// Build a function runner using the host machine's ISA and the default flags for this ISA
    pub fn with_default_host_isa(function: Function) -> Self {
        let flags = settings::Flags::new(settings::builder());
        Self::with_host_isa(function, flags)
    }

    /// Compile the given function to machine code in the TargetIsa.
    /// TODO improve errors; replace String with thiserror?
    pub fn compile(&self) -> Result<CompiledCode, String> {
        if self.function.signature.call_conv != self.isa.default_call_conv() {
            return Err(String::from(
                "Functions only run on the host's default calling convention; remove the specified calling convention in the function signature to use the host's default.",
            ));
        }

        // set up the context
        let mut context = Context::for_function(self.function.clone()); // TODO avoid clone

        // compile and encode the result to machine code
        let relocs = &mut NullRelocSink {};
        let traps = &mut NullTrapSink {};
        let stackmaps = &mut NullStackmapSink {};
        let code_info = context
            .compile(self.isa.as_ref())
            .map_err(|e| e.to_string())?;
        let mut code_page =
            MmapMut::map_anon(code_info.total_size as usize).map_err(|e| e.to_string())?;

        unsafe {
            context.emit_to_memory(
                self.isa.as_ref(),
                code_page.as_mut_ptr(),
                relocs,
                traps,
                stackmaps,
            );
        };

        let compiled_code = CompiledCode::from(code_page.make_exec().map_err(|e| e.to_string())?);
        Ok(compiled_code)
    }

    /// Execute compiled code with the given arguments. TODO change &[u8] to CompiledCode?
    /// TODO better errors
    pub fn execute(code: &[u8], arguments: &[Value]) -> Result<Value, String> {
        match arguments.len() {
            0 => FunctionRunner::execute0(code),
            1 => FunctionRunner::execute1(code, arguments[0].clone()), // TODO avoid clone
            _ => unimplemented!(),
        }
    }

    /// Execute a compiled function with no arguments. TODO multi-value return is unlikely
    pub fn execute0(_code: &[u8]) -> Result<Value, String> {
        // TODO figure out how to cast code to a callable function with the right types (see wrappers! and getters! in crates/api/src/func.rs)
        unimplemented!()
    }

    /// Execute a compiled function with a single argument. TODO multi-value return is unlikely
    pub fn execute1(_code: &[u8], _a: Value) -> Result<Value, String> {
        // TODO figure out how to cast code to a callable function with the right types (see wrappers! and getters! in crates/api/src/func.rs)
        unimplemented!()
    }

    /// Compile and execute a single function, expecting a boolean to be returned; a 'true' value is
    /// interpreted as a successful test execution and mapped to Ok whereas a 'false' value is
    /// interpreted as a failed test and mapped to Err.
    pub fn run(&self) -> Result<(), String> {
        let func = self.function.clone();
        if !(func.signature.params.is_empty()
            && func.signature.returns.len() == 1
            && func.signature.returns.first().unwrap().value_type.is_bool())
        {
            return Err(String::from(
                "Functions must have a signature like: () -> boolean",
            ));
        }

        let code_page = self.compile()?;
        let callable_fn: fn() -> bool = unsafe { mem::transmute(code_page.as_ptr()) };

        // execute
        if callable_fn() {
            Ok(())
        } else {
            Err(format!("Failed: {}", func.name.to_string()))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use cranelift_reader::{parse_test, ParseOptions};

    #[test]
    fn nop() {
        let code = String::from(
            "
            test run
            function %test() -> b8 {
            block0:
                nop
                v1 = bconst.b8 true
                return v1
            }",
        );

        // extract function
        let test_file = parse_test(code.as_str(), ParseOptions::default()).unwrap();
        assert_eq!(1, test_file.functions.len());
        let function = test_file.functions[0].0.clone();

        // execute function
        let runner = FunctionRunner::with_default_host_isa(function);
        runner.run().unwrap() // will panic if execution fails
    }
}
