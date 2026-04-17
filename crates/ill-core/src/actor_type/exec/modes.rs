use crate::actor_type::Mode;

pub struct Idle;
pub struct Running;

impl Mode for Idle {
    fn name(&self) -> &'static str {
        "idle"
    }
    fn actor_type(&self) -> &'static str {
        "exec"
    }
}

impl Mode for Running {
    fn name(&self) -> &'static str {
        "running"
    }
    fn actor_type(&self) -> &'static str {
        "exec"
    }
}

pub static IDLE: &dyn Mode = &Idle;
pub static RUNNING: &dyn Mode = &Running;
