use super::modes::{IDLE, RUNNING};
use crate::actor_type::{Command, KeywordArgDef, Mode, ValueType};

pub struct Run;

impl Command for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[IDLE];
        VALID
    }

    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(RUNNING)
    }

    fn keyword(&self) -> &'static [KeywordArgDef] {
        // `env` is a map; Phase 4 only checks the name, not the value shape.
        &[KeywordArgDef {
            name: "env",
            ty: ValueType::Unknown,
            required: false,
        }]
    }
}

pub static RUN: &dyn Command = &Run;
