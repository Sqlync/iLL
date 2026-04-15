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
    Json,
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

    /// Shape of `ok.*` after a successful execution. `Unknown` is fine for
    /// commands where the result type hasn't been designed yet.
    fn result_type(&self) -> ValueType {
        ValueType::Unknown
    }

    /// Shape of `error.*` after a failed execution.
    fn error_type(&self) -> ValueType {
        ValueType::Unknown
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
