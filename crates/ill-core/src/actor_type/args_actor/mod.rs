// The `args_actor` built-in — exposes command-line arguments as member vars.
//
// Single mode; `check` validates self.* invariants via following `assert`s.

pub mod runtime;

use crate::actor_type::{ActorInstance, ActorType, Command, Mode};
use crate::runtime::{ConstructArgs, RuntimeError};

pub struct Ready;
impl Mode for Ready {
    fn name(&self) -> &'static str {
        "ready"
    }
    fn actor_type(&self) -> &'static str {
        "args_actor"
    }
}
pub static READY: &dyn Mode = &Ready;

pub struct Check;
impl Command for Check {
    fn name(&self) -> &'static str {
        "check"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[READY];
        VALID
    }
}
pub static CHECK: &dyn Command = &Check;

pub struct ArgsActor;

#[async_trait::async_trait]
impl ActorType for ArgsActor {
    fn name(&self) -> &'static str {
        "args_actor"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        READY
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[READY];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[CHECK];
        COMMANDS
    }

    async fn construct(
        &self,
        args: &ConstructArgs,
    ) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        Ok(Box::new(runtime::ArgsActorInstance::construct(args)?))
    }
}

pub static ARGS_ACTOR: &dyn ActorType = &ArgsActor;
