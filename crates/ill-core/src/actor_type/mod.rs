// Actor type substrate for iLL.
//
// Actor types are pluggable via trait objects. Phase 4 (validation) consumes
// `&'static dyn ActorType`; Phase 5 adds runtime execution via
// `ActorType::construct` and `ActorInstance::execute`. Keeping everything
// behind `dyn` means nothing outside an actor's own module needs to match
// on actor identity.
//
// `Command` carries only static metadata (name, modes, arg/outcome shapes)
// consumed by the validator. Dispatch lives on `ActorInstance::execute`:
// `self` is already the concrete instance type inside the impl, so no
// downcast is needed — the vtable does that work.

use std::any::Any;

use crate::ast::Expr;
use crate::runtime::{CommandArgs, ConstructArgs, Dict, RunOutcome, RuntimeError, TeardownOutcome};

pub mod args_actor;
pub mod container;
pub mod exec;
pub mod http_client;
pub mod mqtt_client;
pub mod outcome;
pub mod pg_client;

// ── Modes ──────────────────────────────────────────────────────────────────────
//
// Each mode is a zero-sized unit struct with a `'static` singleton. Identity is
// compared via `TypeId`, so a typo at an impl site becomes "unknown mode" at
// registry-build time.

pub trait Mode: Any + Send + Sync {
    fn name(&self) -> &'static str;
    fn actor_type(&self) -> &'static str;
}

/// Compare two modes by concrete type identity.
///
/// Prefer this over any `PartialEq` impl — `dyn Mode` is unsized, so writing
/// `*a == *b` doesn't work ergonomically. Passing `&dyn Mode` keeps callers
/// simple.
pub fn same_mode(a: &dyn Mode, b: &dyn Mode) -> bool {
    (a as &dyn Any).type_id() == (b as &dyn Any).type_id()
}

// ── Value types ────────────────────────────────────────────────────────────────
//
// The type language used for command arg/result checking. Kept deliberately
// narrow for Phase 4; expand as validation gains teeth. Named `ValueType` to
// avoid colliding with `std::any::TypeId`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    String,
    Number,
    Float,
    Bool,
    Atom,
    Bytes,
    /// A key→value map. Field shape is not statically declared, but the
    /// type is — `Dict` participates in indexing assertions like
    /// `ok.user_properties["env"]`.
    Dict,
    /// A structured runtime value produced by parsing (json, protobuf, etc.).
    /// The shape is not statically known to the validator.
    Dynamic,
    /// Escape hatch for expressions whose type can't be resolved yet.
    /// Prefer this over inventing a wrong type.
    Unknown,
}

// ── Argument descriptors ───────────────────────────────────────────────────────

pub struct ArgDef {
    pub name: &'static str,
    pub ty: ValueType,
}

pub struct KeywordArgDef {
    pub name: &'static str,
    pub ty: ValueType,
    pub required: bool,
}

// ── Outcome field descriptors ──────────────────────────────────────────────────
//
// Declare the named fields available on `ok.*` after a successful command.
// Error outcomes use `ErrorTypeDef` instead — each command declares a list of
// possible error variants, each with its own fields. The harness always
// surfaces `error.type` (atom naming the variant) so tests can report on an
// unexpected variant without knowing its schema.

pub struct OutcomeField {
    pub name: &'static str,
    pub ty: ValueType,
}

/// One variant in a command's set of possible error outcomes. Addressed at
/// runtime as `error.<name>.<field>` and in `.ill` source via the same path.
pub struct ErrorTypeDef {
    pub name: &'static str,
    pub fields: &'static [OutcomeField],
}

// ── Commands ───────────────────────────────────────────────────────────────────

pub trait Command: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    /// Modes in which this command may be invoked.
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode];

    /// Mode to transition to after successful execution, if any.
    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        None
    }

    fn positional(&self) -> &'static [ArgDef] {
        &[]
    }

    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[]
    }

    /// Named fields available on `ok.*` after successful execution.
    ///
    /// These are validated statically against `ok.*` references in `let`
    /// bindings and `assert` statements. Phase 5 will introduce a runtime
    /// result type for each command — that type must expose the same fields
    /// declared here. There is currently no automated check that the two stay
    /// in sync; when Phase 5 result types are defined, revisit whether to
    /// derive these from the runtime struct or add a registry-level assertion.
    fn ok_fields(&self) -> &'static [OutcomeField] {
        &[]
    }

    /// Possible error variants this command can produce. Each variant's
    /// fields are addressable as `error.<variant_name>.<field>`. Defaults to
    /// empty — `error.type` is always available regardless of declared
    /// variants.
    fn error_types(&self) -> &'static [ErrorTypeDef] {
        &[]
    }
}

