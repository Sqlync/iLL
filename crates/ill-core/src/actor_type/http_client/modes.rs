use crate::actor_type::Mode;

pub struct Ready;

impl Mode for Ready {
    fn name(&self) -> &'static str {
        "ready"
    }
    fn actor_type(&self) -> &'static str {
        "http_client"
    }
}

pub static READY: &dyn Mode = &Ready;
