use crate::actor_type::ValueType;

use super::Sigil;

/// `~re` — a regex pattern. Backtick-delimited content is raw, so users can
/// write `\.`, `\d`, `\s` directly without double-escaping. The value is a
/// plain `String`; `assert ... matches` compiles it via the `regex` crate.
pub struct Re;

impl Sigil for Re {
    fn name(&self) -> &'static str {
        "re"
    }

    fn output_type(&self) -> ValueType {
        ValueType::String
    }
}

#[cfg(test)]
mod tests {
    use super::super::Registry;
    use super::*;
    use crate::ast::StringFragment;
    use crate::runtime::eval::Scope;
    use crate::runtime::Value;

    #[test]
    fn registered() {
        assert!(Registry::global().get("re").is_some());
    }

    #[test]
    fn raw_backslash_preserved() {
        let scope = Scope::new();
        let frags = vec![StringFragment::Text(r"^charlie@.+\.org$".into())];
        assert_eq!(
            Re.eval(&frags, &scope).unwrap(),
            Value::String(r"^charlie@.+\.org$".into())
        );
    }
}
