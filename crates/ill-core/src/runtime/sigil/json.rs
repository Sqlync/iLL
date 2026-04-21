use super::Sigil;

/// `~json` — stub. Evaluates as the rendered string for now. When the http
/// actor actually consumes JSON bodies this should parse + re-emit canonical
/// form, or produce a structured `Value::Dict`.
pub struct Json;

impl Sigil for Json {
    fn name(&self) -> &'static str {
        "json"
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
