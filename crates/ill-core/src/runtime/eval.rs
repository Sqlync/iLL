// Expression evaluator. Phase 5 supports what exec examples need: literals,
// idents resolved against a scope, member access on Records, and plain string
// concatenation of fragments. Sigils, regex, interpolation, array indexing,
// and `let parse` are deferred — they return an Eval error so the test fails
// clearly if an example outgrows this subset.

use std::collections::BTreeMap;

use crate::ast::{Expr, StringFragment, StringLit};

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
                Value::Record(fields) => fields.get(&property.name).cloned().ok_or_else(|| {
                    RuntimeError::Eval(format!("no field `{}` on record", property.name))
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
        Expr::Sigil(s) => Err(RuntimeError::Eval(format!(
            "sigil `~{}` not yet supported in runtime",
            s.name.name
        ))),
        Expr::Index { .. } => Err(RuntimeError::Eval(
            "indexing not yet supported in runtime".into(),
        )),
    }
}

fn eval_string_lit(lit: &StringLit, scope: &Scope) -> Result<Value, RuntimeError> {
    let mut out = String::new();
    for frag in &lit.fragments {
        match frag {
            StringFragment::Text(t) => out.push_str(t),
            StringFragment::Interpolation(expr) => {
                let v = eval(expr, scope)?;
                match v {
                    Value::String(s) => out.push_str(&s),
                    Value::Number(n) => out.push_str(&n.to_string()),
                    Value::Bool(b) => out.push_str(&b.to_string()),
                    Value::Atom(a) => out.push_str(&a),
                    other => {
                        return Err(RuntimeError::Eval(format!(
                            "cannot interpolate {} into string",
                            other.type_name()
                        )))
                    }
                }
            }
        }
    }
    Ok(Value::String(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, Span};

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
    fn member_access_on_record() {
        let mut fields = BTreeMap::new();
        fields.insert("pid".into(), Value::Number(12345));
        let mut s = Scope::new();
        s.bind("ok", Value::Record(fields));

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
}
