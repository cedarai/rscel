mod eval_error;
mod eval_result;
mod from_jsvalue;
mod into_jsvalue;
mod object_iter;
mod utils;
mod wasm_program_details;

use eval_error::WasmCelError;
use eval_result::EvalResult;
use from_jsvalue::WasmCelValue;
use object_iter::ObjectIterator;
use rscel::{BindContext, CelCompiler, CelContext, StringTokenizer};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    pub fn log(msg: &str);

    #[wasm_bindgen(js_namespace = Object)]
    fn keys(obj: &JsValue) -> js_sys::Array;

    #[wasm_bindgen(js_namespace = Object)]
    fn values(obj: &JsValue) -> js_sys::Array;

    #[wasm_bindgen(js_namespace = Object)]
    fn hasOwnProperty(obj: &JsValue, property: &str) -> bool;
}

#[wasm_bindgen]
pub fn cel_eval(prog: &str, binding: JsValue) -> JsValue {
    let mut ctx = CelContext::new();
    let mut exec_ctx = BindContext::new();

    if let Err(err) = ctx.add_program_str("entry", prog) {
        return EvalResult::from_error(WasmCelError::new(err).into()).into();
    }

    for (key, value) in ObjectIterator::new(binding.into()) {
        match TryInto::<WasmCelValue>::try_into(value) {
            Ok(celval) => exec_ctx.bind_param(&key, celval.into_inner()),
            Err(err) => return EvalResult::from_error(WasmCelError::new(err).into()).into(),
        }
    }

    let res = ctx.exec("entry", &exec_ctx);

    match res {
        Ok(ok) => EvalResult::from_value(ok),
        Err(err) => EvalResult::from_error(WasmCelError::new(err).into()),
    }
    .into()
}

#[wasm_bindgen]
pub fn cel_details(source: &str) -> JsValue {
    let mut tokenizer = StringTokenizer::with_input(source);
    match CelCompiler::with_tokenizer(&mut tokenizer).compile() {
        Ok(mut prog) => {
            let default_bindings = BindContext::new();

            prog.details_mut().filter_from_bindings(&default_bindings);

            EvalResult::from_program(prog).into()
        }
        Err(err) => EvalResult::from_error(WasmCelError::new(err).into()).into(),
    }
}
