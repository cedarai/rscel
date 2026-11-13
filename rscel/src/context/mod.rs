use std::collections::HashMap;

mod bind_context;
mod default_funcs;
mod default_macros;
mod type_funcs;
use crate::{
    compiler::{compiler::CelCompiler, string_tokenizer::StringTokenizer},
    interp::Interpreter,
    program::{Program, ProgramDetails},
    CelResult, CelValue,
};
pub use bind_context::{BindContext, RsCelFunction, RsCelMacro};
pub use type_funcs::construct_type;

/// The CelContext is the core context in RsCel. This context contains
/// Program information as well as the primary entry point for evaluating
/// an expression.
pub struct CelContext {
    progs: HashMap<String, Program>,
}

impl CelContext {
    /// Constructs a new empty CelContext
    pub fn new() -> CelContext {
        CelContext {
            progs: HashMap::new(),
        }
    }

    /// Add an already constructed Program to the context with a given name. Using
    /// This method can allow a Program to be constructed once and shared between
    /// contexts, if desired. Will override an existing program with same name.
    pub fn add_program(&mut self, name: &str, prog: Program) {
        self.progs.insert(name.to_owned(), prog);
    }

    /// Add a Program to the context with the given name and source string. Return of this
    /// function indicates parseing result of the constructed Program. This method will not
    /// allow for a Program to be shared. Will override an existing program with same name.
    pub fn add_program_str(&mut self, name: &str, prog_str: &str) -> CelResult<()> {
        let mut tokenizer = StringTokenizer::with_input(prog_str);
        let prog = CelCompiler::with_tokenizer(&mut tokenizer).compile()?;

        self.add_program(name, prog);
        Ok(())
    }

    /// Returns ProgramDetails for a program by name if it exists.
    pub fn program_details<'a>(&'a self, name: &str) -> Option<&'a ProgramDetails> {
        let prog = self.progs.get(name)?;

        Some(prog.details())
    }

    pub fn get_program<'a>(&'a self, name: &str) -> Option<&'a Program> {
        self.progs.get(name)
    }

    /// Evaluate a Program with the given name with a provided ExecContext. A single CelContext
    /// can be run multiple times with different ExecContext's. The return of this function is
    /// a Result with either a ValueCell representing the final solution of the Program or an Error
    /// that is discovered during execution, such as mismatch of types
    pub fn exec<'l>(&'l mut self, name: &str, bindings: &'l BindContext) -> CelResult<CelValue> {
        let interp = Interpreter::new(&self, bindings);

        interp.run_program(name)
    }

    /// Cooperative deadline and one total instruction budget, including macros.
    /// A CEL expression cannot mask exhaustion by treating its error as a value.
    pub fn exec_bounded<'l>(
        &'l self,
        name: &str,
        bindings: &'l BindContext,
        instructions: usize,
        deadline: std::time::Instant,
    ) -> CelResult<CelValue> {
        let mut interp = Interpreter::new(self, bindings);
        interp.set_execution_budget(instructions, deadline);
        let result = interp.run_program(name);
        if let Some(error) = interp.execution_failure() {
            return Err(error);
        }
        if interp.execution_budget_exhausted() || std::time::Instant::now() >= deadline {
            return Err(crate::CelError::runtime("CEL execution budget exceeded"));
        }
        result
    }

    /// One shared budget for parser work,constant folding,macro identifiers and
    /// runtime evaluation. Existing add_program_str/exec retain legacy semantics.
    pub fn eval_bounded(
        &mut self,
        name: &str,
        source: &str,
        bindings: &BindContext,
        instructions: usize,
        deadline: std::time::Instant,
    ) -> CelResult<CelValue> {
        let budget = crate::interp::ExecutionBudget::new(instructions, deadline);
        budget.admit_source(source)?;
        let mut tokenizer = StringTokenizer::with_input(source);
        let result = CelCompiler::with_tokenizer(&mut tokenizer)
            .with_execution_budget(budget.clone())
            .compile();
        if let Some(error) = budget.failure() {
            return Err(error);
        }
        if budget.exhausted() || std::time::Instant::now() >= deadline {
            return Err(crate::CelError::runtime("CEL execution budget exceeded"));
        }
        self.add_program(name, result?);
        let mut interp = Interpreter::new(self, bindings);
        interp.set_budget_handle(budget.clone());
        let result = interp.run_program(name);
        if let Some(error) = budget.failure() {
            return Err(error);
        }
        if budget.exhausted() || std::time::Instant::now() >= deadline {
            return Err(crate::CelError::runtime("CEL execution budget exceeded"));
        }
        result
    }

    /// Bounded compilation for native bytecode-based compatibility rewrites.
    /// It returns the actual typed Program;no serialized bytecode transport.
    pub fn compile_bounded(
        source: &str,
        instructions: usize,
        deadline: std::time::Instant,
    ) -> CelResult<Program> {
        let budget = crate::interp::ExecutionBudget::new(instructions, deadline);
        budget.admit_source(source)?;
        let mut tokenizer = StringTokenizer::with_input(source);
        let result = CelCompiler::with_tokenizer(&mut tokenizer)
            .with_execution_budget(budget.clone())
            .compile();
        if let Some(error) = budget.failure() {
            return Err(error);
        }
        if budget.exhausted() || std::time::Instant::now() >= deadline {
            return Err(crate::CelError::runtime("CEL execution budget exceeded"));
        }
        result
    }

    // pub(crate) fn eval_expr(
    //     &mut self,
    //     expr: &Expr,
    //     ctx: &BindContext,
    // ) -> ValueCellResult<ValueCell> {
    //     self.current_ctx = Some(ctx.clone());

    //     let res = eval_expr(expr, self);

    //     self.current_ctx = None;
    //     return res;
    // }
}

impl Clone for CelContext {
    fn clone(&self) -> Self {
        CelContext {
            progs: self.progs.clone(),
        }
    }
}

#[cfg(test)]
mod test {

    use super::{BindContext, CelContext};

    #[test]
    fn test_eval_basic() {
        let mut ctx = CelContext::new();
        let exec_ctx = BindContext::new();

        ctx.add_program_str("test_main", "3 + 4").unwrap();

        let res = ctx.exec("test_main", &exec_ctx).unwrap();

        assert!(res == 7.into())
    }
}
