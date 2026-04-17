use super::modes::{RUNNING, STOPPED};
use super::runtime::ExecInstance;
use crate::actor_type::{ActorInstance, Command, KeywordArgDef, Mode, OutcomeField, ValueType};
use crate::runtime::{CommandArgs, RunOutcome};

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
        &[OutcomeField {
            name: "pid",
            ty: ValueType::Number,
        }]
    }

    fn execute(&self, instance: &mut dyn ActorInstance, args: &CommandArgs) -> RunOutcome {
        let Some(exec) = instance.as_any_mut().downcast_mut::<ExecInstance>() else {
            return RunOutcome::NotImplemented {
                actor: instance.type_name(),
                cmd: "run",
            };
        };
        exec.run(args.kw("env"))
    }
}

pub static RUN: &dyn Command = &Run;
