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
        // `image` and `dockerfile` are individually optional but exactly
        // one is required; that cross-kwarg invariant is enforced in
        // `ContainerInstance::construct`.
        //
        // `internal_port` names the port the process inside the container
        // listens on (image fact). It pairs with the per-invocation
        // `external_port:` kwarg on `run` to drive the host→container
        // mapping. Optional — containers that don't expose anything (e.g.
        // a one-shot CMD that exits) leave it unset.
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
            KeywordArgDef {
                name: "internal_port",
                ty: ValueType::Number,
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
