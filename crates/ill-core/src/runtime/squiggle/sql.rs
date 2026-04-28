use crate::actor_type::ValueType;

use super::Squiggle;

/// `~sql` — a SQL string. For now it's a plain string with tagged syntax;
/// parameterization is deferred until pg_client needs it.
pub struct Sql;

impl Squiggle for Sql {
    fn name(&self) -> &'static str {
        "sql"
    }

    fn output_type(&self) -> ValueType {
        ValueType::String
    }
}

#[cfg(test)]
mod tests {
    use super::super::Registry;
    use super::*;
    use crate::ast::{Expr, Ident, StringFragment};
    use crate::runtime::eval::Scope;
    use crate::runtime::Value;
    use crate::test_util::dummy_span;

    #[test]
    fn registered() {
        assert!(Registry::global().get("sql").is_some());
    }

    #[test]
    fn plain_text() {
        let scope = Scope::new();
        let frags = vec![StringFragment::Text("SELECT 1".into())];
        assert_eq!(
            Sql.eval(&frags, &scope).unwrap(),
            Value::String("SELECT 1".into())
        );
    }

    #[test]
    fn interpolation() {
        let mut scope = Scope::new();
        scope.bind("n", Value::Number(42));
        let frags = vec![
            StringFragment::Text("x = ".into()),
            StringFragment::Interpolation(Expr::Ident(Ident {
                name: "n".into(),
                span: dummy_span(),
            })),
        ];
        assert_eq!(
            Sql.eval(&frags, &scope).unwrap(),
            Value::String("x = 42".into())
        );
    }
}
