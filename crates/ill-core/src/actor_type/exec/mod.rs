// The `exec` actor type — a process running directly on the host.
//
// Intended for long-running things like servers, daemons, and brokers. The
// `command` is supplied at declaration; `run` starts it and transitions the
// actor from `stopped` to `running`.

pub mod commands;
pub mod modes;
pub mod runtime;

use super::{ActorInstance, ActorType, Command, KeywordArgDef, Mode, ValueType};
use crate::runtime::{ConstructArgs, RuntimeError};

pub struct Exec;

#[async_trait::async_trait]
impl ActorType for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::STOPPED
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::STOPPED, modes::RUNNING];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[commands::RUN];
        COMMANDS
    }

    fn constructor_keyword(&self) -> &'static [KeywordArgDef] {
        &[
            KeywordArgDef {
                name: "command",
                ty: ValueType::String,
                required: true,
            },
            KeywordArgDef {
                name: "cwd",
                ty: ValueType::String,
                required: false,
            },
        ]
    }

    async fn construct(
        &self,
        args: &ConstructArgs,
    ) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        let inst = runtime::ExecInstance::construct(args)?;
        Ok(Box::new(inst))
    }
}

pub static EXEC: &dyn ActorType = &Exec;
