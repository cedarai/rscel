pub use super::types::{ByteCode, JmpWhen};
use crate::{types::CelByteCode, CelValueDyn};
use std::{cell::Cell, collections::HashMap, fmt, rc::Rc, time::Instant};

use crate::{
    context::construct_type, utils::ScopedCounter, BindContext, CelContext, CelError, CelResult,
    CelValue, RsCelFunction, RsCelMacro,
};

struct InterpStack<'a, 'b> {
    stack: Vec<CelValue>,

    ctx: &'a Interpreter<'b>,
}

impl<'a, 'b> InterpStack<'a, 'b> {
    fn new(ctx: &'b Interpreter) -> InterpStack<'a, 'b> {
        InterpStack {
            stack: Vec::new(),
            ctx,
        }
    }

    fn push(&mut self, val: CelValue) {
        self.stack.push(val);
    }

    fn pop_noresolve(&mut self) -> CelResult<CelValue> {
        match self.stack.pop() {
            Some(v) => Ok(v),
            None => Err(CelError::runtime("No value on stack!")),
        }
    }

    fn pop(&mut self) -> CelResult<CelValue> {
        match self.stack.pop() {
            Some(stack_val) => {
                if let CelValue::Ident(name) = stack_val {
                    if let Some(val) = self.ctx.get_type_by_name(&name) {
                        self.ctx.before_clone(val)?;
                        return Ok(val.clone());
                    }

                    if let Some(val) = self.ctx.get_param_by_name(&name) {
                        self.ctx.before_clone(val)?;
                        return Ok(val.clone());
                    }

                    if let Some(ctx) = self.ctx.cel {
                        // Allow for loaded programs to run as values
                        if let Some(prog) = ctx.get_program(&name) {
                            return self.ctx.run_raw(prog.bytecode(), true).map(|x| x.into());
                        }
                    }

                    Ok(CelValue::from_err(CelError::binding(&name)).into())
                } else {
                    Ok(stack_val)
                }
            }
            None => Err(CelError::runtime("No value on stack!")),
        }
    }

    fn pop_tryresolve(&mut self) -> CelResult<CelValue> {
        match self.stack.pop() {
            Some(val) => match val {
                CelValue::Ident(name) => {
                    if let Some(val) = self.ctx.get_param_by_name(&name) {
                        self.ctx.before_clone(val)?;
                        Ok(val.clone())
                    } else {
                        Ok(CelValue::from_ident(&name))
                    }
                }
                other => Ok(other),
            },
            None => Err(CelError::runtime("No value on stack!")),
        }
    }
}

impl<'a, 'b> fmt::Debug for InterpStack<'a, 'b> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.stack)
    }
}

/// Optional cooperative execution budget. Legacy exec remains unbounded.
/// The same state is inherited by nested macro interpreters, not reset per item.
#[derive(Clone)]
pub struct ExecutionBudget {
    remaining: Rc<Cell<usize>>,
    exhausted: Rc<Cell<bool>>,
    heap_remaining: Rc<Cell<usize>>,
    refusal: Rc<Cell<Option<&'static str>>>,
    parser_depth: Rc<Cell<usize>>,
    prefix_depth: Rc<Cell<usize>>,
    ast_boxes_remaining: Rc<Cell<usize>>,
    deadline: Instant,
}

