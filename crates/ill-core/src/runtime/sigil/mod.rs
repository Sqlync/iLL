// Sigils. `~name`backtick-fragments-backtick is a tagged string literal. At
// runtime the fragments (text + interpolations) are handed to a `Sigil` impl,
// which decides what `Value` the expression produces. Most sigils are "just
// strings with a tag for syntax highlighting and validation" — those get the
// default `eval` for free. A sigil like `~hex` can override `eval` to return
// `Value::Bytes` instead.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::actor_type::ValueType;
use crate::ast::StringFragment;

use super::eval::{eval, Scope};
use super::{RuntimeError, Value};

mod hex;
mod json;
mod re;
mod sql;

use hex::Hex;
use json::Json;
use re::Re;
use sql::Sql;

pub trait Sigil: Send + Sync {
    fn name(&self) -> &'static str;

    /// Static declaration of the `Value` shape this sigil produces. The
    /// validator uses this for type checking; the runtime asserts the
    /// declaration holds after each `eval`.
    fn output_type(&self) -> ValueType;

    /// Produce the runtime `Value` for this sigil. Default: concatenate all
    /// fragments (with interpolations rendered) into a `Value::String`. Sigils
    /// that declare a non-`String` `output_type()` must override this.
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
        r.register(&Re);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Ident, Span};

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    #[test]
    fn unknown_sigil_lookup_is_none() {
        assert!(Registry::global().get("does_not_exist").is_none());
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

    #[test]
    fn output_type_mismatch_is_caught_at_eval() {
        // A sigil that declares Bytes but returns a String (the default).
        // The Expr::Sigil arm runs `accepts` and rejects the mismatch.
        struct Liar;
        impl Sigil for Liar {
            fn name(&self) -> &'static str {
                "liar"
            }
            fn output_type(&self) -> ValueType {
                ValueType::Bytes
            }
        }

        let frags = vec![StringFragment::Text("not bytes".into())];
        let scope = Scope::new();
        let v = Liar.eval(&frags, &scope).unwrap();
        assert!(!v.is_of_type(Liar.output_type()));
    }
}
