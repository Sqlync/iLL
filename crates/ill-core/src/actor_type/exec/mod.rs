// The `exec` actor type — a process running directly on the host.
//
// Intended for long-running things like servers, daemons, and brokers. The
// `command` is supplied at declaration; `run` starts it and transitions the
// actor from `idle` to `running`.

pub mod commands;
pub mod modes;

use super::{ActorType, Command, KeywordArgDef, Mode, ValueType};

pub struct Exec;

impl ActorType for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::IDLE
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::IDLE, modes::RUNNING];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[commands::RUN];
        COMMANDS
    }

    fn constructor_keyword(&self) -> &'static [KeywordArgDef] {
        &[KeywordArgDef {
            name: "command",
            ty: ValueType::String,
            required: true,
        }]
    }
}

pub static EXEC: &dyn ActorType = &Exec;
