use crate::actor_type::ValueType;
use crate::ast::StringFragment;
use crate::runtime::eval::Scope;
use crate::runtime::{RuntimeError, Value};

use super::{concat_fragments, Squiggle};

/// `~b` — UTF-8-encoded bytes. The complement of `~hex`: where `~hex`
/// decodes a hex literal into raw bytes, `~b` takes ordinary text content
/// (with interpolations) and produces its UTF-8 byte sequence. Useful for
/// MQTT payloads, HTTP bodies, and any other context where a Bytes value is
/// wanted but the contents are naturally written as text.
pub struct Bytes;

impl Squiggle for Bytes {
    fn name(&self) -> &'static str {
        "b"
    }

    fn output_type(&self) -> ValueType {
        ValueType::Bytes
    }

    fn eval(&self, fragments: &[StringFragment], scope: &Scope) -> Result<Value, RuntimeError> {
        let rendered = concat_fragments(fragments, scope)?;
        Ok(Value::Bytes(rendered.into_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Registry;
    use super::*;
    use crate::ast::{Expr, Ident};
    use crate::test_util::dummy_span;

    #[test]
    fn registered() {
        assert!(Registry::global().get("b").is_some());
    }

    #[test]
    fn output_type_is_bytes() {
        assert_eq!(Bytes.output_type(), ValueType::Bytes);
    }

    #[test]
    fn ascii_text_round_trips() {
        let frags = vec![StringFragment::Text("hello".into())];
        let v = Bytes.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(b"hello".to_vec()));
    }

    #[test]
    fn unicode_is_utf8_encoded() {
        let frags = vec![StringFragment::Text("héllo".into())];
        let v = Bytes.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes("héllo".as_bytes().to_vec()));
    }

    #[test]
    fn empty_decodes_to_empty_bytes() {
        let frags = vec![StringFragment::Text("".into())];
        let v = Bytes.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(vec![]));
    }

    #[test]
    fn string_interpolation_works() {
        let mut scope = Scope::new();
        scope.bind("name", Value::String("world".into()));
        let frags = vec![
            StringFragment::Text("hello, ".into()),
            StringFragment::Interpolation(Expr::Ident(Ident {
                name: "name".into(),
                span: dummy_span(),
            })),
        ];
        let v = Bytes.eval(&frags, &scope).unwrap();
        assert_eq!(v, Value::Bytes(b"hello, world".to_vec()));
    }

    #[test]
    fn bytes_interpolation_round_trips_when_utf8() {
        // Captured payload from a text publish is `Value::Bytes`; interpolating
        // it back into a `~b` template should re-render the original text.
        let mut scope = Scope::new();
        scope.bind("captured", Value::Bytes(b"world".to_vec()));
        let frags = vec![
            StringFragment::Text("got: ".into()),
            StringFragment::Interpolation(Expr::Ident(Ident {
                name: "captured".into(),
                span: dummy_span(),
            })),
        ];
        let v = Bytes.eval(&frags, &scope).unwrap();
        assert_eq!(v, Value::Bytes(b"got: world".to_vec()));
    }

    #[test]
    fn non_utf8_bytes_interpolation_errors() {
        let mut scope = Scope::new();
        // 0xFF is invalid UTF-8 on its own.
        scope.bind("binary", Value::Bytes(vec![0xFF, 0xFE]));
        let frags = vec![StringFragment::Interpolation(Expr::Ident(Ident {
            name: "binary".into(),
            span: dummy_span(),
        }))];
        assert!(Bytes.eval(&frags, &scope).is_err());
    }
}
