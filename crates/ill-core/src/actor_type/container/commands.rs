use super::modes::{RUNNING, STOPPED};
use crate::actor_type::{Command, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType};
use crate::define_outcome;

define_outcome! {
    /// Result of `container.run` — the started container's id and its mapped
    /// host port. `port` is 0 when the user did not supply a `port:` kwarg.
    pub RunOk {
        id: String,
        port: Number,
    }
}

define_outcome! {
    /// Fields on `error.container.*` for failing container commands.
    ///
    /// Atoms produced by `run`: `:timeout`, `:already_running`,
    /// `:docker_unavailable`, `:bad_env`, `:bad_port`.
    ///
    /// Atoms produced by `stop`: `:not_running`, `:timeout`,
    /// `:docker_unavailable`.
    ///
    /// Image acquisition failures (missing image, pull failure, Dockerfile
    /// build failure) surface at construct time as `ConstructFailure`, not
    /// here — by the time `run` executes, the image is already resolved.
    pub ContainerError {
        reason: Atom,
    }
}

static CONTAINER_ERROR_TYPES: &[ErrorTypeDef] = &[ErrorTypeDef {
    name: "container",
    fields: ContainerError::FIELDS,
}];

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
            KeywordArgDef {
                name: "port",
                ty: ValueType::Number,
                required: false,
            },
            // `env` is a map; Phase 4 only checks the name, not the value shape.
            KeywordArgDef {
                name: "env",
                ty: ValueType::Unknown,
                required: false,
            },
            KeywordArgDef {
                name: "timeout",
                ty: ValueType::Number,
                required: false,
            },
        ]
    }

    fn ok_fields(&self) -> &'static [OutcomeField] {
        RunOk::FIELDS
    }

    fn error_types(&self) -> &'static [ErrorTypeDef] {
        CONTAINER_ERROR_TYPES
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

    fn error_types(&self) -> &'static [ErrorTypeDef] {
        CONTAINER_ERROR_TYPES
    }
}

pub static RUN: &dyn Command = &Run;
pub static STOP: &dyn Command = &Stop;
