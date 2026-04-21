// The `container` actor type — a process running in a container, declared
// with either a pre-built image or a Dockerfile path. Construct is eager:
// the image is pulled or built at declaration time so that `run` has no
// acquisition failures to surface. See `runtime.rs`.

pub mod commands;
pub mod modes;
pub mod runtime;

use super::{ActorInstance, ActorType, Command, KeywordArgDef, Mode, ValueType};
use crate::runtime::{ConstructArgs, RuntimeError};

pub struct Container;

#[async_trait::async_trait]
impl ActorType for Container {
    fn name(&self) -> &'static str {
        "container"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::STOPPED
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::STOPPED, modes::RUNNING];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[commands::RUN, commands::STOP];
        COMMANDS
    }

    fn constructor_keyword(&self) -> &'static [KeywordArgDef] {
        // The metadata says both are optional individually; the "exactly
        // one" check lives in `ContainerInstance::construct` because it's
        // a cross-kwarg invariant the current validator can't express.
        &[
            KeywordArgDef {
                name: "image",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "dockerfile",
                ty: ValueType::String,
                required: false,
            },
        ]
    }

    async fn construct(
        &self,
        args: &ConstructArgs,
    ) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        let inst = runtime::ContainerInstance::construct(args).await?;
        Ok(Box::new(inst))
    }
}

pub static CONTAINER: &dyn ActorType = &Container;
