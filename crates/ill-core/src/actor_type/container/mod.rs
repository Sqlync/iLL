// The `container` actor type — a process running in a container, declared with
// either a pre-built image or a Dockerfile path.

pub mod commands;
pub mod modes;

use super::{ActorType, Command, KeywordArgDef, Mode, ValueType};

pub struct Container;

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
        // Exactly one of `image` or `dockerfile` should ultimately be required;
        // Phase 4 accepts either (or neither, for purely parameterized declarations).
        // Tighten once the grammar settles.
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
}

pub static CONTAINER: &dyn ActorType = &Container;
