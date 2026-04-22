// Phase 5 runtime. Mirrors the validator's shape: construct all actors, then
// walk `as` blocks in order, dispatching per-actor commands. See the `exec`
// actor's `runtime.rs` for the first concrete implementation.

use std::path::PathBuf;

pub mod value;

pub use value::{Dict, Value};

pub mod assert;
pub mod eval;
pub mod harness;
pub mod report;
pub mod sigil;

// ── Args passed to construct / execute ────────────────────────────────────────

/// Keyword arguments evaluated at an actor declaration site, plus the
/// directory containing the .ill file (used to resolve relative paths).
///
/// `vars` is the actor's declared member-variable list, in source order.
/// Each entry carries its name and its default value (already evaluated
/// against an empty scope, so defaults may not reference `self` or other
/// actors). `None` means the variable was declared without a default and
/// is therefore required. Actors that don't use declaration-site vars
/// (exec, container, …) can ignore it.
///
/// `cli_args` holds the `--arg KEY=VALUE` entries passed to `ill test`,
/// left as raw strings. Coercion to declared types is the consuming
/// actor's responsibility (see `args_actor`).
#[derive(Default)]
pub struct ConstructArgs {
    pub keyword: Dict,
    pub source_dir: PathBuf,
    pub vars: Vec<DeclaredVar>,
    pub cli_args: std::collections::BTreeMap<String, String>,
}

/// A single member variable declared on an actor, with its default already
/// evaluated to a runtime value (if it had one).
pub struct DeclaredVar {
    pub name: String,
    pub default: Option<Value>,
}

/// Arguments passed to a command invocation. Positional + keyword.
pub struct CommandArgs {
    pub positional: Vec<Value>,
    pub keyword: Dict,
}

impl ConstructArgs {
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

/// The result of a command. `Ok` populates `ok.*`; `Error` populates
/// `error.*`. `NotImplemented` is the default for Phase 6 actors that haven't
/// yet been wired to a runtime.
///
/// An `Error` carries the declared variant name and its fields. The harness
/// assembles the final `error` record as `{ type: :variant, <variant>: {fields} }`.
pub enum RunOutcome {
    Ok(Dict),
    Error {
        variant: &'static str,
        fields: Dict,
    },
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
    Construct(String),
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
            RuntimeError::Construct(msg) => write!(f, "construct failed: {msg}"),
            RuntimeError::Eval(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RuntimeError {}
