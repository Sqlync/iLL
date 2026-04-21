// Sigils. `~name`backtick-fragments-backtick is a tagged string literal. At
// runtime the fragments (text + interpolations) are handed to a `Sigil` impl,
// which decides what `Value` the expression produces. Most sigils are "just
// strings with a tag for syntax highlighting and validation" — those get the
// default `eval` for free. A sigil like `~hex` can override `eval` to return
// `Value::Bytes` instead.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ast::StringFragment;

use super::eval::{eval, Scope};
use super::{RuntimeError, Value};

pub trait Sigil: Send + Sync {
    fn name(&self) -> &'static str;

    /// Produce the runtime `Value` for this sigil. Default: concatenate all
    /// fragments (with interpolations rendered) into a `Value::String`.
    fn eval(&self, fragments: &[StringFragment], scope: &Scope) -> Result<Value, RuntimeError> {
        concat_fragments(fragments, scope).map(Value::String)
    }
}

/// Render string fragments — literal text interleaved with `${expr}` holes —
/// into a single `String`. Shared between plain string literals and the
/// default sigil eval.
pub fn concat_fragments(
    fragments: &[StringFragment],
    scope: &Scope,
) -> Result<String, RuntimeError> {
    let mut out = String::new();
    for frag in fragments {
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
    Ok(out)
}

pub struct Registry {
    sigils: HashMap<&'static str, &'static dyn Sigil>,
}

impl Registry {
    pub fn global() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(Registry::build)
    }

    fn build() -> Registry {
        let mut r = Registry {
            sigils: HashMap::new(),
        };
        r.register(&Sql);
        r.register(&Json);
        r.register(&Hex);
        r
    }

    fn register(&mut self, s: &'static dyn Sigil) {
        let prev = self.sigils.insert(s.name(), s);
        assert!(prev.is_none(), "duplicate sigil: {}", s.name());
    }

    pub fn get(&self, name: &str) -> Option<&'static dyn Sigil> {
        self.sigils.get(name).copied()
    }
}

// ── Sigils ────────────────────────────────────────────────────────────────────

/// `~sql` — a SQL string. For now it's a plain string with tagged syntax;
/// parameterization is deferred until pg_client needs it.
pub struct Sql;

impl Sigil for Sql {
    fn name(&self) -> &'static str {
        "sql"
    }
}

/// `~json` — stub. Evaluates as the rendered string for now. When the http
/// actor actually consumes JSON bodies this should parse + re-emit canonical
/// form, or produce a structured `Value::Dict`.
pub struct Json;

impl Sigil for Json {
    fn name(&self) -> &'static str {
        "json"
    }
}

/// `~hex` — stub. Evaluates as the rendered string for now. When a consumer
/// (e.g. mqtt) actually needs binary payloads this should override `eval` to
/// hex-decode into `Value::Bytes`.
pub struct Hex;

impl Sigil for Hex {
    fn name(&self) -> &'static str {
        "hex"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Ident, Span};

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    #[test]
    fn registry_knows_sql() {
        assert!(Registry::global().get("sql").is_some());
        assert!(Registry::global().get("does_not_exist").is_none());
    }

    #[test]
    fn sql_eval_plain_text() {
        let scope = Scope::new();
        let frags = vec![StringFragment::Text("SELECT 1".into())];
        let v = Sql.eval(&frags, &scope).unwrap();
        assert_eq!(v, Value::String("SELECT 1".into()));
    }

    #[test]
    fn sql_eval_with_interpolation() {
        let mut scope = Scope::new();
        scope.bind("n", Value::Number(42));
        let frags = vec![
            StringFragment::Text("x = ".into()),
            StringFragment::Interpolation(Expr::Ident(Ident {
                name: "n".into(),
                span: span(),
            })),
        ];
        let v = Sql.eval(&frags, &scope).unwrap();
        assert_eq!(v, Value::String("x = 42".into()));
    }

    #[test]
    fn concat_fragments_rejects_unrenderable_values() {
        let mut scope = Scope::new();
        scope.bind("xs", Value::Array(vec![Value::Number(1)]));
        let frags = vec![StringFragment::Interpolation(Expr::Ident(Ident {
            name: "xs".into(),
            span: span(),
        }))];
        assert!(concat_fragments(&frags, &scope).is_err());
    }
}
