use super::modes::{RUNNING, STOPPED};
use crate::actor_type::{Command, KeywordArgDef, Mode, ValueType};

pub struct Run;

impl Command for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[STOPPED];
        VALID
    }

    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(RUNNING)
    }

    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[
            KeywordArgDef { name: "port", ty: ValueType::Number, required: false },
            // `env` is a map; Phase 4 only checks the name, not the value shape.
            KeywordArgDef { name: "env", ty: ValueType::Unknown, required: false },
            KeywordArgDef { name: "timeout", ty: ValueType::Number, required: false },
        ]
    }
}

pub struct Stop;

impl Command for Stop {
    fn name(&self) -> &'static str {
        "stop"
    }

    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[RUNNING];
        VALID
    }

    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(STOPPED)
    }
}

pub static RUN: &dyn Command = &Run;
pub static STOP: &dyn Command = &Stop;