// ── Actor types ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait ActorType: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    /// The mode a freshly-declared actor starts in.
    fn initial_mode(&self) -> &'static dyn Mode;

    fn modes(&self) -> &'static [&'static dyn Mode];

    fn commands(&self) -> &'static [&'static dyn Command];

    /// Keyword args accepted at `actor foo = type, k: v` declaration sites.
    /// Positional args on declarations aren't currently part of the grammar.
    fn constructor_keyword(&self) -> &'static [KeywordArgDef] {
        &[]
    }

    fn command(&self, name: &str) -> Option<&'static dyn Command> {
        self.commands().iter().copied().find(|c| c.name() == name)
    }

    /// Resolve a source-form command spelling to a concrete `Command`.
    ///
    /// Returns the resolved command and how many leading positional args the
    /// resolution consumed (those are part of the source spelling, not the
    /// command's argument list — the validator and harness skip them).
    ///
    /// Default behaviour: a plain name lookup with zero positionals consumed.
    /// Actor types that have multi-token source forms (e.g. mqtt's
    /// `receive publish`) override this to fuse the leading event ident into
    /// the command name without a separate AST-rewrite pass.
    ///
    /// **Contract:** the returned `consumed` count must satisfy
    /// `consumed <= positional.len()`. Validators and the harness slice
    /// `positional[consumed..]`; an over-count panics. Override impls should
    /// only consume positionals they have positively identified as part of
    /// the source spelling.
    fn resolve_command(
        &self,
        name: &str,
        _positional: &[Expr],
    ) -> Option<(&'static dyn Command, usize)> {
        self.command(name).map(|c| (c, 0))
    }

    fn mode(&self, name: &str) -> Option<&'static dyn Mode> {
        self.modes().iter().copied().find(|m| m.name() == name)
    }

    /// Construct a runtime instance from declaration-site kwargs.
    ///
    /// Async because some actor types (e.g. `container`) do real I/O at
    /// declaration time — pulling images or building from a Dockerfile —
    /// so that failures surface as a construct error rather than buried in
    /// the first command. Default returns `ActorNotImplemented` — Phase 6
    /// actors opt in by overriding.
    async fn construct(
        &self,
        _args: &ConstructArgs,
    ) -> Result<Box<dyn ActorInstance>, RuntimeError> {
        Err(RuntimeError::ActorNotImplemented(self.name()))
    }
}

/// Format an "unknown command" diagnostic. When the source spelling has a
/// leading bare-ident positional (e.g. mqtt's `receive bogus`), name it
/// explicitly — that's almost always the real source of the problem, not
/// the bare command keyword. Shared by the validator and the harness so
/// both layers report the same message.
pub fn unknown_command_message(actor_type: &str, name: &str, positional: &[Expr]) -> String {
    if let Some(Expr::Ident(event)) = positional.first() {
        format!(
            "unknown command `{name} {}` for actor type `{actor_type}`",
            event.name
        )
    } else {
        format!("unknown command `{name}` for actor type `{actor_type}`")
    }
}

// ── Actor instances (runtime) ─────────────────────────────────────────────────
//
// A live actor. Created by `ActorType::construct`, dispatched against by
// `execute`, torn down by `teardown`. `teardown` is idempotent — the harness
// calls it on the happy path (success or command failure). On panic, each
// actor's own `Drop` impl is responsible for emergency resource cleanup
// (e.g. exec's `KillOnDrop`).

#[async_trait::async_trait]
pub trait ActorInstance: Send {
    fn type_name(&self) -> &'static str;

    /// Run a validated command against this instance. `cmd` is the command's
    /// static name (as returned by `Command::name`), so the default
    /// `NotImplemented` arm can pass it through without allocation. Default
    /// returns `NotImplemented` — Phase 6 actors opt in by overriding this.
    async fn execute(&mut self, cmd: &'static str, _args: &CommandArgs) -> RunOutcome {
        RunOutcome::NotImplemented {
            actor: self.type_name(),
            cmd,
        }
    }

    /// Release any resources held by the instance. Called in reverse
    /// construction order. Idempotent — safe to call more than once.
    async fn teardown(&mut self) -> TeardownOutcome;

    /// Snapshot of the actor's member variables, used to bind `self` inside
    /// `as` blocks. The harness calls this before each statement, so
    /// mutations made by one command are visible to the asserts that follow.
    /// Default is `None` — actors without user-visible state opt out and
    /// `self.*` accesses error as undefined.
    fn self_view(&self) -> Option<Dict> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ModeA;
    struct ModeB;

    impl Mode for ModeA {
        fn name(&self) -> &'static str {
            "a"
        }
        fn actor_type(&self) -> &'static str {
            "test"
        }
    }
    impl Mode for ModeB {
        fn name(&self) -> &'static str {
            "b"
        }
        fn actor_type(&self) -> &'static str {
            "test"
        }
    }

    #[test]
    fn same_mode_by_type_identity() {
        let a1: &dyn Mode = &ModeA;
        let a2: &dyn Mode = &ModeA;
        let b: &dyn Mode = &ModeB;
        assert!(same_mode(a1, a2));
        assert!(!same_mode(a1, b));
    }
}
