// Runtime half of the `pg_client` actor. State machine over the actor's
// modes: `Disconnected` carries no live TCP resource; `Connected` owns
// the `tokio_postgres::Client` plus the `JoinHandle` of the background
// task driving the `Connection` future. On teardown we abort the task
// and drop the client, which closes the TCP connection.
//
// Construct is lazy — no I/O at declaration time. Connection only
// happens on `connect`, matching the example `actor alice = pg_client`
// (no kwargs on declaration). This mirrors the intent that
// `pg_client.connect` is where auth / network failures live.
//
// Error surface follows the command declarations in `commands.rs`:
//   connect  → `error.network.reason` | `error.connect.reason`
//   query    → `error.network.reason` | `error.query.reason`

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config, NoTls};

use super::commands::{ConnectError, ConnectOk, NetworkError, QueryError};
use super::convert::build_result_dict;
use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, Dict, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

// `127.0.0.1` rather than `localhost`: on systems where `localhost` resolves
// to both `::1` and `127.0.0.1`, tokio-postgres may stall on the IPv6 attempt
// when Docker forwards container ports on IPv4 only — burning the connect
// timeout budget for nothing. A literal IPv4 address sidesteps the DNS race.
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 5432;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// Network reasons
const NET_HOST_UNREACHABLE: &str = "host_unreachable";
const NET_CONNECTION_REFUSED: &str = "connection_refused";
const NET_CONNECTION_LOST: &str = "connection_lost";
const NET_TIMEOUT: &str = "timeout";
const NET_TLS: &str = "tls";
const NET_OTHER: &str = "other";

// Connect reasons
const CONNECT_AUTH_FAILED: &str = "authentication_failed";
const CONNECT_BAD_DATABASE: &str = "bad_database";
const CONNECT_OTHER: &str = "other";

// Query reasons
const QUERY_SYNTAX_ERROR: &str = "syntax_error";
const QUERY_CONSTRAINT_VIOLATION: &str = "constraint_violation";
const QUERY_TIMEOUT: &str = "timeout";
const QUERY_OTHER: &str = "other";

fn network_error(reason: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "network",
        fields: NetworkError {
            reason: reason.into(),
        }
        .into_dict(),
    }
}

fn connect_error(reason: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "connect",
        fields: ConnectError {
            reason: reason.into(),
        }
        .into_dict(),
    }
}

fn query_error(reason: &str, sqlstate: &str, message: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "query",
        fields: QueryError {
            reason: reason.into(),
            sqlstate: sqlstate.into(),
            message: message.into(),
        }
        .into_dict(),
    }
}

pub struct PgClientInstance {
    mode: PgMode,
}

pub enum PgMode {
    Disconnected(Disconnected),
    Connected(Connected),
}

impl Default for PgMode {
    fn default() -> Self {
        PgMode::Disconnected(Disconnected)
    }
}

pub struct Disconnected;

pub struct Connected {
    client: Client,
    /// Drives the `tokio_postgres::Connection` future. Aborting this
    /// closes the TCP connection without waiting for a graceful shutdown
    /// — which is what we want on teardown.
    conn_task: JoinHandle<()>,
}

impl PgClientInstance {
    pub async fn construct(_args: &ConstructArgs) -> Result<Self, RuntimeError> {
        // No I/O here. `pg_client` takes no declaration-time kwargs; the
        // connection is established lazily by `connect`. That keeps the
        // construct-time error surface empty and routes all network /
        // auth failures through `error.network.*` / `error.connect.*`.
        Ok(PgClientInstance {
            mode: PgMode::default(),
        })
    }
}

