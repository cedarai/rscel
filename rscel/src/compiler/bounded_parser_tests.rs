use super::source_location::SourceLocation;
use super::syntax_error::SyntaxError;
use super::tokenizer::{TokenWithLoc, Tokenizer};
use crate::interp::ExecutionBudget;
use crate::{CelCompiler, StringTokenizer};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

struct CountingTokenizer<'a> {
    inner: StringTokenizer<'a>,
    consumed: Rc<Cell<usize>>,
}

impl Tokenizer for CountingTokenizer<'_> {
    fn peek(&mut self) -> Result<Option<&TokenWithLoc>, SyntaxError> {
        self.inner.peek()
    }
    fn next(&mut self) -> Result<Option<TokenWithLoc>, SyntaxError> {
        self.consumed.set(self.consumed.get() + 1);
        self.inner.next()
    }
    fn source(&self) -> &str {
        self.inner.source()
    }
    fn location(&self) -> SourceLocation {
        self.inner.location()
    }
}

#[test]
fn exhausted_fuel_stops_prefix_scanning_before_recursing_through_input() {
    for prefix in ['!', '-'] {
        let source = format!(
            "{}{}",
            prefix.to_string().repeat(128),
            if prefix == '!' { "true" } else { "1" }
        );
        let consumed = Rc::new(Cell::new(0));
        let mut tokenizer = CountingTokenizer {
            inner: StringTokenizer::with_input(&source),
            consumed: Rc::clone(&consumed),
        };
        let budget = ExecutionBudget::new(1, Instant::now() + Duration::from_secs(5));
        let result = CelCompiler::with_tokenizer(&mut tokenizer)
            .with_execution_budget(budget)
            .compile();
        assert!(
            consumed.get() <= 2,
            "consumed {} tokens after fuel exhaustion",
            consumed.get()
        );
        assert!(result.is_err());
    }
}

#[test]
fn exhausted_fuel_stops_flat_binary_boolean_and_member_scans() {
    for source in [
        format!("{}1", "1+".repeat(128)),
        format!("{}true", "true && ".repeat(128)),
        format!("root{}", ".field".repeat(128)),
    ] {
        let consumed = Rc::new(Cell::new(0));
        let mut tokenizer = CountingTokenizer {
            inner: StringTokenizer::with_input(&source),
            consumed: Rc::clone(&consumed),
        };
        let budget = ExecutionBudget::new(1, Instant::now() + Duration::from_secs(5));
        let result = CelCompiler::with_tokenizer(&mut tokenizer)
            .with_execution_budget(budget)
            .compile();
        assert!(
            consumed.get() <= 2,
            "consumed {} tokens after fuel exhaustion",
            consumed.get()
        );
        assert!(result.is_err());
    }
}