pub(crate) struct ParserDepth {
    depth: Rc<Cell<usize>>,
}
impl Drop for ParserDepth {
    fn drop(&mut self) {
        self.depth.set(self.depth.get() - 1);
    }
}
impl ExecutionBudget {
    pub(crate) fn new(instructions: usize, deadline: Instant) -> Self {
        Self {
            remaining: Rc::new(Cell::new(instructions)),
            exhausted: Rc::new(Cell::new(false)),
            heap_remaining: Rc::new(Cell::new(4 * 1024 * 1024)),
            refusal: Rc::new(Cell::new(None)),
            parser_depth: Rc::new(Cell::new(0)),
            prefix_depth: Rc::new(Cell::new(0)),
            ast_boxes_remaining: Rc::new(Cell::new(128)),
            deadline,
        }
    }
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted.get()
    }
    pub(crate) fn failure(&self) -> Option<CelError> {
        self.refusal.get().map(CelError::runtime)
    }
    pub(crate) fn admit_source(&self, source: &str) -> CelResult<()> {
        self.charge()?;
        if source.len() > 512 * 1024 {
            self.refuse("CEL resource limit exceeded")?;
        }
        Ok(())
    }
    pub(crate) fn enter_parser(&self) -> CelResult<ParserDepth> {
        // Recursive-descent frames are large in unoptimized builds. Keep this
        // conservative on ordinary native test/worker thread stacks as well.
        self.enter_depth(&self.parser_depth, 8)
    }
    pub(crate) fn enter_prefix(&self) -> CelResult<ParserDepth> {
        self.enter_depth(&self.prefix_depth, 64)
    }
    pub(crate) fn reserve_ast_box(&self) -> CelResult<()> {
        self.charge()?;
        let Some(left) = self.ast_boxes_remaining.get().checked_sub(1) else {
            return self.refuse("CEL resource limit exceeded");
        };
        self.ast_boxes_remaining.set(left);
        Ok(())
    }
    fn enter_depth(&self, depth: &Rc<Cell<usize>>, limit: usize) -> CelResult<ParserDepth> {
        self.charge()?;
        if depth.get() >= limit {
            self.refuse("CEL resource limit exceeded")?;
        }
        depth.set(depth.get() + 1);
        Ok(ParserDepth {
            depth: Rc::clone(depth),
        })
    }
    fn refuse(&self, reason: &'static str) -> CelResult<()> {
        self.refusal.set(Some(reason));
        Err(CelError::runtime(reason))
    }
    fn reserve_heap(&self, bytes: Option<usize>) -> CelResult<()> {
        self.charge()?;
        let Some(left) = bytes.and_then(|n| self.heap_remaining.get().checked_sub(n)) else {
            return self.refuse("CEL resource limit exceeded");
        };
        self.heap_remaining.set(left);
        Ok(())
    }
    fn value_cost(&self, value: &CelValue, depth: usize) -> CelResult<usize> {
        self.charge()?;
        if depth > 64 {
            self.refuse("CEL resource limit exceeded")?;
        }
        let mut size = 64usize;
        let extra = match value {
            CelValue::String(v) | CelValue::Ident(v) | CelValue::Type(v) => v.len(),
            CelValue::Bytes(v) => v.len(),
            CelValue::Err(error) => match error.owned_text_bytes() {
                Some(bytes) => bytes,
                None => {
                    self.refuse("CEL resource limit exceeded")?;
                    0
                }
            },
            CelValue::List(v) => {
                for item in v {
                    size = size
                        .checked_add(self.value_cost(item, depth + 1)?)
                        .unwrap_or(usize::MAX);
                }
                0
            }
            CelValue::Map(v) => {
                for (key, item) in v {
                    let child = self.value_cost(item, depth + 1)?;
                    size = size.saturating_add(key.len()).saturating_add(child);
                }
                0
            }
            // Dyn values are Arc-cloned. Their provider graph is separately
            // admitted by the native activation owner,not cloned by this meter.
            CelValue::ByteCode(v) => {
                for operation in v.as_slice() {
                    if let ByteCode::Push(item) = operation {
                        size = size.saturating_add(self.value_cost(item, depth + 1)?);
                    }
                }
                v.len().saturating_mul(64)
            }
            #[cfg(feature = "protobuf")]
            CelValue::Message(_) => {
                self.refuse("CEL resource limit exceeded")?;
                0
            }
            _ => 0,
        };
        Ok(size.saturating_add(extra))
    }
    pub(crate) fn charge(&self) -> CelResult<()> {
        if let Some(error) = self.failure() {
            return Err(error);
        }
        if self.exhausted.get() || self.remaining.get() == 0 || Instant::now() >= self.deadline {
            self.exhausted.set(true);
            return Err(CelError::runtime("CEL execution budget exceeded"));
        }
        self.remaining.set(self.remaining.get() - 1);
        Ok(())
    }
}

