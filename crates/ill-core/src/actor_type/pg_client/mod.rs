// The `pg_client` actor type — a postgres client that connects to a database
// and runs queries.

pub mod commands;
pub mod modes;

use super::{ActorType, Command, Mode};

pub struct PgClient;

impl ActorType for PgClient {
    fn name(&self) -> &'static str {
        "pg_client"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::DISCONNECTED
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::DISCONNECTED, modes::CONNECTED];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] =
            &[commands::CONNECT, commands::QUERY, commands::DISCONNECT];
        COMMANDS
    }
}

pub static PG_CLIENT: &dyn ActorType = &PgClient;
