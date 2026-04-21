use crate::actor_type::ValueType;

use super::Sigil;

/// `~hex` — stub. Evaluates as the rendered string for now, so `output_type`
/// is honestly `String`. When a consumer (e.g. mqtt) needs real binary
/// payloads, override `eval` to hex-decode into `Value::Bytes` and bump
/// `output_type` to `Bytes`.
pub struct Hex;

impl Sigil for Hex {
    fn name(&self) -> &'static str {
        "hex"
    }

    fn output_type(&self) -> ValueType {
        ValueType::String
    }
}

#[cfg(test)]
mod tests {
    use super::super::Registry;

    #[test]
    fn registered() {
        assert!(Registry::global().get("hex").is_some());
    }
}
