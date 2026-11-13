//! Opt-in budgets and observation must preserve authoritative unbounded execution.
use rscel::{clock, BindContext, CelContext, CelValue, Program};
use std::time::{Duration, Instant};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[test]
fn outer_replay_cannot_report_an_inner_scopes_completion() {
    let outer = clock::Scope::replay(vec![(1, 0)]).unwrap();
    {
        let inner = clock::Scope::replay(vec![]).unwrap();
        assert!(!outer.complete());
        assert!(inner.complete());
    }
    assert!(!outer.complete());
    CelContext::new()
        .eval_bounded("outer", "now()", &BindContext::new(), 100_000, deadline())
        .unwrap();
    assert!(outer.complete());
}

#[test]
fn out_of_order_finish_does_not_return_another_scope_or_resurrect_stale_state() {
    let outer = clock::Scope::observe();
    let inner = clock::Scope::observe();
    assert!(outer.finish().is_none());
    assert!(!inner.complete());
    assert!(inner.finish().is_none());
    assert!(!clock::replaying());
}

#[test]
fn dropping_an_outer_scope_invalidates_children_without_clobbering_a_new_scope() {
    let outer = clock::Scope::replay(vec![(1, 0)]).unwrap();
    let inner = clock::Scope::replay(vec![(2, 0)]).unwrap();
    drop(outer);
    assert!(!inner.complete());
    let fresh = clock::Scope::observe();
    drop(inner);
    let mut context = CelContext::new();
    context.add_program_str("fresh", "now()").unwrap();
    let CelValue::TimeStamp(actual) = context.exec("fresh", &BindContext::new()).unwrap() else {
        panic!("timestamp lost")
    };
    assert!(actual.timestamp() > 2);
    assert_eq!(fresh.finish().unwrap().len(), 1);
    assert!(!clock::replaying());
}

#[test]
fn nested_macros_share_one_budget_and_cannot_mask_exhaustion() {
    let mut context = CelContext::new();
    let mut bindings = BindContext::new();
    bindings.bind_param("xs", CelValue::List((1..=16).map(CelValue::Int).collect()));
    context
        .add_program_str("nested", "xs.exists(x, xs.all(y, x+y >= 0)) || true")
        .unwrap();
    assert_eq!(
        context.exec("nested", &bindings).unwrap(),
        CelValue::Bool(true)
    );
    let error = context
        .exec_bounded("nested", &bindings, 20, deadline())
        .unwrap_err();
    assert!(error.to_string().contains("CEL execution budget exceeded"));
    assert_eq!(
        context.exec("nested", &bindings).unwrap(),
        CelValue::Bool(true)
    );
}

#[test]
fn expired_compile_and_eval_do_not_receive_a_fresh_allowance() {
    let expired = Instant::now() - Duration::from_secs(1);
    assert!(CelContext::compile_bounded("true", 100_000, expired).is_err());
    let mut context = CelContext::new();
    assert!(context
        .eval_bounded("expired", "true", &BindContext::new(), 100_000, expired)
        .is_err());
    assert!(context.get_program("expired").is_none());
}

#[test]
fn bounded_compilation_caps_recursive_prefixes_ternaries_and_source_bytes() {
    let inputs = [
        format!("{}true", "!".repeat(4096)),
        format!("{}1", "-".repeat(4096)),
        format!("{}0", "false ? 0 : ".repeat(1000)),
        format!("{}true{}", "(".repeat(128), ")".repeat(128)),
        format!("'{}'", "x".repeat(512 * 1024)),
    ];
    for (case, source) in inputs.into_iter().enumerate() {
        eprintln!("bounded parser case {case}");
        let error = CelContext::compile_bounded(&source, 1_000_000, deadline()).unwrap_err();
        assert!(error.to_string().contains("CEL resource limit exceeded"));
    }
    let shallow = format!("{}0", "false ? 0 : ".repeat(4));
    let mut context = CelContext::new();
    assert_eq!(
        context
            .eval_bounded(
                "shallow",
                &shallow,
                &BindContext::new(),
                100_000,
                deadline()
            )
            .unwrap(),
        CelValue::Int(0)
    );
    context.add_program_str("legacy", &shallow).unwrap();
    assert_eq!(
        context.exec("legacy", &BindContext::new()).unwrap(),
        CelValue::Int(0)
    );
}

#[test]
fn owned_error_text_is_charged_before_a_prebuilt_program_clones_it() {
    for error in [
        rscel::CelError::Runtime("x".repeat(5 * 1024 * 1024)),
        rscel::CelError::Attribute {
            parent: "parent".into(),
            field: "x".repeat(5 * 1024 * 1024),
        },
    ] {
        let program = Program::new(
            rscel::ProgramDetails::new(),
            vec![rscel::ByteCode::Push(CelValue::from_err(error))].into(),
        );
        let mut context = CelContext::new();
        context.add_program("large_error", program);
        let error = context
            .exec_bounded("large_error", &BindContext::new(), 1_000_000, deadline())
            .unwrap_err();
        assert!(error.to_string().contains("CEL resource limit exceeded"));
    }
}

