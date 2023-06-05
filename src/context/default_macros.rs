use crate::{
    interp::Interpreter,
    utils::eval_ident,
    value_cell::{ValueCell, ValueCellError, ValueCellResult},
    BindContext, ByteCode, CelContext, ValueCellInner,
};

use super::bind_context::RsCellMacro;

const DEFAULT_MACROS: &[(&str, RsCellMacro)] = &[
    ("has", has_impl),
    ("all", all_impl),
    ("exists", exists_impl),
    ("exists_one", exists_one_impl),
    ("filter", filter_impl),
    ("map", map_impl),
];

pub fn load_default_macros(exec_ctx: &mut BindContext) {
    for (name, macro_) in DEFAULT_MACROS.iter() {
        exec_ctx.bind_macro(name, *macro_)
    }
}

fn has_impl(
    ctx: &Interpreter,
    _this: ValueCell,
    exprlist: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if exprlist.len() != 1 {
        return Err(ValueCellError::with_msg(
            "has() macro expects exactly 1 argument",
        ));
    }

    match ctx.run_raw(&exprlist[0]) {
        Ok(_) => Ok(ValueCell::from_bool(true)),
        Err(_) => Ok(ValueCell::from_bool(false)),
    }
}

fn all_impl(
    ctx: &Interpreter,
    this: ValueCell,
    bytecode: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if bytecode.len() != 2 {
        return Err(ValueCellError::with_msg(
            "all() macro expects exactly 2 arguments",
        ));
    }

    let ident_prog = ctx.run_raw(bytecode[0])?;
    let ident_name = if let ValueCellInner::Ident(ident) = ident_prog.inner() {
        ident
    } else {
        return Err(ValueCellError::with_msg("all() predicate must be ident"));
    };

    if let ValueCellInner::List(list) = this.inner() {
        let cel = ctx.cel_copy().unwrap_or_else(|| CelContext::new());
        let mut bindings = ctx.bindings_copy().unwrap_or_else(|| BindContext::new());

        for v in list.into_iter() {
            // make a copy of the context to make borrow checker happy
            bindings.bind_param(&ident_name, v.clone());
            let interp = Interpreter::new(&cel, &bindings);

            let res = interp.run_raw(bytecode[1])?;

            if let ValueCellInner::Bool(b) = res.into_inner() {
                if !b {
                    return Ok(false.into());
                }
            }
        }

        return Ok(true.into());
    } else {
        Err(ValueCellError::with_msg("all() only available on list"))
    }
}

fn exists_impl(
    ctx: &Interpreter,
    this: ValueCell,
    bytecode: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if bytecode.len() != 2 {
        return Err(ValueCellError::with_msg(
            "exists() macro expects exactly 2 arguments",
        ));
    }

    let ident_name = eval_ident(bytecode[0])?;

    if let ValueCellInner::List(list) = this.inner() {
        let cel = ctx.cel_copy().unwrap_or_else(|| CelContext::new());
        let mut bindings = ctx.bindings_copy().unwrap_or_else(|| BindContext::new());

        for v in list.into_iter() {
            bindings.bind_param(&ident_name, v.clone());
            let interp = Interpreter::new(&cel, &bindings);

            let res = interp.run_raw(bytecode[1])?;

            if let ValueCellInner::Bool(b) = res.into_inner() {
                if b {
                    return Ok(true.into());
                }
            }
        }

        return Ok(false.into());
    } else {
        Err(ValueCellError::with_msg("exists() only available on list"))
    }
}

fn exists_one_impl(
    ctx: &Interpreter,
    this: ValueCell,
    bytecode: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if bytecode.len() != 2 {
        return Err(ValueCellError::with_msg(
            "exists_one() macro expects exactly 2 arguments",
        ));
    }

    let ident_name = eval_ident(bytecode[0])?;

    if let ValueCellInner::List(list) = this.inner() {
        let cel = ctx.cel_copy().unwrap_or_else(|| CelContext::new());
        let mut bindings = ctx.bindings_copy().unwrap_or_else(|| BindContext::new());

        let mut count = 0;
        for v in list.into_iter() {
            bindings.bind_param(&ident_name, v.clone());
            let interp = Interpreter::new(&cel, &bindings);

            let res = interp.run_raw(bytecode[1])?;

            if let ValueCellInner::Bool(b) = res.into_inner() {
                if b {
                    count += 1;

                    if count > 1 {
                        return Ok(false.into());
                    }
                }
            }
        }

        return Ok((count == 1).into());
    } else {
        Err(ValueCellError::with_msg(
            "exists_one() only available on list",
        ))
    }
}

fn filter_impl(
    ctx: &Interpreter,
    this: ValueCell,
    bytecode: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if bytecode.len() != 2 {
        return Err(ValueCellError::with_msg(
            "filter() macro expects exactly 2 arguments",
        ));
    }

    let ident_name = eval_ident(bytecode[0])?;

    if let ValueCellInner::List(list) = this.inner() {
        let mut filtered_list: Vec<ValueCell> = Vec::new();
        let cel = ctx.cel_copy().unwrap_or_else(|| CelContext::new());
        let mut bindings = ctx.bindings_copy().unwrap_or_else(|| BindContext::new());

        for v in list.into_iter() {
            // make a copy of the context to make borrow checker happy
            bindings.bind_param(&ident_name, v.clone());
            let interp = Interpreter::new(&cel, &bindings);

            if let ValueCellInner::Bool(b) = interp.run_raw(bytecode[1])?.into_inner() {
                if b {
                    filtered_list.push(v.clone());
                }
            }
        }
        Ok(filtered_list.into())
    } else {
        Err(ValueCellError::with_msg("filter() only available on list"))
    }
}

fn map_impl(
    ctx: &Interpreter,
    this: ValueCell,
    bytecode: &[&[ByteCode]],
) -> ValueCellResult<ValueCell> {
    if bytecode.len() != 2 {
        return Err(ValueCellError::with_msg(
            "map() macro expects exactly 2 arguments",
        ));
    }

    let ident_name = eval_ident(bytecode[0])?;

    if let ValueCellInner::List(list) = this.inner() {
        let mut mapped_list: Vec<ValueCell> = Vec::new();
        let cel = ctx.cel_copy().unwrap_or_else(|| CelContext::new());
        let mut bindings = ctx.bindings_copy().unwrap_or_else(|| BindContext::new());

        for v in list.into_iter() {
            // make a copy of the context to make borrow checker happy
            bindings.bind_param(&ident_name, v.clone());
            let interp = Interpreter::new(&cel, &bindings);

            mapped_list.push(interp.run_raw(bytecode[1])?);
        }
        Ok(mapped_list.into())
    } else {
        Err(ValueCellError::with_msg("map() only available on list"))
    }
}