impl Disconnected {
    async fn connect(self, kw: &Dict) -> (PgMode, RunOutcome) {
        let mut cfg = Config::new();

        match kw.get("host") {
            Some(Value::String(s)) => {
                cfg.host(s);
            }
            None => {
                cfg.host(DEFAULT_HOST);
            }
            Some(_) => {
                // Validator catches type mismatches on declared kwargs; if a
                // mismatched value gets here, treat it as a bad config and
                // return a generic connect error rather than panicking.
                return (PgMode::Disconnected(self), connect_error(CONNECT_OTHER));
            }
        }

        let port = match kw.get("port") {
            Some(Value::Number(n)) if *n > 0 && *n <= u16::MAX as i64 => *n as u16,
            Some(Value::Number(_)) => {
                return (PgMode::Disconnected(self), connect_error(CONNECT_OTHER));
            }
            Some(_) => {
                return (PgMode::Disconnected(self), connect_error(CONNECT_OTHER));
            }
            None => DEFAULT_PORT,
        };
        cfg.port(port);

        match kw.get("user") {
            Some(Value::String(s)) => {
                cfg.user(s);
            }
            _ => {
                // Validator enforces `user` is required; this branch is
                // defensive for direct ActorInstance use (tests).
                return (PgMode::Disconnected(self), connect_error(CONNECT_OTHER));
            }
        }

        if let Some(Value::String(s)) = kw.get("password") {
            cfg.password(s);
        }

        match kw.get("database") {
            Some(Value::String(s)) => {
                cfg.dbname(s);
            }
            _ => {
                return (PgMode::Disconnected(self), connect_error(CONNECT_OTHER));
            }
        }

        let timeout = match kw.get("timeout") {
            Some(Value::Number(n)) if *n > 0 => Duration::from_millis(*n as u64),
            _ => DEFAULT_CONNECT_TIMEOUT,
        };
        cfg.connect_timeout(timeout);

        // Retry transient transport failures (refused / unreachable / TCP
        // timeout) up to the overall `timeout` budget. Postgres inside a
        // container we just started may not yet be accepting TCP — the
        // user's intent is "give me a connection in <timeout> ms," not
        // "TCP must be ready right now." Auth and bad-database errors are
        // not retried; they're returned as soon as they occur.
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last_outcome: Option<RunOutcome> = None;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return (
                    PgMode::Disconnected(self),
                    last_outcome.unwrap_or_else(|| network_error(NET_TIMEOUT)),
                );
            }
            let attempt = cfg.connect(NoTls);
            match tokio::time::timeout(remaining, attempt).await {
                Err(_) => {
                    return (PgMode::Disconnected(self), network_error(NET_TIMEOUT));
                }
                Ok(Ok((client, connection))) => {
                    let conn_task = tokio::spawn(async move {
                        // Errors on the background connection are absorbed
                        // here. Subsequent `client.query` failures surface
                        // as `error.network.reason == :connection_lost`.
                        let _ = connection.await;
                    });
                    let ok = ConnectOk { is_connected: true };
                    return (
                        PgMode::Connected(Connected { client, conn_task }),
                        RunOutcome::Ok(ok.into_dict()),
                    );
                }
                Ok(Err(e)) => {
                    let outcome = classify_connect_error(&e);
                    if !is_transient_connect_error(&outcome) {
                        return (PgMode::Disconnected(self), outcome);
                    }
                    last_outcome = Some(outcome);
                    // Brief backoff so we don't spin while postgres is
                    // still binding its socket. Capped well under the
                    // typical retry budget.
                    let backoff = std::cmp::min(Duration::from_millis(200), remaining);
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}

/// True if a connect-time outcome warrants retrying within the timeout
/// budget. Anything that came back as a `connect` variant (auth failed,
/// bad database) is a real server response and is surfaced immediately.
/// All `network` variants are transient — they cover the window where
/// the TCP socket isn't yet open, the handshake gets cut off mid-stream,
/// or the postgres process is still binding its port.
fn is_transient_connect_error(outcome: &RunOutcome) -> bool {
    matches!(
        outcome,
        RunOutcome::Error {
            variant: "network",
            ..
        }
    )
}

impl Connected {
    async fn query(self, sql: &str, timeout_kw: Option<&Value>) -> (PgMode, RunOutcome) {
        let run = async {
            // `client.query` works for DDL/DML as well — non-row-producing
            // statements yield an empty `Vec<Row>`.
            self.client.query(sql, &[]).await
        };

        let result = match timeout_kw {
            Some(Value::Number(n)) if *n > 0 => {
                let dur = Duration::from_millis(*n as u64);
                match tokio::time::timeout(dur, run).await {
                    Ok(r) => r,
                    Err(_) => {
                        return (
                            PgMode::Connected(self),
                            query_error(QUERY_TIMEOUT, "", "query timeout"),
                        );
                    }
                }
            }
            _ => run.await,
        };

        match result {
            Ok(rows) => {
                let fields = build_result_dict(&rows);
                (PgMode::Connected(self), RunOutcome::Ok(fields))
            }
            Err(e) => (PgMode::Connected(self), classify_query_error(&e)),
        }
    }

    async fn disconnect(self) -> (PgMode, RunOutcome) {
        self.conn_task.abort();
        drop(self.client);
        (
            PgMode::Disconnected(Disconnected),
            RunOutcome::Ok(Dict::new()),
        )
    }

    async fn teardown(self) -> (PgMode, TeardownOutcome) {
        self.conn_task.abort();
        drop(self.client);
        (PgMode::Disconnected(Disconnected), TeardownOutcome::ok())
    }
}

fn classify_connect_error(e: &tokio_postgres::Error) -> RunOutcome {
    // Server-side DbError first — that's where auth / bad database live.
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        if *code == SqlState::INVALID_PASSWORD
            || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
        {
            return connect_error(CONNECT_AUTH_FAILED);
        }
        if *code == SqlState::INVALID_CATALOG_NAME {
            return connect_error(CONNECT_BAD_DATABASE);
        }
        // Fallback for other server-origin connect errors.
        return connect_error(CONNECT_OTHER);
    }

    // Transport-level: walk the std::error::Error chain looking for an
    // io::Error. That's what tokio-postgres wraps for connection-refused,
    // DNS failures, and unreachable hosts.
    if let Some(io) = find_io_error(e) {
        return match io.kind() {
            std::io::ErrorKind::ConnectionRefused => network_error(NET_CONNECTION_REFUSED),
            std::io::ErrorKind::TimedOut => network_error(NET_TIMEOUT),
            _ => network_error(NET_HOST_UNREACHABLE),
        };
    }

    // TLS errors don't come from `NoTls`, but if TLS is ever wired in
    // they'd show up here — leave the atom available so the grammar
    // doesn't need another pass.
    if e.to_string().to_lowercase().contains("tls") {
        return network_error(NET_TLS);
    }

    network_error(NET_OTHER)
}

fn classify_query_error(e: &tokio_postgres::Error) -> RunOutcome {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        let sqlstate = code.code().to_string();
        let message = db.message().to_string();

        let reason = if *code == SqlState::SYNTAX_ERROR
            || *code == SqlState::UNDEFINED_COLUMN
            || *code == SqlState::UNDEFINED_TABLE
            || *code == SqlState::UNDEFINED_FUNCTION
            || *code == SqlState::UNDEFINED_OBJECT
        {
            QUERY_SYNTAX_ERROR
        } else if code.code().starts_with("23") {
            // 23xxx — integrity_constraint_violation class (unique,
            // foreign_key, not_null, check, exclusion).
            QUERY_CONSTRAINT_VIOLATION
        } else {
            QUERY_OTHER
        };
        return query_error(reason, &sqlstate, &message);
    }