#[test]
fn large_builtin_allocations_are_refused_before_constant_folding_or_execution() {
    for (source, reason) in [
        ("'x'.repeat(1000000000)", "CEL resource limit exceeded"),
        ("repeat('x',1000000000)", "CEL execution budget exceeded"),
        ("'x'.padStart(1000000000)", "CEL resource limit exceeded"),
        (
            "('x'.padStart(1000000000) == '') || true",
            "CEL resource limit exceeded",
        ),
    ] {
        let error = CelContext::new()
            .eval_bounded("large", source, &BindContext::new(), 1_000_000, deadline())
            .unwrap_err();
        assert!(error.to_string().contains(reason), "{source}: {error}");
    }
    assert_eq!(
        CelContext::new()
            .eval_bounded(
                "small",
                "'x'.repeat(3)",
                &BindContext::new(),
                100_000,
                deadline()
            )
            .unwrap(),
        CelValue::String("xxx".into())
    );
}

#[test]
fn large_binding_clones_are_admitted_before_allocation_and_cannot_be_masked() {
    let mut bindings = BindContext::new();
    bindings.bind_param("large", CelValue::List(vec![CelValue::Null; 65_537]));
    let mut context = CelContext::new();
    context
        .add_program_str("large", "large == null || true")
        .unwrap();
    let error = context
        .exec_bounded("large", &bindings, 1_000_000, deadline())
        .unwrap_err();
    assert!(error.to_string().contains("CEL resource limit exceeded"));
}

#[test]
fn observation_records_live_clock_operands_and_replay_requires_exact_consumption() {
    let mut context = CelContext::new();
    let observation = clock::Scope::observe();
    context.add_program("clock", Program::from_source("now()").unwrap());
    let CelValue::TimeStamp(actual) = context.exec("clock", &BindContext::new()).unwrap() else {
        panic!("timestamp lost")
    };
    assert_eq!(
        observation.finish().unwrap(),
        vec![(actual.timestamp(), actual.timestamp_subsec_nanos())]
    );
    for clocks in [vec![(123, 456)], vec![], vec![(123, 456), (789, 1)]] {
        let replay = clock::Scope::replay(clocks.clone()).unwrap();
        let result = context.eval_bounded(
            "replayed",
            "now()",
            &BindContext::new(),
            100_000,
            deadline(),
        );
        assert_eq!(replay.complete(), clocks.len() == 1);
        if clocks.len() == 1 {
            let CelValue::TimeStamp(actual) = result.unwrap() else {
                panic!("timestamp lost")
            };
            assert_eq!(
                (actual.timestamp(), actual.timestamp_subsec_nanos()),
                (123, 456)
            );
        } else if clocks.is_empty() {
            assert!(result.is_err());
        }
    }
}

#[test]
fn nested_scopes_restore_outer_observation_and_reject_invalid_replay() {
    let mut context = CelContext::new();
    let outer = clock::Scope::observe();
    {
        let replay = clock::Scope::replay(vec![(123, 456)]).unwrap();
        context
            .eval_bounded("clock", "now()", &BindContext::new(), 100_000, deadline())
            .unwrap();
        assert!(replay.complete());
    }
    context.add_program_str("outer_clock", "now()").unwrap();
    context.exec("outer_clock", &BindContext::new()).unwrap();
    assert_eq!(outer.finish().unwrap().len(), 1);
    assert!(clock::Scope::replay(vec![(123, 0); 8_193]).is_err());
    assert!(clock::Scope::replay(vec![(i64::MAX, u32::MAX)]).is_err());
    assert!(!clock::replaying());
}

#[test]
fn cached_clock_constants_do_not_read_a_new_clock_when_executed_again() {
    let mut context = CelContext::new();
    let compiled = clock::Scope::observe();
    context.add_program_str("cached", "now()").unwrap();
    let first = context.exec("cached", &BindContext::new()).unwrap();
    assert_eq!(compiled.finish().unwrap().len(), 1);
    let executed = clock::Scope::observe();
    assert_eq!(context.exec("cached", &BindContext::new()).unwrap(), first);
    assert!(executed.finish().unwrap().is_empty());
}

