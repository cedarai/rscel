# rscel

RsCel is a CEL evaluator written in Rust. CEL is a Turing-incomplete
language that can be used to evaluate
a user-provided expression. The language specification can be found
[here](https://github.com/google/cel-spec/blob/master/doc/langdef.md).

The design goals of this project are as follows:

- Isolated execution of CEL expressions
- Flexible enough to allow for a user to bend the spec if needed
- Can be used as a WASM depenedency (or other FFI)

The basic example of how to use:

```rust
use rscel::{CelContext, BindContext};

let mut ctx = CelContext::new();
let mut exec_ctx = BindContext::new();

ctx.add_program_str("main", "foo + 3").unwrap();
exec_ctx.bind_param("foo", 3.into()); // convert to CelValue

let res = ctx.exec("main", &exec_ctx).unwrap(); // CelValue::Int(6)
assert_eq!(res, 6.into());
```

As of 0.10.0, binding protobuf messages from the protobuf crate is now available! Given
the following protobuf message:

```protobuf

message Point {
  int32 x = 1;
  int32 y = 2;
}

```

The following code can be used to evaluate a CEL expression on a Point message:

```rust
use rscel::{CelContext, BindContext};

// currently rscel requires protobuf messages to be in a box
let p = Box::new(protos::Point::new());
p.x = 4;
p.y = 5;

let mut ctx = CelContext::new();
let mut exec_ctx = BindContext::new();

ctx.add_program_str("main", "p.x + 3").unwrap();
exec_ctx.bind_protobuf_msg("p", p);

assert_eq!(ctx.exec("main", &exec_ctx), 7.into());

```

Build status: [![Rust](https://github.com/1BADragon/rscel/actions/workflows/rust.yml/badge.svg)](https://github.com/1BADragon/rscel/actions/workflows/rust.yml)

## Cedar fork: bounded comparison support

`CelContext::exec_bounded`, `eval_bounded` and `compile_bounded` opt into a
cooperative deadline and shared instruction/resource accounting, including nested
macros and compilation-time constant folding. Resource refusals remain sticky
when an expression tries to mask an error. Bounded compilation admits at most
512 KiB of source, 128 recursive AST boxes, 8 active full-expression levels and
64 prefix-operator levels before parsing
can recurse further. Owned error text is included in clone accounting.
Bounded runtime entries share an eight-entry active-depth guard across macro
children, named-program references and isolated identifier evaluation. This
corrects the earlier per-interpreter counter: recursive named programs could
reset it through macro children and overflow the process stack. Depth refusal
is sticky, and its owned scope is released on every return.
Normal `exec`, compilation and Python
`eval` keep their existing semantics. These are cooperative guards, not hard
process CPU or memory limits; caller-provided functions and provider graphs
still require their own admission and lifetime controls.

Callers can capture one comparison start time and leave unaligned time-sensitive
cases unverified. Bounded evaluation refuses live `now` and `clockNow` calls;
cached timestamp literals remain usable without reading a new clock. Ordinary
evaluation uses the upstream runtime: `now()` is fixed within each execution,
`clockNow()` reads the live clock, and Python may supply an `InterpRuntime`.
Per-read clock replay was tried and removed because unaligned comparisons can
remain explicitly unverified.

`comparison::snapshot` and `restore` use a versioned, bounded internal format
that preserves float bits and nanosecond temporal constants, including nested
bytecode and typed macro binders. This format is separate from public program JSON;
upstream removed the bincode APIs. `CelValue::from_timestamp_text` and `from_duration_micros` let
native callers construct precise values without sharing this crate's chrono
version.

Python's private `_eval_observed` and
`CelProgram._comparison_json` expose program evidence and an explicit compile-failure flag without replacing normal
evaluation results or errors. Observer callback errors propagate to the caller,
which must handle its own diagnostic failures and connection recovery. The
existing CPython stable ABI floor remains 3.11.
