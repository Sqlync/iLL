use super::modes::{RUNNING, STOPPED};
use crate::actor_type::{Command, KeywordArgDef, Mode, OutcomeField, ValueType};
use crate::define_outcome;

define_outcome! {
    /// Result of `exec.run` — the spawned child's process id.
    pub RunOk {
        pid: Number,
    }
}

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
        // `env` is a map; Phase 4 only checks the name, not the value shape.
        &[KeywordArgDef {
            name: "env",
            ty: ValueType::Unknown,
            required: false,
        }]
    }

    fn ok_fields(&self) -> &'static [OutcomeField] {
        RunOk::FIELDS
    }
}

pub static RUN: &dyn Command = &Run;