#[cfg(test)]
mod lossless_tests {
    use rscel::{ByteCode, CelValue, Program, ProgramDetails};
    fn bits(v: &CelValue) -> String {
        match v {
            CelValue::Float(v) => format!("float:{}", v.to_bits()),
            CelValue::TimeStamp(v) => {
                format!("time:{}:{}", v.timestamp(), v.timestamp_subsec_nanos())
            }
            CelValue::Duration(v) => format!("duration:{}:{}", v.num_seconds(), v.subsec_nanos()),
            CelValue::List(v) => format!("{:?}", v.iter().map(bits).collect::<Vec<_>>()),
            CelValue::Map(v) => {
                let mut pairs = v.iter().map(|(k, v)| (k, bits(v))).collect::<Vec<_>>();
                pairs.sort();
                format!("{:?}", pairs)
            }
            CelValue::ByteCode(v) => format!(
                "{:?}",
                v.iter()
                    .map(|v| match v {
                        ByteCode::Push(v) => bits(v),
                        _ => format!("{:?}", v),
                    })
                    .collect::<Vec<_>>()
            ),
            _ => format!("{:?}", v),
        }
    }
    #[test]
    fn comparison_codec_preserves_nested_temporal_constants_and_all_float_bits() {
        let values = vec![
            CelValue::from_timestamp_text("2026-11-11T12:13:14.123456789Z").unwrap(),
            CelValue::from_timestamp_text("1969-12-31T23:59:59.999999999Z").unwrap(),
            CelValue::from_duration_micros(1234567),
            CelValue::from_duration_micros(-1234567),
            CelValue::from_duration_micros(-1),
            CelValue::Float(-0.0),
            CelValue::Float(f64::MAX),
            CelValue::Float(f64::INFINITY),
            CelValue::Float(f64::NEG_INFINITY),
            CelValue::Float(f64::from_bits(0x7ff8000000000123)),
        ];
        let list = CelValue::List(values);
        let map = CelValue::Map([(String::from("nested"), list)].into_iter().collect());
        let code = CelValue::ByteCode(vec![ByteCode::Push(map)].into());
        let expected = bits(&code);
        let program = Program::new(ProgramDetails::new(), vec![ByteCode::Push(code)].into());
        let encoded = rscel::comparison::snapshot(&program).unwrap();
        let restored = rscel::comparison::restore(&encoded).unwrap();
        let ByteCode::Push(actual) = &restored.bytecode()[0] else {
            panic!("lost Push")
        };
        assert_eq!(bits(actual), expected);
    }
    #[test]
    fn comparison_codec_refuses_large_borrowed_constants_without_mutating_originals() {
        let cap = 512 * 1024;
        let values = vec![
            CelValue::String("x".repeat(cap + 1)),
            CelValue::from_bytes(vec![b'x'; cap + 1]),
            CelValue::Map(
                [("x".repeat(cap + 1), CelValue::Null)]
                    .into_iter()
                    .collect(),
            ),
            CelValue::List(vec![
                CelValue::String("x".repeat(cap / 2)),
                CelValue::String("y".repeat(cap / 2)),
            ]),
            CelValue::from_err(rscel::CelError::runtime(&"x".repeat(cap + 1))),
            CelValue::List(vec![CelValue::Null; 65537]),
        ];
        for value in values {
            let program = Program::new(ProgramDetails::new(), vec![ByteCode::Push(value)].into());
            assert!(rscel::comparison::snapshot(&program).is_none());
            // Repeated refusal must leave the source compiled artifact usable.
            assert!(rscel::comparison::snapshot(&program).is_none());
            assert_eq!(program.bytecode().len(), 1);
        }
    }
    #[test]
    fn comparison_codec_requires_version_and_typed_constants() {
        let program = Program::from_source("timestamp('2026-11-11T12:13:14.123456789Z')").unwrap();
        let encoded = rscel::comparison::snapshot(&program).unwrap();
        assert!(
            rscel::comparison::restore(&encoded.replace("\"version\":1", "\"version\":2"))
                .is_none()
        );
        assert!(rscel::comparison::restore(r#"{"version":1,"details":{"source":null,"params":[]},"code":[{"Push":{"Scalar":{"Float":1.0}}}]}"#).is_none());
        assert!(rscel::comparison::restore(r#"{"version":1,"details":{"source":null,"params":[]},"code":[{"Other":{"Push":{"Bool":true}}}]}"#).is_none());
        assert!(rscel::comparison::restore(&"x".repeat(512 * 1024 + 1)).is_none());
    }
}

#[test]
fn flat_owned_ast_chains_are_refused_and_dropped_without_recursive_stack_growth() {
    let addition = format!("{}1", "1+".repeat(100_000));
    for source in [
        addition.clone(),
        format!("{}true", "true && ".repeat(40_000)),
        format!("{}{}{}", "(".repeat(6), addition, ")".repeat(6)),
    ] {
        let error = CelContext::compile_bounded(&source, 1_000_000, deadline()).unwrap_err();
        assert!(error.to_string().contains("CEL resource limit exceeded"));
    }
    let program =
        CelContext::compile_bounded(&format!("{}1", "1+".repeat(24)), 100_000, deadline()).unwrap();
    drop(program);
}

#[test]
fn documented_expression_and_ast_box_boundaries_compile_and_drop_successfully() {
    let nested = format!("{}true{}", "(".repeat(7), ")".repeat(7));
    drop(CelContext::compile_bounded(&nested, 100_000, deadline()).unwrap());
    // Each addition adds one lhs box; the complete expression adds one more.
    let at_limit = format!("{}1", "1+".repeat(127));
    drop(CelContext::compile_bounded(&at_limit, 100_000, deadline()).unwrap());
    let over_limit = format!("{}1", "1+".repeat(128));
    assert!(CelContext::compile_bounded(&over_limit, 100_000, deadline()).is_err());
}
