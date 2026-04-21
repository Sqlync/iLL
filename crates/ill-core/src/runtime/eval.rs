// Expression evaluator. Phase 5 supports what exec examples need: literals,
// idents resolved against a scope, member access on Dicts, plain string
// concatenation of fragments, and indexing into arrays and dicts (used by
// query-result assertions like `ok.row[0]`, `ok.col["name"]`, `ok.cell[i, j]`).
// Sigils dispatch through `runtime::sigil::Registry`. `let parse` is still
// deferred — it returns an Eval error so the test fails clearly if an example
// outgrows this subset.

use std::collections::BTreeMap;

use crate::ast::{Expr, StringLit};

use super::sigil::{concat_fragments, Registry as SigilRegistry};
use super::{RuntimeError, Value};

/// Name→Value scope. `ok`, `error`, `self`, and per-actor vars all live here
/// as regular bindings — callers set them up before calling `eval`.
pub struct Scope {
    bindings: BTreeMap<String, Value>,
}

impl Scope {
    pub fn new() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    pub fn bind(&mut self, name: impl Into<String>, value: Value) {
        self.bindings.insert(name.into(), value);
    }

    pub fn unbind(&mut self, name: &str) {
        self.bindings.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.bindings.get(name)
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

pub fn eval(expr: &Expr, scope: &Scope) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Atom(ident) => Ok(Value::Atom(ident.name.clone())),
        Expr::StringLit(lit) => eval_string_lit(lit, scope),
        Expr::Ident(ident) => scope
            .get(&ident.name)
            .cloned()
            .ok_or_else(|| RuntimeError::Eval(format!("undefined name `{}`", ident.name))),
        Expr::MemberAccess {
            object, property, ..
        } => {
            let obj = eval(object, scope)?;
            match obj {
                Value::Dict(fields) => fields.get(&property.name).cloned().ok_or_else(|| {
                    RuntimeError::Eval(format!("no field `{}` on dict", property.name))
                }),
                other => Err(RuntimeError::Eval(format!(
                    "cannot access `.{}` on {}",
                    property.name,
                    other.type_name()
                ))),
            }
        }
        Expr::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(eval(it, scope)?);
            }
            Ok(Value::Array(out))
        }
        Expr::Sigil(s) => {
            let Some(sigil) = SigilRegistry::global().get(&s.name.name) else {
                return Err(RuntimeError::Eval(format!(
                    "unknown sigil `~{}`",
                    s.name.name
                )));
            };
            let value = sigil.eval(&s.fragments, scope)?;
            if !sigil.output_type().accepts(&value) {
                return Err(RuntimeError::Eval(format!(
                    "sigil `~{}` declared {:?} but produced {}",
                    sigil.name(),
                    sigil.output_type(),
                    value.type_name()
                )));
            }
            Ok(value)
        }
        Expr::Index {
            object, indices, ..
        } => {
            let mut current = eval(object, scope)?;
            for idx_expr in indices {
                let idx = eval(idx_expr, scope)?;
                current = index_into(&current, &idx)?;
            }
            Ok(current)
        }
    }
}

/// Apply one level of indexing. Multi-arg indexing (`obj[i, j]`) is the same
/// as repeated single-arg indexing — the caller folds across the index list.
fn index_into(container: &Value, index: &Value) -> Result<Value, RuntimeError> {
    match (container, index) {
        (Value::Array(items), Value::Number(n)) => {
            let i = usize::try_from(*n).map_err(|_| {
                RuntimeError::Eval(format!("array index must be non-negative, got {n}"))
            })?;
            items.get(i).cloned().ok_or_else(|| {
                RuntimeError::Eval(format!(
                    "array index {i} out of bounds (length {})",
                    items.len()
                ))
            })
        }
        (Value::Dict(fields), Value::String(key)) => fields
            .get(key)
            .cloned()
            .ok_or_else(|| RuntimeError::Eval(format!("no field `{key}` on dict"))),
        (Value::Dict(fields), Value::Number(n)) => {
            let i = usize::try_from(*n).map_err(|_| {
                RuntimeError::Eval(format!("dict index must be non-negative, got {n}"))
            })?;
            fields.get_index(i).map(|(_, v)| v.clone()).ok_or_else(|| {
                RuntimeError::Eval(format!(
                    "dict index {i} out of bounds (length {})",
                    fields.len()
                ))
            })
        }
        (container, index) => Err(RuntimeError::Eval(format!(
            "cannot index {} with {}",
            container.type_name(),
            index.type_name()
        ))),
    }
}

