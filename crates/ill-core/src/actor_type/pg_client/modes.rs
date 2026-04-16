use crate::actor_type::Mode;

pub struct Disconnected;
pub struct Connected;

impl Mode for Disconnected {
    fn name(&self) -> &'static str {
        "disconnected"
    }
    fn actor_type(&self) -> &'static str {
        "pg_client"
    }
}

impl Mode for Connected {
    fn name(&self) -> &'static str {
        "connected"
    }
    fn actor_type(&self) -> &'static str {
        "pg_client"
    }
}

pub static DISCONNECTED: &dyn Mode = &Disconnected;
pub static CONNECTED: &dyn Mode = &Connected;
