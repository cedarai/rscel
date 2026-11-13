"""Private observations must preserve normal Python results and cached programs."""

import json

import pytest
import rscel


def test_observed_eval_retains_normal_result_and_reports_actual_program():
    observations = []
    result = rscel._eval_observed(
        "x + 1",
        {"x": 2},
        lambda program, clocks: observations.append((program, clocks)),
    )
    assert result == rscel.eval("x + 1", {"x": 2}) == 3
    assert len(observations) == 1
    program, clocks = observations[0]
    assert json.loads(program)["version"] == 1
    assert clocks == []


@pytest.mark.parametrize("source", ["3 +", "missing + 1"])
def test_observed_eval_preserves_compile_and_runtime_errors(source):
    observations = []
    with pytest.raises(Exception) as original:
        rscel.eval(source, {})
    with pytest.raises(type(original.value)) as observed:
        rscel._eval_observed(
            source, {}, lambda program, clocks: observations.append((program, clocks))
        )
    assert str(observed.value) == str(original.value)
    assert len(observations) == 1
    program, clocks = observations[0]
    assert clocks == []
    if source == "3 +":
        assert program is None
    else:
        assert json.loads(program)["version"] == 1


def test_comparison_snapshot_does_not_replace_or_mutate_public_program_formats():
    program = rscel.CelProgram()
    program.add_source("timestamp('2026-11-11T12:13:14.123456789Z')")
    public_json = program.serialize_to_json()
    public_binary = program.serialize_to_bincode()
    snapshot = json.loads(program._comparison_json())
    assert snapshot["code"][0]["Push"]["Timestamp"][1] == 123456789
    assert program.serialize_to_json() == public_json
    assert program.serialize_to_bincode() == public_binary
    for encoded, method in [
        (public_json, "add_serialized_json"),
        (public_binary, "add_serialized_bincode"),
    ]:
        restored = rscel.CelProgram()
        getattr(restored, method)(encoded)
        context = rscel.CelContext()
        context.add_program("legacy", restored)
        assert context.exec("legacy", rscel.BindContext()).year == 2026


def test_manual_observation_records_compilation_clock_and_preserves_cached_value():
    context = rscel.CelContext()
    scope = rscel._ClockObservation()
    try:
        context.add_program_str("cached", "now()")
        actual = context.exec("cached", rscel.BindContext())
    finally:
        clocks = scope.finish()
    assert len(clocks) == 1
    assert clocks[0][0] == int(actual.timestamp())
    assert clocks[0][1] // 1000 == actual.microsecond
    assert scope.finish() is None
    later = rscel._ClockObservation()
    try:
        assert context.exec("cached", rscel.BindContext()) == actual
    finally:
        assert later.finish() == []


def test_observer_callback_errors_propagate_without_poisoning_later_evaluation():
    def failed_observer(_program, _clocks):
        raise RuntimeError("observer failure")

    with pytest.raises(RuntimeError, match="observer failure"):
        rscel._eval_observed("1", {}, failed_observer)
    assert rscel.eval("1 + 1", {}) == 2


def test_out_of_order_observer_finish_cannot_borrow_or_resurrect_other_evidence():
    outer = rscel._ClockObservation()
    inner = rscel._ClockObservation()
    assert outer.finish() is None
    assert inner.finish() is None
    fresh = rscel._ClockObservation()
    try:
        actual = rscel.eval("now()", {})
    finally:
        clocks = fresh.finish()
    assert len(clocks) == 1
    assert clocks[0][0] == int(actual.timestamp())
