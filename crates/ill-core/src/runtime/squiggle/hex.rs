use crate::actor_type::ValueType;
use crate::ast::StringFragment;
use crate::runtime::eval::Scope;
use crate::runtime::{RuntimeError, Value};

use super::{concat_fragments, Squiggle};

/// `~hex` — backtick contents are hex (0-9a-fA-F, optional ASCII whitespace
/// between bytes); evaluates to `Value::Bytes`. Interpolations render as
/// strings first and must themselves be valid hex.
pub struct Hex;

impl Squiggle for Hex {
    fn name(&self) -> &'static str {
        "hex"
    }

    fn output_type(&self) -> ValueType {
        ValueType::Bytes
    }

    fn eval(&self, fragments: &[StringFragment], scope: &Scope) -> Result<Value, RuntimeError> {
        let rendered = concat_fragments(fragments, scope)?;
        decode_hex(&rendered).map(Value::Bytes)
    }
}

fn decode_hex(s: &str) -> Result<Vec<u8>, RuntimeError> {
    let mut nibbles: Vec<u8> = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        let n = match ch {
            '0'..='9' => ch as u8 - b'0',
            'a'..='f' => ch as u8 - b'a' + 10,
            'A'..='F' => ch as u8 - b'A' + 10,
            _ => {
                return Err(RuntimeError::Eval(format!(
                    "invalid hex character `{ch}` in ~hex literal"
                )))
            }
        };
        nibbles.push(n);
    }
    if nibbles.len() % 2 != 0 {
        return Err(RuntimeError::Eval(format!(
            "~hex literal has odd nibble count ({})",
            nibbles.len()
        )));
    }
    Ok(nibbles
        .chunks_exact(2)
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::Registry;
    use super::*;

    #[test]
    fn registered() {
        assert!(Registry::global().get("hex").is_some());
    }

    #[test]
    fn output_type_is_bytes() {
        assert_eq!(Hex.output_type(), ValueType::Bytes);
    }

    #[test]
    fn decodes_uppercase() {
        let frags = vec![StringFragment::Text("DEADBEEF".into())];
        let v = Hex.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn decodes_lowercase() {
        let frags = vec![StringFragment::Text("deadbeef".into())];
        let v = Hex.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn whitespace_is_ignored() {
        let frags = vec![StringFragment::Text("DE AD\tBE\nEF".into())];
        let v = Hex.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn empty_decodes_to_empty_bytes() {
        let frags = vec![StringFragment::Text("".into())];
        let v = Hex.eval(&frags, &Scope::new()).unwrap();
        assert_eq!(v, Value::Bytes(vec![]));
    }

    #[test]
    fn odd_nibble_count_errors() {
        let frags = vec![StringFragment::Text("DEA".into())];
        assert!(Hex.eval(&frags, &Scope::new()).is_err());
    }

    #[test]
    fn invalid_char_errors() {
        let frags = vec![StringFragment::Text("DEADBEEG".into())];
        assert!(Hex.eval(&frags, &Scope::new()).is_err());
    }
}