fn eval_string_lit(lit: &StringLit, scope: &Scope) -> Result<Value, RuntimeError> {
    concat_fragments(&lit.fragments, scope).map(Value::String)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, Span};
    use crate::runtime::Dict;

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.into(),
            span: span(),
        }
    }

    #[test]
    fn literals() {
        let s = Scope::new();
        assert_eq!(eval(&Expr::Number(7), &s).unwrap(), Value::Number(7));
        assert_eq!(eval(&Expr::Bool(true), &s).unwrap(), Value::Bool(true));
        assert_eq!(
            eval(&Expr::Atom(ident("on")), &s).unwrap(),
            Value::Atom("on".into())
        );
    }

    #[test]
    fn ident_lookup() {
        let mut s = Scope::new();
        s.bind("x", Value::Number(42));
        assert_eq!(
            eval(&Expr::Ident(ident("x")), &s).unwrap(),
            Value::Number(42)
        );
    }

    #[test]
    fn member_access_on_dict() {
        let mut fields = Dict::new();
        fields.insert("pid".into(), Value::Number(12345));
        let mut s = Scope::new();
        s.bind("ok", Value::Dict(fields));

        let expr = Expr::MemberAccess {
            object: Box::new(Expr::Ident(ident("ok"))),
            property: ident("pid"),
            span: span(),
        };
        assert_eq!(eval(&expr, &s).unwrap(), Value::Number(12345));
    }

    #[test]
    fn undefined_name_errors() {
        let s = Scope::new();
        assert!(eval(&Expr::Ident(ident("nope")), &s).is_err());
    }

    fn index_expr(obj: Expr, indices: Vec<Expr>) -> Expr {
        Expr::Index {
            object: Box::new(obj),
            indices,
            span: span(),
        }
    }

    #[test]
    fn array_int_index() {
        let mut s = Scope::new();
        s.bind(
            "xs",
            Value::Array(vec![
                Value::Number(10),
                Value::Number(20),
                Value::Number(30),
            ]),
        );
        let e = index_expr(Expr::Ident(ident("xs")), vec![Expr::Number(1)]);
        assert_eq!(eval(&e, &s).unwrap(), Value::Number(20));
    }

    #[test]
    fn array_out_of_bounds_errors() {
        let mut s = Scope::new();
        s.bind("xs", Value::Array(vec![Value::Number(1)]));
        let e = index_expr(Expr::Ident(ident("xs")), vec![Expr::Number(5)]);
        assert!(eval(&e, &s).is_err());
    }

    #[test]
    fn dict_string_key() {
        let mut fields = Dict::new();
        fields.insert("name".into(), Value::String("alice".into()));
        let mut s = Scope::new();
        s.bind("r", Value::Dict(fields));

        let e = index_expr(
            Expr::Ident(ident("r")),
            vec![Expr::StringLit(crate::ast::StringLit {
                fragments: vec![crate::ast::StringFragment::Text("name".into())],
                span: span(),
            })],
        );
        assert_eq!(eval(&e, &s).unwrap(), Value::String("alice".into()));
    }

    #[test]
    fn dict_int_index_follows_insertion_order() {
        let mut fields = Dict::new();
        fields.insert("zebra".into(), Value::Number(1));
        fields.insert("apple".into(), Value::Number(2));
        let mut s = Scope::new();
        s.bind("r", Value::Dict(fields));

        let e = index_expr(Expr::Ident(ident("r")), vec![Expr::Number(0)]);
        assert_eq!(
            eval(&e, &s).unwrap(),
            Value::Number(1),
            "first inserted entry wins over alphabetical"
        );
    }

    #[test]
    fn multi_arg_indexing_is_sequential() {
        // cell[0, 1] on [[10, 20], [30, 40]] → 20
        let mut s = Scope::new();
        s.bind(
            "cell",
            Value::Array(vec![
                Value::Array(vec![Value::Number(10), Value::Number(20)]),
                Value::Array(vec![Value::Number(30), Value::Number(40)]),
            ]),
        );
        let e = index_expr(
            Expr::Ident(ident("cell")),
            vec![Expr::Number(0), Expr::Number(1)],
        );
        assert_eq!(eval(&e, &s).unwrap(), Value::Number(20));
    }

    #[test]
    fn indexing_wrong_type_errors() {
        let mut s = Scope::new();
        s.bind("n", Value::Number(42));
        let e = index_expr(Expr::Ident(ident("n")), vec![Expr::Number(0)]);
        assert!(eval(&e, &s).is_err());
    }
}
