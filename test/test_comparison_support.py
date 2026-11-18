"""Private observations must preserve normal Python results and cached programs."""

import json
from datetime import UTC, datetime

import pytest

import rscel


def test_observed_eval_retains_normal_result_and_reports_actual_program():
    observations = []
    result = rscel._eval_observed(
        "x + 1",
        {"x": 2},
        lambda program, failed: observations.append((program, failed)),
    )
    assert result == rscel.eval("x + 1", {"x": 2}) == 3
    assert len(observations) == 1
    program, compile_failed = observations[0]
    assert json.loads(program)["version"] == 1
    assert compile_failed is False


@pytest.mark.parametrize("source", ["3 +", "missing + 1"])
def test_observed_eval_preserves_compile_and_runtime_errors(source):
    observations = []
    with pytest.raises(Exception) as original:
        rscel.eval(source, {})
    with pytest.raises(type(original.value)) as observed:
        rscel._eval_observed(
            source, {}, lambda program, failed: observations.append((program, failed))
        )
    assert str(observed.value) == str(original.value)
    assert len(observations) == 1
    program, compile_failed = observations[0]
    if source == "3 +":
        assert compile_failed is True
        assert program is None
    else:
        assert compile_failed is False
        assert json.loads(program)["version"] == 1


def test_comparison_snapshot_does_not_replace_or_mutate_public_program_formats():
    program = rscel.CelProgram()
    program.add_source("timestamp('2026-11-11T12:13:14.123456789Z')")
    public_json = program.serialize_to_json()
    snapshot = json.loads(program._comparison_json())
    assert snapshot["code"][0]["Push"]["Timestamp"][1] == 123456789
    assert program.serialize_to_json() == public_json
    for encoded, method in [
        (public_json, "add_serialized_json"),
    ]:
        restored = rscel.CelProgram()
        getattr(restored, method)(encoded)
        context = rscel.CelContext()
        context.add_program("legacy", restored)
        assert context.exec("legacy", rscel.BindContext()).year == 2026


def test_cached_clock_constant_keeps_the_original_value_without_replay():
    context = rscel.CelContext()
    context.add_program_str("cached", "timestamp('2026-11-11T12:13:14.123456789Z')")
    first = context.exec("cached", rscel.BindContext())
    assert context.exec("cached", rscel.BindContext()) == first


def test_observed_runtime_clock_keeps_normal_python_result():
    observations = []
    observed_result = rscel._eval_observed(
        "now() != null",
        {"x": 1},
        lambda program, failed: observations.append((program, failed)),
    )
    assert observed_result is True
    assert len(observations) == 1
    assert json.loads(observations[0][0])["version"] == 1
    assert observations[0][1] is False
    assert rscel.eval("now() != null", {"x": 1}) is True


def test_observed_eval_preserves_explicit_runtime_and_typed_macro_binders():
    runtime = rscel.InterpRuntime(datetime(2020, 1, 1, tzinfo=UTC))
    observations = []
    source = "items.map(x, now())"
    inputs = {"items": [1, 2]}
    actual = rscel._eval_observed(
        source,
        inputs,
        lambda program, failed: observations.append((program, failed)),
        runtime=runtime,
    )
    assert (
        actual
        == rscel.eval(source, inputs, runtime=runtime)
        == [runtime.start_time] * 2
    )
    assert len(observations) == 1
    assert observations[0][0] is not None
    assert observations[0][1] is False
    assert '"Binder":"x"' in observations[0][0]


def test_observer_callback_errors_propagate_without_poisoning_later_evaluation():
    def failed_observer(_program, _compile_failed):
        raise RuntimeError("observer failure")

    with pytest.raises(RuntimeError, match="observer failure"):
        rscel._eval_observed("1", {}, failed_observer)
    assert rscel.eval("1 + 1", {}) == 2


def test_unavailable_snapshot_does_not_claim_a_compile_failure_or_change_result():
    observations = []
    source = repr("x" * 600000)
    result = rscel._eval_observed(
        source, {}, lambda program, failed: observations.append((program, failed))
    )
    assert len(result) == 600000
    assert observations == [(None, False)]