pub struct Interpreter<'a> {
    cel: Option<&'a CelContext>,
    bindings: Option<&'a BindContext<'a>>,
    depth: ScopedCounter,
    budget: Option<ExecutionBudget>,
}

impl<'a> Interpreter<'a> {
    pub fn new(cel: &'a CelContext, bindings: &'a BindContext) -> Interpreter<'a> {
        Interpreter {
            cel: Some(cel),
            bindings: Some(bindings),
            depth: ScopedCounter::new(),
            budget: None,
        }
    }

    pub fn empty() -> Interpreter<'a> {
        Interpreter {
            cel: None,
            bindings: None,
            depth: ScopedCounter::new(),
            budget: None,
        }
    }

    /// Create a child which shares the parent's total budget across macro calls.
    pub fn child(
        parent: &Interpreter,
        cel: &'a CelContext,
        bindings: &'a BindContext,
    ) -> Interpreter<'a> {
        let mut child = Self::new(cel, bindings);
        child.budget = parent.budget.clone();
        child
    }

    pub fn set_execution_budget(&mut self, instructions: usize, deadline: Instant) {
        self.budget = Some(ExecutionBudget::new(instructions, deadline));
    }

    pub(crate) fn set_budget_handle(&mut self, budget: ExecutionBudget) {
        self.budget = Some(budget);
    }

    /// Preserve eval_ident's isolated bindings while sharing bounded caller work.
    pub fn eval_ident(&self, prog: &CelByteCode) -> CelResult<String> {
        let mut interp = Self::empty();
        interp.budget = self.budget.clone();
        match interp.run_raw(prog, false)? {
            CelValue::Ident(ident) => Ok(ident),
            _ => Err(CelError::misc("ident required")),
        }
    }

    fn before_clone(&self, value: &CelValue) -> CelResult<()> {
        if let Some(budget) = &self.budget {
            budget.reserve_heap(Some(budget.value_cost(value, 0)?))?;
        }
        Ok(())
    }
    pub fn execution_failure(&self) -> Option<CelError> {
        self.budget.as_ref().and_then(ExecutionBudget::failure)
    }
    fn before_function(&self, name: &str, this: &CelValue, args: &[CelValue]) -> CelResult<()> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };
        budget.charge()?;
        if name == "now" {
            return budget.refuse("CEL clock provenance unavailable");
        }
        let mut size = budget.value_cost(this, 0)?;
        for arg in args {
            size = size.saturating_add(budget.value_cost(arg, 0)?);
        }
        budget.reserve_heap(Some(size))?;
        let text = match this {
            CelValue::String(v) => Some(v),
            _ => None,
        };
        let count = |v: &CelValue| match v {
            CelValue::Int(n) => usize::try_from(*n).ok(),
            CelValue::UInt(n) => usize::try_from(*n).ok(),
            _ => None,
        };
        match name {
            "padStart" | "padEnd" => {
                if let (Some(text), Some(width)) = (text.as_ref(), args.first().and_then(count)) {
                    budget.reserve_heap(
                        width.checked_mul(4).and_then(|v| v.checked_add(text.len())),
                    )?;
                } else if matches!(args.first(), Some(CelValue::Int(_) | CelValue::UInt(_))) {
                    budget.reserve_heap(None)?;
                }
            }
            "repeat" => {
                if let (Some(text), Some(n)) = (text.as_ref(), args.first().and_then(count)) {
                    budget.reserve_heap(text.len().checked_mul(n))?;
                }
            }
            "replace" | "replaceI" | "matchReplace" | "matchReplaceOnce" => {
                if let (Some(text), Some(CelValue::String(rep))) = (text.as_ref(), args.get(1)) {
                    budget.reserve_heap(text.len().checked_add(1).and_then(|n| {
                        n.checked_mul(rep.len().saturating_mul(2).saturating_add(1))
                    }))?;
                }
            }
            "split" | "rsplit" | "splitAt" | "splitWhiteSpace" => {
                if let Some(text) = text.as_ref() {
                    budget
                        .reserve_heap(text.len().checked_add(1).and_then(|n| n.checked_mul(64)))?;
                }
            }
            "matchCaptures" | "matchCapturesAll" => {
                if let (Some(text), Some(CelValue::String(pattern))) = (text.as_ref(), args.first())
                {
                    budget.reserve_heap(
                        text.len()
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(pattern.len().saturating_add(1)))
                            .and_then(|n| n.checked_mul(64)),
                    )?;
                }
            }
            "format" => {
                let mut factor = 128usize;
                let mut length = 1usize;
                for arg in args {
                    if let CelValue::String(spec) = arg {
                        length = length.saturating_add(spec.len());
                        for run in spec
                            .split(|c: char| !c.is_ascii_digit())
                            .filter(|s| !s.is_empty())
                        {
                            let Some(value) =
                                run.parse::<usize>().ok().filter(|v| *v <= 512 * 1024)
                            else {
                                return budget.refuse("CEL resource limit exceeded");
                            };
                            factor = factor.max(value);
                        }
                    }
                }
                budget.reserve_heap(
                    length
                        .checked_mul(factor)
                        .and_then(|n| n.checked_add(size.saturating_mul(6))),
                )?;
            }
            "toLower" | "toUpper" => {
                if let Some(text) = text.as_ref() {
                    budget.reserve_heap(text.len().checked_mul(4))?;
                }
            }
            _ => (),
        }
        // repeat allocates before the next instruction. Bound that allocation
        // before both compile-time constant folding and runtime execution.
        if name == "repeat" {
            let (text, count) = match (this, args) {
                (CelValue::String(text), [count]) => (Some(text), Some(count)),
                (CelValue::Null, [CelValue::String(text), count]) => (Some(text), Some(count)),
                _ => (None, None),
            };
            if let (Some(text), Some(count)) = (text, count) {
                let count = match count {
                    CelValue::Int(n) => usize::try_from(*n).ok(),
                    CelValue::UInt(n) => usize::try_from(*n).ok(),
                    _ => return Ok(()),
                };
                if count
                    .and_then(|n| text.len().checked_mul(n))
                    .is_none_or(|n| n > 512 * 1024)
                {
                    budget.exhausted.set(true);
                    return Err(CelError::runtime("CEL execution budget exceeded"));
                }
            }
        }
        Ok(())
    }

    pub fn execution_budget_exhausted(&self) -> bool {
        self.budget.as_ref().is_some_and(|b| b.exhausted.get())
    }

    pub fn add_bindings(&mut self, bindings: &'a BindContext) {
        self.bindings = Some(bindings);
    }

    pub fn cel_copy(&self) -> Option<CelContext> {
        self.cel.cloned()
    }

    pub fn bindings_copy(&self) -> Option<BindContext<'_>> {
        self.bindings.cloned()
    }

    pub fn run_program(&self, name: &str) -> CelResult<CelValue> {
        match self.cel {
            Some(cel) => match cel.get_program(name) {
                Some(prog) => self.run_raw(prog.bytecode(), true),
                None => Err(CelError::binding(&name)),
            },
            None => Err(CelError::internal("No CEL context bound to interpreter")),
        }
    }

    pub fn run_raw(&self, prog: &CelByteCode, resolve: bool) -> CelResult<CelValue> {
        let mut pc: usize = 0;
        let mut stack = InterpStack::new(self);

        let count = self.depth.inc();

        if count.count() > 32 {
            return Err(CelError::runtime("Max call depth excceded"));
        }

        while pc < prog.len() {
            if let Some(budget) = &self.budget {
                budget.charge()?;
            }
            let oldpc = pc;
            pc += 1;
            match &prog[oldpc] {
                ByteCode::Push(val) => {
                    self.before_clone(val)?;
                    stack.push(val.clone())
                }
                ByteCode::Pop => {
                    stack.pop()?;
                }
                ByteCode::Test => {
                    let v = stack.pop()?;

                    if v.is_err() {
                        stack.push(v);
                    } else if cfg!(feature = "type_prop") {
                        stack.push(v.is_truthy().into());
                    } else if let CelValue::Bool(b) = v {
                        stack.push(b.into())
                    } else {
                        return Err(CelError::invalid_op(&format!(
                            "TEST invalid on type {:?}",
                            v.as_type()
                        )));
                    }
                }
                ByteCode::Dup => {
                    let v = stack.pop()?;

                    self.before_clone(&v)?;
                    stack.push(v.clone());
                    stack.push(v);
                }
                ByteCode::Or => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.or(&v2))
                }
                ByteCode::And => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.and(v2))
                }
                ByteCode::Not => {
                    let v1 = stack.pop()?;

                    stack.push(!v1);
                }
                ByteCode::Neg => {
                    let v1 = stack.pop()?;

                    stack.push(-v1);
                }
                ByteCode::Add => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    self.before_clone(&v1)?;
                    self.before_clone(&v2)?;
                    stack.push(v1 + v2);
                }
                ByteCode::Sub => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1 - v2);
                }
                ByteCode::Mul => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1 * v2);
                }
                ByteCode::Div => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1 / v2);
                }
                ByteCode::Mod => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1 % v2);
                }
                ByteCode::Lt => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.lt(v2));
                }
                ByteCode::Le => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.le(v2));
                }
                ByteCode::Eq => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(CelValueDyn::eq(&v1, &v2));
                }
                ByteCode::Ne => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.neq(v2));
                }
                ByteCode::Ge => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.ge(v2));
                }
                ByteCode::Gt => {
                    let v2 = stack.pop()?;
                    let v1 = stack.pop()?;

                    stack.push(v1.gt(v2));
                }
                ByteCode::In => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;

                    stack.push(lhs.in_(rhs));
                }
                ByteCode::Jmp(dist) => pc = Self::checked_jump_target(pc, *dist, prog.len())?,
                ByteCode::JmpCond { when, dist } => {
                    let v1 = stack.pop()?;
                    match v1 {
                        CelValue::Bool(b) => {
                            if b == when.as_bool() {
                                pc = Self::checked_jump_target(pc, *dist, prog.len())?
                            }
                        }
                        CelValue::Err(_) => {
                            if *when == JmpWhen::False {
                                pc = Self::checked_jump_target(pc, *dist, prog.len())?
                            }
                        }
                        v => {
                            return Err(CelError::invalid_op(&format!(
                                "JMP TRUE invalid on type {:?}",
                                v.as_type()
                            )))
                        }
                    }
                }
                ByteCode::MkList(size) => {
                    if let Some(budget) = &self.budget {
                        budget.reserve_heap(
                            usize::try_from(*size).ok().and_then(|s| s.checked_mul(64)),
                        )?;
                    }
                    let mut v = Vec::new();

                    for _ in 0..*size {
                        v.push(stack.pop()?)
                    }

                    v.reverse();
                    stack.push(v.into());
                }
                ByteCode::MkDict(size) => {
                    if let Some(budget) = &self.budget {
                        budget.reserve_heap(
                            usize::try_from(*size).ok().and_then(|s| s.checked_mul(96)),
                        )?;
                    }
                    let mut map = HashMap::new();

                    for _ in 0..*size {
                        let key = if let CelValue::String(key) = stack.pop()? {
                            key
                        } else {
                            return Err(CelError::value("Only strings can be used as Object keys"));
                        };

                        map.insert(key, stack.pop()?);
                    }

                    stack.push(map.into());
                }
                ByteCode::Index => {
                    let index = stack.pop()?;
                    let obj = stack.pop()?;

                    stack.push(obj.index(index));
                }
                ByteCode::Access => {
                    let index = stack.pop_noresolve()?;
                    if let CelValue::Ident(ident) = index {
                        let obj = stack.pop()?;
                        match obj {
                            CelValue::Map(ref map) => match map.get(ident.as_str()) {
                                Some(val) => {
                                    self.before_clone(val)?;
                                    stack.push(val.clone())
                                }
                                None => {
                                    stack.push(
                                        CelValue::from_err(CelError::attribute(
                                            "obj",
                                            ident.as_str(),
                                        ))
                                        .into(),
                                    );
                                }
                            },
                            #[cfg(feature = "protobuf")]
                            CelValue::Message(msg) => {
                                let desc = msg.descriptor_dyn();

                                if let Some(field) = desc.field_by_name(ident.as_str()) {
                                    stack.push(
                                        field.get_singular_field_or_default(msg.as_ref()).into(),
                                    )
                                } else {
                                    return Err(CelError::attribute("msg", ident.as_str()));
                                }
                            }
                            CelValue::Dyn(d) => {
                                stack.push(d.access(ident.as_str()));
                            }
                            CelValue::Err(e) => stack.push(CelValue::Err(e)),
                            obj => stack.push(
                                CelValue::from_err(CelError::value(&format!(
                                    "Access operator invalid between {:?} and IDENT",
                                    obj.as_type(),
                                )))
                                .into(),
                            ),
                        }
                    } else {
                        let obj_type = stack.pop()?.as_type();
                        stack.push(
                            CelValue::from_err(CelError::value(&format!(
                                "Access operator invalid between {:?} and {:?}",
                                obj_type,
                                index.as_type(),
                            )))
                            .into(),
                        );
                    }
                }
                ByteCode::CallMethod(n_args) => {
                    let method_name = stack.pop_noresolve()?;
                    let this = stack.pop()?;
                    let mut args = Vec::new();

                    for _ in 0..*n_args {
                        args.push(stack.pop()?)
                    }
                    match method_name {
                        CelValue::Err(e) => {
                            return Err(e);
                        }
                        CelValue::Ident(func_name) => {
                            if let Some(func) = self.get_func_by_name(&func_name) {
                                let arg_values = self.resolve_args(args)?;
                                self.before_function(&func_name, &this, &arg_values)?;
                                stack.push(func(this, arg_values));
                            } else if let Some(macro_) = self.get_macro_by_name(&func_name) {
                                stack.push(self.call_macro(&this, &args, macro_)?);
                            } else {
                                stack.push(CelValue::from_err(CelError::runtime(&format!(
                                    "{} is not a method",
                                    func_name
                                ))));
                            }
                        }
                        CelValue::Type(type_name) => {
                            let arg_values = self.resolve_args(args)?;
                            stack.push(construct_type(&type_name, arg_values));
                        }
                        other => stack.push(
                            CelValue::from_err(CelError::runtime(&format!(
                                "{:?} cannot be called",
                                other
                            )))
                            .into(),
                        ),
                    };
                }
                ByteCode::Call(n_args) => {
                    let func_name = stack.pop_noresolve()?;
                    let mut args = Vec::new();

                    for _ in 0..*n_args {
                        args.push(stack.pop()?)
                    }

                    match func_name {
                        CelValue::Err(e) => {
                            return Err(e);
                        }
                        CelValue::Ident(func_name) => {
                            if let Some(func) = self.get_func_by_name(&func_name) {
                                let arg_values = self.resolve_args(args)?;
                                self.before_function(&func_name, &CelValue::Null, &arg_values)?;
                                stack.push(func(CelValue::from_null(), arg_values));
                            } else if let Some(macro_) = self.get_macro_by_name(&func_name) {
                                stack.push(self.call_macro(
                                    &CelValue::from_null(),
                                    &args,
                                    macro_,
                                )?);
                            } else if let Some(CelValue::Type(type_name)) =
                                self.get_type_by_name(&func_name)
                            {
                                let arg_values = self.resolve_args(args)?;
                                stack.push(construct_type(type_name, arg_values));
                            } else {
                                stack.push(CelValue::from_err(CelError::runtime(&format!(
                                    "{} is not callable",
                                    func_name
                                ))));
                            }
                        }
                        CelValue::Type(type_name) => {
                            let arg_values = self.resolve_args(args)?;
                            stack.push(construct_type(&type_name, arg_values));
                        }
                        other => stack.push(
                            CelValue::from_err(CelError::runtime(&format!(
                                "{:?} cannot be called",
                                other
                            )))
                            .into(),
                        ),
                    };
                }
                ByteCode::FmtString(nsegments) => {
                    let mut segments = Vec::new();
                    for _ in 0..*nsegments {
                        segments.push(stack.pop()?);
                    }

                    let mut working = String::new();
                    for seg in segments.into_iter().rev() {
                        if let CelValue::String(s) = seg {
                            if let Some(budget) = &self.budget {
                                budget.reserve_heap(Some(s.len()))?;
                            }
                            working.push_str(&s)
                        } else {
                            return Err(CelError::Runtime(
                                "Expected string from format string specifier".to_string(),
                            ));
                        }
                    }

                    stack.push(CelValue::String(working));
                }
            };
        }

        if resolve {
            stack.pop()?.into_result()
        } else {
            match stack.pop_tryresolve() {
                Ok(val) => val.into_result(),
                Err(err) => Err(err),
            }
        }
    }

    fn call_macro(
        &self,
        this: &CelValue,
        args: &Vec<CelValue>,
        macro_: &RsCelMacro,
    ) -> Result<CelValue, CelError> {
        let mut v = Vec::new();
        for arg in args.iter() {
            if let CelValue::ByteCode(bc) = arg {
                v.push(bc);
            } else {
                return Err(CelError::internal("macro args must be bytecode"));
            }
        }
        self.before_clone(this)?;
        let res = macro_(self, this.clone(), &v);
        Ok(res)
    }

    fn resolve_args(&self, args: Vec<CelValue>) -> Result<Vec<CelValue>, CelError> {
        let mut arg_values = Vec::new();
        for arg in args.into_iter() {
            if let CelValue::ByteCode(bc) = arg {
                arg_values.push(self.run_raw(&bc, true)?);
            } else {
                arg_values.push(arg)
            }
        }
        Ok(arg_values)
    }

    fn get_param_by_name(&self, name: &str) -> Option<&'a CelValue> {
        self.bindings?.get_param(name)
    }

    fn get_func_by_name(&self, name: &str) -> Option<&'a RsCelFunction> {
        self.bindings?.get_func(name)
    }

    fn get_macro_by_name(&self, name: &str) -> Option<&'a RsCelMacro> {
        self.bindings?.get_macro(name)
    }

    fn get_type_by_name(&self, name: &str) -> Option<&'a CelValue> {
        self.bindings?.get_type(name)
    }

    fn checked_jump_target(pc: usize, dist: i32, len: usize) -> CelResult<usize> {
        let target = (pc as isize)
            .checked_add(dist as isize)
            .ok_or_else(|| CelError::runtime("Jump target out of range"))?;

        if target < 0 || target > len as isize {
            Err(CelError::runtime("Jump target out of range"))
        } else {
            Ok(target as usize)
        }
    }
}
