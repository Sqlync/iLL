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
        // `cwd` overrides the working directory the child process is spawned
        // in. Defaults to the `.ill` file's directory (so relative `command`
        // paths and child-process file lookups stay anchored to the test).
        // Relative `cwd` values resolve against the same `.ill` directory;
        // absolute values are used as-is.
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
