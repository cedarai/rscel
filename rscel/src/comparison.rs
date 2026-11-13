//! Private, versioned comparison snapshot. The public legacy program wire keeps
//! its historical millisecond representation; this wire preserves every bit of
//! float constants and nanoseconds of temporal constants, including nested code.
use crate::types::CelByteCode;
use crate::{ByteCode, CelValue, Program, ProgramDetails};
use chrono::{DateTime, Duration};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Write},
};
const BYTES: usize = 512 * 1024;
const NODES: usize = 65536;
const DEPTH: usize = 64;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    details: ProgramDetails,
    code: Vec<Instruction>,
}
#[derive(Serialize)]
struct SnapshotRef<'a> {
    version: u32,
    details: &'a ProgramDetails,
    code: Vec<InstructionRef<'a>>,
}
#[derive(Serialize)]
enum InstructionRef<'a> {
    Push(ValueRef<'a>),
    Other(&'a ByteCode),
}
#[derive(Serialize)]
enum ValueRef<'a> {
    Scalar(&'a CelValue),
    Float(u64),
    Timestamp(i64, u32),
    Duration(i64, i32),
    List(Vec<ValueRef<'a>>),
    Map(BTreeMap<&'a str, ValueRef<'a>>),
    Code(Vec<InstructionRef<'a>>),
}
#[derive(Serialize, Deserialize)]
enum Instruction {
    Push(Value),
    Other(ByteCode),
}
#[derive(Serialize, Deserialize)]
enum Value {
    Scalar(CelValue),
    Float(u64),
    Timestamp(i64, u32),
    Duration(i64, i32),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Code(Vec<Instruction>),
}
struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn new() -> Self {
        Self { nodes: 0, bytes: 0 }
    }
    fn visit(&mut self, depth: usize) -> Option<()> {
        if depth > DEPTH || self.nodes >= NODES {
            return None;
        }
        self.nodes += 1;
        self.charge(std::mem::size_of::<ValueRef<'_>>())
    }
    fn children(&self, len: usize) -> Option<()> {
        if len > NODES - self.nodes
            || len > (BYTES - self.bytes) / std::mem::size_of::<ValueRef<'_>>()
        {
            None
        } else {
            Some(())
        }
    }
    fn charge(&mut self, bytes: usize) -> Option<()> {
        if bytes > BYTES - self.bytes {
            return None;
        }
        self.bytes += bytes;
        Some(())
    }
}
fn scalar_bytes(value: &CelValue) -> Option<usize> {
    Some(match value {
        CelValue::String(s) | CelValue::Ident(s) | CelValue::Type(s) => s.len(),
        CelValue::Bytes(v) => v.len(),
        CelValue::Err(e) => e.owned_text_bytes()?,
        _ => 0,
    })
}

struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > BYTES - self.0.len() {
            return Err(io::Error::other("comparison program limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn scalar(v: &CelValue) -> bool {
    matches!(
        v,
        CelValue::Int(_)
            | CelValue::UInt(_)
            | CelValue::Bool(_)
            | CelValue::String(_)
            | CelValue::Bytes(_)
            | CelValue::Null
            | CelValue::Ident(_)
            | CelValue::Type(_)
            | CelValue::Err(_)
    )
}
fn encode_code<'a>(
    code: &'a CelByteCode,
    budget: &mut Budget,
    depth: usize,
) -> Option<Vec<InstructionRef<'a>>> {
    budget.children(code.len())?;
    code.iter()
        .map(|op| {
            budget.visit(depth)?;
            Some(match op {
                ByteCode::Push(v) => InstructionRef::Push(encode_value(v, budget, depth + 1)?),
                other => InstructionRef::Other(other),
            })
        })
        .collect()
}
fn encode_value<'a>(v: &'a CelValue, budget: &mut Budget, depth: usize) -> Option<ValueRef<'a>> {
    budget.visit(depth)?;
    Some(match v {
        CelValue::Float(v) => ValueRef::Float(v.to_bits()),
        CelValue::TimeStamp(v) => ValueRef::Timestamp(v.timestamp(), v.timestamp_subsec_nanos()),
        CelValue::Duration(v) => ValueRef::Duration(v.num_seconds(), v.subsec_nanos()),
        CelValue::List(v) => {
            budget.children(v.len())?;
            ValueRef::List(
                v.iter()
                    .map(|v| encode_value(v, budget, depth + 1))
                    .collect::<Option<_>>()?,
            )
        }
        CelValue::Map(v) => {
            budget.children(v.len())?;
            ValueRef::Map(
                v.iter()
                    .map(|(k, v)| {
                        budget.charge(k.len())?;
                        Some((k.as_str(), encode_value(v, budget, depth + 1)?))
                    })
                    .collect::<Option<_>>()?,
            )
        }
        CelValue::ByteCode(v) => ValueRef::Code(encode_code(v, budget, depth + 1)?),
        v if scalar(v) => {
            budget.charge(scalar_bytes(v)?)?;
            ValueRef::Scalar(v)
        }
        _ => return None,
    })
}

fn decode_code(code: Vec<Instruction>, budget: &mut Budget, depth: usize) -> Option<CelByteCode> {
    code.into_iter()
        .map(|op| {
            budget.visit(depth)?;
            Some(match op {
                Instruction::Push(value) => ByteCode::Push(decode_value(value, budget, depth + 1)?),
                Instruction::Other(ByteCode::Push(_)) => return None,
                Instruction::Other(other) => other,
            })
        })
        .collect()
}
fn decode_value(v: Value, budget: &mut Budget, depth: usize) -> Option<CelValue> {
    budget.visit(depth)?;
    Some(match v {
        Value::Float(bits) => CelValue::Float(f64::from_bits(bits)),
        Value::Timestamp(seconds, nanos) => {
            CelValue::TimeStamp(DateTime::from_timestamp(seconds, nanos)?)
        }
        Value::Duration(seconds, nanos) => {
            if !(-999999999..=999999999).contains(&nanos)
                || seconds.signum() * i64::from(nanos.signum()) < 0
            {
                return None;
            }
            CelValue::Duration(
                Duration::try_seconds(seconds)?
                    .checked_add(&Duration::nanoseconds(i64::from(nanos)))?,
            )
        }
        Value::List(v) => CelValue::List(
            v.into_iter()
                .map(|v| decode_value(v, budget, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Value::Map(v) => CelValue::Map(
            v.into_iter()
                .map(|(k, v)| Some((k, decode_value(v, budget, depth + 1)?)))
                .collect::<Option<_>>()?,
        ),
        Value::Code(v) => CelValue::ByteCode(decode_code(v, budget, depth + 1)?),
        Value::Scalar(v) if scalar(&v) => v,
        _ => return None,
    })
}
pub fn snapshot(program: &Program) -> Option<String> {
    // Metadata is borrowed too. Admit it before serialization scans source text.
    let mut budget = Budget::new();
    budget.charge(program.source().map_or(0, str::len))?;
    for name in program.details().comparison_param_names() {
        budget.visit(0)?;
        budget.charge(name.len())?;
    }

    let value = SnapshotRef {
        version: 1,
        details: program.details(),
        code: encode_code(program.bytecode(), &mut budget, 0)?,
    };
    let mut out = Bounded(Vec::new());
    serde_json::to_writer(&mut out, &value).ok()?;
    String::from_utf8(out.0).ok()
}
pub fn restore(encoded: &str) -> Option<Program> {
    if encoded.len() > BYTES {
        return None;
    }
    let value: Snapshot = serde_json::from_str(encoded).ok()?;
    if value.version != 1 {
        return None;
    }
    Some(Program::new(
        value.details,
        decode_code(value.code, &mut Budget::new(), 0)?,
    ))
}
