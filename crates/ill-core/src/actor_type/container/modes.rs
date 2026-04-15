use crate::actor_type::Mode;

pub struct Stopped;
pub struct Running;

impl Mode for Stopped {
    fn name(&self) -> &'static str {
        "stopped"
    }
    fn actor_type(&self) -> &'static str {
        "container"
    }
}

impl Mode for Running {
    fn name(&self) -> &'static str {
        "running"
    }
    fn actor_type(&self) -> &'static str {
        "container"
    }
}

pub static STOPPED: &dyn Mode = &Stopped;
pub static RUNNING: &dyn Mode = &Running;
