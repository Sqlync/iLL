// The `args_actor` built-in — exposes command-line arguments as member vars.
//
// Single mode; `check` validates self.* invariants via following `assert`s.
//
// Process-wide `--arg KEY=VALUE` values live in `CLI_ARGS` below rather
// than being threaded through `ConstructArgs`, since no other actor type
// consumes them. The `ill` binary calls `set_cli_args` once at startup;
// every `args_actor` construct reads from the same slot.

pub mod runtime;

use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::actor_type::{ActorInstance, ActorType, Command, Mode};
use crate::runtime::{ConstructArgs, RuntimeError};

static CLI_ARGS: RwLock<BTreeMap<String, String>> = RwLock::new(BTreeMap::new());

/// Install the `--arg KEY=VALUE` map parsed by the CLI. Later calls
/// overwrite earlier ones; intended to be called once per process.
pub fn set_cli_args(args: BTreeMap<String, String>) {
    *CLI_ARGS.write().expect("CLI_ARGS lock poisoned") = args;
}

fn cli_args_snapshot() -> BTreeMap<String, String> {
    CLI_ARGS.read().expect("CLI_ARGS lock poisoned").clone()
}

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
        let cli = cli_args_snapshot();
        Ok(Box::new(runtime::ArgsActorInstance::construct(args, &cli)?))
    }
}

pub static ARGS_ACTOR: &dyn ActorType = &ArgsActor;
