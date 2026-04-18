// The `exec` actor type — a process running directly on the host.
//
// Intended for long-running things like servers, daemons, and brokers. The
// `command` is supplied at declaration; `run` starts it and transitions the
// actor from `stopped` to `running`.

pub mod commands;
pub mod modes;
pub mod runtime;

use super::{ActorInstance, ActorType, Command, KeywordArgDef, Mode, ValueType};
use crate::runtime::{CommandArgs, RunOutcome, RuntimeError, SpawnArgs};

pub struct Exec;

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
        &[KeywordArgDef {
            name: "command",
            ty: ValueType::String,
            required: true,
        }]
    }

    fn spawn(&self, args: &SpawnArgs) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        let inst = runtime::ExecInstance::spawn(args)?;
        Ok(Box::new(inst))
    }

    fn execute(
        &self,
        cmd: &'static str,
        instance: &mut dyn ActorInstance,
        args: &CommandArgs,
    ) -> RunOutcome {
        let exec = instance
            .as_any_mut()
            .downcast_mut::<runtime::ExecInstance>()
            .expect("harness routes commands to matching actor instances");
        match cmd {
            "run" => exec.run(args.kw("env")),
            other => RunOutcome::NotImplemented {
                actor: self.name(),
                cmd: other,
            },
        }
    }
}

pub static EXEC: &dyn ActorType = &Exec;
