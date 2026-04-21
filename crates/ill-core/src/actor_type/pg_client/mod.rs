// The `pg_client` actor type — a Postgres client that connects to a
// database and runs queries. Construct is lazy: no network I/O happens
// at declaration time, so all auth / network failures surface through
// `error.network.*` / `error.connect.*` on the `connect` command. See
// `runtime.rs` for the state machine.

pub mod commands;
pub mod convert;
pub mod modes;
pub mod runtime;

use super::{ActorInstance, ActorType, Command, Mode};
use crate::runtime::{ConstructArgs, RuntimeError};

pub struct PgClient;

#[async_trait::async_trait]
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

    async fn construct(
        &self,
        args: &ConstructArgs,
    ) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        let inst = runtime::PgClientInstance::construct(args).await?;
        Ok(Box::new(inst))
    }
}

pub static PG_CLIENT: &dyn ActorType = &PgClient;
