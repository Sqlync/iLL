use super::modes::{RUNNING, STOPPED};
use crate::actor_type::{Command, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType};
use crate::define_outcome;

define_outcome! {
    /// Result of `exec.run` — the spawned child's process id.
    pub RunOk {
        pid: Number,
    }
}

define_outcome! {
    /// Fields on `error.exec.*` when `exec.run` fails. `reason` is one of
    /// the atoms classified in `super::runtime`: `:invalid_command`,
    /// `:command_not_found`, `:permission_denied`, `:spawn_failed`,
    /// `:bad_env`, `:already_running`.
    pub ExecError {
        reason: Atom,
    }
}

static EXEC_ERROR_TYPE: ErrorTypeDef = ErrorTypeDef {
    name: "exec",
    fields: ExecError::FIELDS,
};

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

    fn error_types(&self) -> &'static [ErrorTypeDef] {
        std::slice::from_ref(&EXEC_ERROR_TYPE)
    }
}

pub static RUN: &dyn Command = &Run;
