#![feature(fn_traits)]
#![feature(unboxed_closures)]
use py_cel_error::PyCelError;
use py_cel_value::PyCelValue;
use rscel::{BindContext, CelContext, CelValue, Program};

use pyo3::{
    exceptions::PyException,
    prelude::*,
    types::{PyDict, PyString},
    IntoPyObjectExt,
};

mod cel_py_object;
mod celpycallable;
mod comparison_observer;
mod frompyobject;
mod py_bind_context;
mod py_cel_context;
mod py_cel_error;
mod py_cel_program;
mod py_cel_value;

use celpycallable::CelPyCallable;
use py_bind_context::PyBindContext;
use py_cel_context::PyCelContext;
use py_cel_program::PyCelProgram;
use rscel_to_sql::IntoSqlBuilder;

/* Eval entry point */
#[pyfunction]
fn eval(py: Python<'_>, prog_str: String, bindings: &Bound<PyDict>) -> PyResult<Py<PyAny>> {
    eval_impl(py, prog_str, bindings, None)
}

#[pyfunction]
fn _eval_observed(
    py: Python<'_>,
    prog_str: String,
    bindings: &Bound<PyDict>,
    observer: Py<PyAny>,
) -> PyResult<Py<PyAny>> {
    eval_impl(py, prog_str, bindings, Some(observer))
}
fn eval_impl(
    py: Python<'_>,
    prog_str: String,
    bindings: &Bound<PyDict>,
    observer: Option<Py<PyAny>>,
) -> PyResult<Py<PyAny>> {
    let callables = {
        let mut callables = Vec::new();
        for keyobj in bindings.keys() {
            let key = keyobj.cast::<PyString>()?;
            let val = bindings.get_item(key).unwrap().unwrap().clone();

            if val.is_callable() {
                callables.push((key.to_str()?.to_string(), CelPyCallable::new(val.into())));
            }
        }
        callables
    };
    let mut ctx = CelContext::new();
    let mut exec_ctx = BindContext::new();

    if let Err(e) = ctx.add_program_str("entry", &prog_str) {
        if let Some(observer) = &observer {
            observer.call1(py, (None::<String>, Some(Vec::<(i64, u32)>::new())))?;
        }
        return Err(PyCelError::new(e).into());
    }

    for keyobj in bindings.keys() {
        let key = keyobj.cast::<PyString>()?;

        let val = bindings.get_item(key).unwrap().unwrap();

        if !val.is_callable() {
            exec_ctx.bind_param(key.to_str()?, val.extract::<PyCelValue>()?.into_inner())
        }
    }

    for callable in callables.iter() {
        exec_ctx.bind_func(&callable.0, &callable.1);
    }

    let scope = observer.as_ref().map(|_| rscel::clock::Scope::observe());
    let program = observer.as_ref().and_then(|_| {
        ctx.get_program("entry")
            .and_then(comparison_observer::program_json)
    });
    let res = ctx.exec("entry", &exec_ctx);
    if let Some(observer) = observer {
        let clocks = scope.and_then(rscel::clock::Scope::finish);
        // Python collector suppresses ordinary diagnostic errors. A physical
        // connection loss must propagate to normal owner recovery.
        observer.call1(py, (program, clocks))?;
    }

    match res {
        Ok(res) => Ok(PyCelValue::new(res).into_pyobject_or_pyerr(py)?.unbind()),
        Err(err) => Err(PyException::new_err(err.to_string())),
    }
}

#[pyfunction]
fn to_sql(py: Python<'_>, prog_str: String) -> PyResult<Py<PyString>> {
    let p = match Program::from_source(&prog_str) {
        Ok(p) => p,
        Err(err) => return Err(PyException::new_err(err.to_string())),
    };

    let sql_str = if let Some(ast) = p.ast() {
        match ast.into_sql_builder() {
            Ok(b) => b.to_sql(),
            Err(e) => return Err(PyException::new_err(e.to_string())),
        }
    } else {
        return Err(PyException::new_err("Internal Error".to_string()));
    };

    match sql_str {
        Ok(s) => Ok(PyString::new(py, &s).unbind()),
        Err(e) => Err(PyException::new_err(e.to_string())),
    }
}

/* Module decl */
#[pymodule]
#[pyo3(name = "rscel")]
fn rscel_module(_py: Python<'_>, m: &Bound<PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(eval, m)?)?;
    m.add_function(wrap_pyfunction!(_eval_observed, m)?)?;
    m.add_class::<comparison_observer::ClockObservation>()?;
    m.add_function(wrap_pyfunction!(to_sql, m)?)?;
    m.add_class::<PyCelContext>()?;
    m.add_class::<PyBindContext>()?;
    m.add_class::<PyCelProgram>()?;
    Ok(())
}
