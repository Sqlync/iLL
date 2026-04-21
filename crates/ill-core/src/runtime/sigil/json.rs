use crate::actor_type::ValueType;

use super::Sigil;

/// `~json` — stub. Evaluates as the rendered string for now, so `output_type`
/// is honestly `String`. When the http actor actually consumes JSON bodies,
/// override `eval` to produce a structured `Value::Dict` and bump
/// `output_type` to `Dynamic`.
pub struct Json;

impl Sigil for Json {
    fn name(&self) -> &'static str {
        "json"
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
        assert!(Registry::global().get("json").is_some());
    }
}
