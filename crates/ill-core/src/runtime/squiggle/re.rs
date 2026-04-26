use crate::actor_type::ValueType;

use super::Squiggle;

/// `~re` — a regex pattern. Backtick-delimited content is raw, so users can
/// write `\.`, `\d`, `\s` directly without double-escaping. The value is a
/// plain `String`; `assert ... matches` compiles it via the `regex` crate.
pub struct Re;

impl Squiggle for Re {
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

    #[test]
    fn registered() {
        assert!(Registry::global().get("re").is_some());
    }

    #[test]
    fn declares_string_output() {
        assert_eq!(Re.output_type(), ValueType::String);
    }
}
