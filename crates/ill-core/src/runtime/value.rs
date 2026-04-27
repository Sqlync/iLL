// Runtime values. Mirrors `ValueType` 1:1 plus `Dict` (for `ok.*` /
// `error.*` / struct-shaped kwargs) and `Null` (for SQL NULLs and other
// "absent" values surfaced from external systems).
//
// Absence is *not* a `Value`. "This member var was never assigned" or
// "this outcome field hasn't been produced yet" is represented by the
// containing dict simply not having the key — lookups fail with a clear
// error rather than returning a sentinel value.
//
// `Dict` is an `IndexMap` so field iteration follows insertion order. That
// matters for positional access like `ok.col[0]` on query results, where the
// nth entry means "the nth column as declared" — alphabetical ordering would
// surprise the reader.

use std::fmt;

use indexmap::IndexMap;

use crate::actor_type::ValueType;

/// An ordered map of field name → value. Insertion order is preserved so that
/// integer indexing into a dict (`dict[0]`) means "the nth inserted field",
/// which is what examples like `ok.col[0]` rely on.
pub type Dict = IndexMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Number(i64),
    Float(f64),
    Bool(bool),
    Atom(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Dict(Dict),
    /// SQL NULL / absent value surfaced from external systems.
    Null,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Atom(_) => "atom",
            Value::Bytes(_) => "bytes",
            Value::Array(_) => "array",
            Value::Dict(_) => "dict",
            Value::Null => "null",
        }
    }

    /// True if this value is a valid inhabitant of `ty`. `Dynamic` and
    /// `Unknown` are permissive (they match any runtime value); every other
    /// variant is strict. `Array`/`Null` match no concrete `ValueType` — use
    /// `Dynamic` to accept them.
    pub fn is_of_type(&self, ty: ValueType) -> bool {
        matches!(
            (ty, self),
            (ValueType::Dynamic | ValueType::Unknown, _)
                | (ValueType::String, Value::String(_))
                | (ValueType::Number, Value::Number(_))
                | (ValueType::Float, Value::Float(_))
                | (ValueType::Bool, Value::Bool(_))
                | (ValueType::Atom, Value::Atom(_))
                | (ValueType::Bytes, Value::Bytes(_))
                | (ValueType::Dict, Value::Dict(_))
        )
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) => write!(f, "{s:?}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{x}"),
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
            Value::Null => write!(f, "null"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_of_type_dict_matches_value_dict() {
        let mut d = Dict::new();
        d.insert("k".into(), Value::String("v".into()));
        assert!(Value::Dict(d).is_of_type(ValueType::Dict));
    }

    #[test]
    fn is_of_type_dict_does_not_match_other_values() {
        assert!(!Value::String("x".into()).is_of_type(ValueType::Dict));
        assert!(!Value::Number(1).is_of_type(ValueType::Dict));
        assert!(!Value::Bytes(vec![0]).is_of_type(ValueType::Dict));
        assert!(!Value::Null.is_of_type(ValueType::Dict));
    }
}
