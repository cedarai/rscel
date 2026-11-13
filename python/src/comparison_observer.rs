//! Private observation API: no alternate evaluator and no result replacement.
use pyo3::{pyclass, pymethods};
#[pyclass(name = "_ClockObservation", unsendable)]
pub struct ClockObservation {
    scope: Option<rscel::clock::Scope>,
}
#[pymethods]
impl ClockObservation {
    #[new]
    fn new() -> Self {
        Self {
            scope: Some(rscel::clock::Scope::observe()),
        }
    }
    fn finish(&mut self) -> Option<Vec<(i64, u32)>> {
        self.scope.take().and_then(rscel::clock::Scope::finish)
    }
}
pub fn program_json(program: &rscel::Program) -> Option<String> {
    rscel::comparison::snapshot(program)
}
