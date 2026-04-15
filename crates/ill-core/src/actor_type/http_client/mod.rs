// The `http_client` actor type — stateless HTTP request sender.
//
// Only one mode (`ready`): HTTP is stateless, so there are no transitions.

pub mod commands;
pub mod modes;

use super::{ActorType, Command, Mode};

pub struct HttpClient;

impl ActorType for HttpClient {
    fn name(&self) -> &'static str {
        "http_client"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::READY
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::READY];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[
            commands::GET,
            commands::POST,
            commands::PUT,
            commands::DELETE,
        ];
        COMMANDS
    }
}

pub static HTTP_CLIENT: &dyn ActorType = &HttpClient;
