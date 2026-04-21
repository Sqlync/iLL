use super::Sigil;

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
    use super::super::Registry;

    #[test]
    fn registered() {
        assert!(Registry::global().get("hex").is_some());
    }
}
