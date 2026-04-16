// Actor type substrate for iLL.
//
// Actor types are pluggable via trait objects. Phase 4 (validation) consumes
// `&'static dyn ActorType`; Phase 5 will add runtime execution as a sibling
// trait. Keeping everything behind `dyn` means nothing outside an actor's own
// module needs to match on actor identity.

use std::any::Any;

pub mod args_actor;
pub mod container;
pub mod http_client;
pub mod mqtt_client;
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
    Bool,
    Atom,
    Bytes,
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
// Declare the named fields available on `ok.*` and `error.*` after a command.
// Commands that don't override `ok_fields` / `error_fields` return empty slices
// for ok and `DEFAULT_ERROR_FIELDS` for error.

pub struct OutcomeField {
    pub name: &'static str,
    pub ty: ValueType,
}

/// Fields present on `error.*` for every command that can fail with a
/// structured error. Commands may override `error_fields` to add more.
pub static DEFAULT_ERROR_FIELDS: &[OutcomeField] = &[
    OutcomeField {
        name: "code",
        ty: ValueType::Number,
    },
    OutcomeField {
        name: "message",
        ty: ValueType::String,
    },
];

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

    /// Named fields available on `error.*` after a failed execution.
    /// Defaults to `DEFAULT_ERROR_FIELDS` (`code`, `message`).
    ///
    /// Same Phase 5 caveat as `ok_fields`: the runtime error type must match
    /// what is declared here.
    fn error_fields(&self) -> &'static [OutcomeField] {
        DEFAULT_ERROR_FIELDS
    }

    // Phase 5 will add something like:
    //   fn execute(&self, instance: &mut dyn ActorInstance, args: &Args)
    //     -> Result<Value, RuntimeError>;
}

// ── Actor types ────────────────────────────────────────────────────────────────

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

    fn mode(&self, name: &str) -> Option<&'static dyn Mode> {
        self.modes().iter().copied().find(|m| m.name() == name)
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
