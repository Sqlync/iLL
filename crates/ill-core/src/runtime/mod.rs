// Phase 5 runtime. Mirrors the validator's shape: spawn all actors, then
// walk `as` blocks in order, dispatching per-actor commands. See the `exec`
// actor's `runtime.rs` for the first concrete implementation.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod value;

pub use value::Value;

pub mod assert;
pub mod eval;
pub mod harness;
pub mod report;

// ── Args passed to spawn / execute ────────────────────────────────────────────

/// Keyword arguments evaluated at an actor declaration site, plus the
/// directory containing the .ill file (used to resolve relative paths).
pub struct SpawnArgs {
    pub keyword: BTreeMap<String, Value>,
    pub source_dir: PathBuf,
}

/// Arguments passed to a command invocation. Positional + keyword.
pub struct CommandArgs {
    pub positional: Vec<Value>,
    pub keyword: BTreeMap<String, Value>,
}

impl SpawnArgs {
    pub fn kw(&self, name: &str) -> Option<&Value> {
        self.keyword.get(name)
    }
}

impl CommandArgs {
    pub fn kw(&self, name: &str) -> Option<&Value> {
        self.keyword.get(name)
    }
}

// ── Outcomes ──────────────────────────────────────────────────────────────────

/// The result of a command. `Ok` populates `ok.*`; `Error` populates `error.*`.
/// `NotImplemented` is the default for Phase 6 actors that haven't yet been
/// wired to a runtime.
pub enum RunOutcome {
    Ok(BTreeMap<String, Value>),
    Error(BTreeMap<String, Value>),
    NotImplemented {
        actor: &'static str,
        cmd: &'static str,
    },
}

/// The result of tearing down an actor instance. Teardown errors are reported
/// but don't overwrite a test failure that already happened earlier.
pub struct TeardownOutcome {
    pub ok: bool,
    pub message: Option<String>,
}

impl TeardownOutcome {
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(msg.into()),
        }
    }
}

// ── Runtime errors ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RuntimeError {
    ActorNotImplemented(&'static str),
    MissingKwarg {
        name: &'static str,
    },
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
        context: String,
    },
    Spawn(String),
    Eval(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::ActorNotImplemented(name) => {
                write!(f, "actor type `{name}` has no runtime implementation yet")
            }
            RuntimeError::MissingKwarg { name } => {
                write!(f, "missing required keyword arg `{name}`")
            }
            RuntimeError::TypeMismatch {
                expected,
                got,
                context,
            } => write!(
                f,
                "type mismatch in {context}: expected {expected}, got {got}"
            ),
            RuntimeError::Spawn(msg) => write!(f, "spawn failed: {msg}"),
            RuntimeError::Eval(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RuntimeError {}