    // No DbError — either we lost the connection or something transport-level.
    if find_io_error(e).is_some() || e.is_closed() {
        return network_error(NET_CONNECTION_LOST);
    }

    query_error(QUERY_OTHER, "", &e.to_string())
}

/// Walk a `tokio_postgres::Error`'s source chain looking for an `io::Error`.
fn find_io_error(e: &tokio_postgres::Error) -> Option<&std::io::Error> {
    use std::error::Error;
    let mut cur: &dyn Error = e;
    loop {
        if let Some(io) = cur.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        match cur.source() {
            Some(next) => cur = next,
            None => return None,
        }
    }
}

#[async_trait::async_trait]
impl ActorInstance for PgClientInstance {
    fn type_name(&self) -> &'static str {
        "pg_client"
    }

    async fn execute(&mut self, cmd: &'static str, args: &CommandArgs) -> RunOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            PgMode::Disconnected(d) => match cmd {
                "connect" => d.connect(&args.keyword).await,
                "disconnect" | "query" => (
                    PgMode::Disconnected(d),
                    RunOutcome::NotImplemented {
                        actor: "pg_client",
                        cmd,
                    },
                ),
                other => (
                    PgMode::Disconnected(d),
                    RunOutcome::NotImplemented {
                        actor: "pg_client",
                        cmd: other,
                    },
                ),
            },
            PgMode::Connected(c) => match cmd {
                "query" => {
                    let sql = match args.positional.first() {
                        Some(Value::String(s)) => s.clone(),
                        _ => {
                            // Validator enforces `sql: String` positional;
                            // defensive fallback preserves the mode.
                            return {
                                self.mode = PgMode::Connected(c);
                                query_error(QUERY_OTHER, "", "missing sql argument")
                            };
                        }
                    };
                    c.query(&sql, args.kw("timeout")).await
                }
                "disconnect" => c.disconnect().await,
                "connect" => (
                    PgMode::Connected(c),
                    RunOutcome::NotImplemented {
                        actor: "pg_client",
                        cmd,
                    },
                ),
                other => (
                    PgMode::Connected(c),
                    RunOutcome::NotImplemented {
                        actor: "pg_client",
                        cmd: other,
                    },
                ),
            },
        };
        self.mode = next;
        outcome
    }

    async fn teardown(&mut self) -> TeardownOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            PgMode::Disconnected(d) => (PgMode::Disconnected(d), TeardownOutcome::ok()),
            PgMode::Connected(c) => c.teardown().await,
        };
        self.mode = next;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Postgres-gated tests ───────────────────────────────────────────────
    //
    // These spin up a real `postgres:18` testcontainer and exercise the
    // state machine end-to-end. `#[ignore]` by default so `cargo test`
    // stays offline-friendly. Run locally with:
    //
    //     cargo test -p ill-core --lib pg_client -- --ignored
    //
    // First run pulls the image (multiple seconds); subsequent runs hit
    // the daemon cache.

    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt};

    const PG_PASSWORD: &str = "postgres_test_pw";
    const PG_USER: &str = "postgres";
    const PG_DB: &str = "postgres";

    async fn start_pg() -> ContainerAsync<GenericImage> {
        GenericImage::new("postgres", "18")
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_PASSWORD", PG_PASSWORD)
            .start()
            .await
            .expect("start postgres container")
    }

    async fn host_port(c: &ContainerAsync<GenericImage>) -> u16 {
        c.get_host_port_ipv4(5432.tcp())
            .await
            .expect("get host port")
    }

    fn connect_args(port: u16, user: &str, password: &str, database: &str) -> CommandArgs {
        let mut kw = Dict::new();
        kw.insert("host".into(), Value::String("127.0.0.1".into()));
        kw.insert("port".into(), Value::Number(port as i64));
        kw.insert("user".into(), Value::String(user.into()));
        kw.insert("password".into(), Value::String(password.into()));
        kw.insert("database".into(), Value::String(database.into()));
        CommandArgs {
            positional: Vec::new(),
            keyword: kw,
        }
    }

    fn query_args(sql: &str) -> CommandArgs {
        CommandArgs {
            positional: vec![Value::String(sql.into())],
            keyword: Dict::new(),
        }
    }

    fn empty_construct() -> ConstructArgs {
        ConstructArgs {
            keyword: Dict::new(),
            source_dir: std::env::temp_dir(),
            vars: Vec::new(),
        }
    }

    fn expect_ok(o: RunOutcome) -> Dict {
        match o {
            RunOutcome::Ok(f) => f,
            RunOutcome::Error { variant, fields } => {
                panic!("expected Ok, got Error {variant}: {fields:?}")
            }
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected Ok, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    fn expect_error(o: RunOutcome, expected_variant: &str, expected_reason: &str) {
        match o {
            RunOutcome::Error { variant, fields } => {
                assert_eq!(variant, expected_variant, "error variant mismatch");
                match fields.get("reason") {
                    Some(Value::Atom(a)) => assert_eq!(a, expected_reason, "reason mismatch"),
                    other => panic!("expected .reason atom, got {other:?}"),
                }
            }
            RunOutcome::Ok(f) => panic!("expected Error, got Ok({f:?})"),
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected Error, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_query_disconnect_happy_path() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        assert!(matches!(inst.mode, PgMode::Disconnected(_)));

        let ok = expect_ok(
            inst.execute("connect", &connect_args(port, PG_USER, PG_PASSWORD, PG_DB))
                .await,
        );
        assert_eq!(ok.get("is_connected"), Some(&Value::Bool(true)));
        assert!(matches!(inst.mode, PgMode::Connected(_)));

        // DDL
        let _ = expect_ok(
            inst.execute(
                "query",
                &query_args("CREATE TABLE users (id INT, name TEXT)"),
            )
            .await,
        );
        // DML
        let _ = expect_ok(
            inst.execute(
                "query",
                &query_args("INSERT INTO users VALUES (1, 'alice')"),
            )
            .await,
        );
        let _ = expect_ok(
            inst.execute("query", &query_args("INSERT INTO users VALUES (2, 'bob')"))
                .await,
        );
        // SELECT — verify shape
        let ok = expect_ok(
            inst.execute("query", &query_args("SELECT * FROM users ORDER BY id"))
                .await,
        );
        assert_eq!(ok.get("row_count"), Some(&Value::Number(2)));
        assert_eq!(ok.get("col_count"), Some(&Value::Number(2)));
        match ok.get("row") {
            Some(Value::Array(rows)) => {
                assert_eq!(rows.len(), 2);
                match &rows[0] {
                    Value::Array(cells) => {
                        assert_eq!(cells[0], Value::Number(1));
                        assert_eq!(cells[1], Value::String("alice".into()));
                    }
                    other => panic!("row[0] not an array: {other:?}"),
                }
            }
            other => panic!("ok.row not an array: {other:?}"),
        }

        // disconnect returns to Disconnected
        let _ = expect_ok(inst.execute("disconnect", &empty_cmd_args()).await);
        assert!(matches!(inst.mode, PgMode::Disconnected(_)));

        let td = inst.teardown().await;
        assert!(td.ok, "teardown failed: {:?}", td.message);
    }

    fn empty_cmd_args() -> CommandArgs {
        CommandArgs {
            positional: Vec::new(),
            keyword: Dict::new(),
        }
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_bad_password_surfaces_auth_failed() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let outcome = inst
            .execute("connect", &connect_args(port, PG_USER, "wrong", PG_DB))
            .await;
        expect_error(outcome, "connect", "authentication_failed");
        assert!(matches!(inst.mode, PgMode::Disconnected(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_bad_database_surfaces_bad_database() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let outcome = inst
            .execute(
                "connect",
                &connect_args(port, PG_USER, PG_PASSWORD, "no_such_db"),
            )
            .await;
        expect_error(outcome, "connect", "bad_database");
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_refused_surfaces_network_error() {
        // Connect to a port nothing is listening on.
        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        // Use a tight timeout so the test doesn't wait 10s.
        let mut kw = Dict::new();
        kw.insert("host".into(), Value::String("127.0.0.1".into()));
        kw.insert("port".into(), Value::Number(1));
        kw.insert("user".into(), Value::String(PG_USER.into()));
        kw.insert("password".into(), Value::String(PG_PASSWORD.into()));
        kw.insert("database".into(), Value::String(PG_DB.into()));
        kw.insert("timeout".into(), Value::Number(1000));
        let outcome = inst
            .execute(
                "connect",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        // Could be :connection_refused on most Unix or :host_unreachable elsewhere;
        // either way it's the `network` variant.
        match outcome {
            RunOutcome::Error { variant, .. } => assert_eq!(variant, "network"),
            RunOutcome::Ok(f) => panic!("expected network error, got Ok({f:?})"),
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected network error, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn syntax_error_surfaces_as_query_error() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(
            inst.execute("connect", &connect_args(port, PG_USER, PG_PASSWORD, PG_DB))
                .await,
        );

        let outcome = inst.execute("query", &query_args("SELEC bad")).await;
        expect_error(outcome, "query", "syntax_error");
        // Still connected after a query error.
        assert!(matches!(inst.mode, PgMode::Connected(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn reconnect_after_disconnect() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(
            inst.execute("connect", &connect_args(port, PG_USER, PG_PASSWORD, PG_DB))
                .await,
        );
        let _ = expect_ok(inst.execute("disconnect", &empty_cmd_args()).await);
        assert!(matches!(inst.mode, PgMode::Disconnected(_)));
        let _ = expect_ok(
            inst.execute("connect", &connect_args(port, PG_USER, PG_PASSWORD, PG_DB))
                .await,
        );
        assert!(matches!(inst.mode, PgMode::Connected(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn teardown_while_connected_cleans_up() {
        let pg = start_pg().await;
        let port = host_port(&pg).await;

        let mut inst = PgClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(
            inst.execute("connect", &connect_args(port, PG_USER, PG_PASSWORD, PG_DB))
                .await,
        );
        let td = inst.teardown().await;
        assert!(td.ok);
        assert!(matches!(inst.mode, PgMode::Disconnected(_)));
    }

    /// End-to-end coverage through the `.ill` harness: a container actor
    /// runs postgres, a pg_client actor connects through cross-actor
    /// `db.port` resolution, and queries assert against the result. This
    /// is the integration the unit tests above can't reach by themselves
    /// — they hit `ActorInstance` directly and don't exercise the
    /// validator, lower, or scope-binding paths.
    #[tokio::test]
    #[ignore = "requires docker"]
    async fn end_to_end_through_harness() {
        use crate::runtime::harness::run_test_file;
        use std::path::Path;

        let src = "\
actor db = container,
  image: \"postgres:18\"
  vars:
    @access read
    port: 5432

actor alice = pg_client

as db:
  run,
    port: self.port
    env:
      POSTGRES_PASSWORD: \"pw\"
      POSTGRES_USER: \"postgres\"
      POSTGRES_DB: \"postgres\"

as alice:
  connect,
    host: \"127.0.0.1\"
    user: \"postgres\"
    password: \"pw\"
    port: db.port
    database: \"postgres\"
    timeout: 30000

  query ~sql`CREATE TABLE users (id INT, name TEXT)`
  query ~sql`INSERT INTO users VALUES (1, 'alice')`
  query ~sql`SELECT * FROM users WHERE id = 1`
  assert ok.row[0] == [1, \"alice\"]
";

        let report = run_test_file(Path::new("e2e.ill"), src).await;
        assert!(
            report.passed,
            "expected pass, got {} failed statement(s)",
            report.statements.len()
        );
        // Both actors should have torn down cleanly.
        assert_eq!(report.teardown.len(), 2);
        assert!(report.teardown.iter().all(|t| t.outcome.ok));
    }
}
