//! Test command for running CLIF files and verifying their results
//!
//! The `run` test command compiles each function on the host machine and executes it

use crate::function_runner::SingleFunctionCompiler;
use crate::subtest::{Context, SubTest, SubtestResult};
use cranelift_codegen;
use cranelift_codegen::ir;
use cranelift_reader::TestCommand;
use cranelift_reader::{parse_run_command, RunCommand};
use log::trace;
use std::borrow::Cow;

struct TestRun;

pub fn subtest(parsed: &TestCommand) -> SubtestResult<Box<dyn SubTest>> {
    assert_eq!(parsed.command, "run");
    if !parsed.options.is_empty() {
        Err(format!("No options allowed on {}", parsed))
    } else {
        Ok(Box::new(TestRun))
    }
}

impl SubTest for TestRun {
    fn name(&self) -> &'static str {
        "run"
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn needs_isa(&self) -> bool {
        false
    }

    fn run(&self, func: Cow<ir::Function>, context: &Context) -> SubtestResult<()> {
        let mut compiler = SingleFunctionCompiler::with_host_isa(context.flags.clone());
        for comment in context.details.comments.iter() {
            if RunCommand::is_potential_run_command(comment.text) {
                let trimmed = RunCommand::trim_comment_chars(comment.text);
                let command =
                    parse_run_command(trimmed, &func.signature).map_err(|e| e.to_string())?;
                trace!("Parsed run command: {}", command);

                let compiled_fn = compiler
                    .compile(func.clone().into_owned())
                    .map_err(|e| format!("{}", e))?; // TODO avoid clone
                command.run(|_, args| Ok(compiled_fn.call(args)))?;
            }
        }
        Ok(())
    }
}
