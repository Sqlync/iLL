// Runtime values. Mirrors `ValueType` 1:1 plus `Dict` (for `ok.*` /
// `error.*` / struct-shaped kwargs) and `Unit` (for values not yet available,
// e.g. `ok.exit` before process teardown).
//
// `Dict` is an `IndexMap` so field iteration follows insertion order. That
// matters for positional access like `ok.col[0]` on query results, where the
// nth entry means "the nth column as declared" — alphabetical ordering would
// surprise the reader.

use std::fmt;

use indexmap::IndexMap;

/// An ordered map of field name → value. Insertion order is preserved so that
/// integer indexing into a dict (`dict[0]`) means "the nth inserted field",
/// which is what examples like `ok.col[0]` rely on.
pub type Dict = IndexMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Number(i64),
    Bool(bool),
    Atom(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Dict(Dict),
    Unit,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::Atom(_) => "atom",
            Value::Bytes(_) => "bytes",
            Value::Array(_) => "array",
            Value::Dict(_) => "dict",
            Value::Unit => "unit",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) => write!(f, "{s:?}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Atom(a) => write!(f, ":{a}"),
            Value::Bytes(b) => write!(f, "<{} bytes>", b.len()),
            Value::Array(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Dict(fields) => {
                write!(f, "{{")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::Unit => write!(f, "()"),
        }
    }
}
