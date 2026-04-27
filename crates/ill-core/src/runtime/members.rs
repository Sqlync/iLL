// Storage for an actor's declared member variables. Each declared name
// is a slot that may or may not hold a value; absence lives in the slot
// (`Option<Value>`) so `Value` itself never needs an "unset" sentinel.
//
// `BTreeMap` rather than `IndexMap` because positional access on member
// vars isn't a supported operation — users address them by name (`db.port`,
// `self.user_id`). Sorted iteration keeps debug output and `assigned_view`
// snapshots deterministic without relying on declaration order.

use std::collections::BTreeMap;

use super::{DeclaredVar, Dict, Value};

pub struct Members {
    slots: BTreeMap<String, Option<Value>>,
}

/// Returned by `Members::set` when the caller tries to assign to a name
/// that wasn't declared on the actor. Future user-driven writes via
/// `self.<name> = ...` will surface this as a runtime error.
#[derive(Debug, PartialEq)]
pub struct Undeclared;

impl Members {
    pub fn from_declarations(vars: &[DeclaredVar]) -> Self {
        let mut slots = BTreeMap::new();
        for v in vars {
            slots.insert(v.name.clone(), v.default.clone());
        }
        Self { slots }
    }

    /// Returns `Some(&v)` only when the slot is both declared and assigned.
    /// Use `is_declared` to distinguish "declared but unset" from "never
    /// declared."
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.slots.get(name).and_then(|s| s.as_ref())
    }

    pub fn is_declared(&self, name: &str) -> bool {
        self.slots.contains_key(name)
    }

    /// Assign a value to a declared slot. `Err(Undeclared)` if the name
    /// wasn't part of the actor's declaration.
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), Undeclared> {
        match self.slots.get_mut(name) {
            Some(slot) => {
                *slot = Some(value);
                Ok(())
            }
            None => Err(Undeclared),
        }
    }

    /// Snapshot of all currently-assigned slots as a `Dict`, for the
    /// harness's `self_view` binding. Unassigned slots are omitted —
    /// reads of an unassigned-but-declared var surface as a "no field"
    /// lookup error at the eval layer.
    pub fn assigned_view(&self) -> Dict {
        let mut out = Dict::new();
        for (name, slot) in &self.slots {
            if let Some(v) = slot {
                out.insert(name.clone(), v.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(name: &str, default: Option<Value>) -> DeclaredVar {
        DeclaredVar {
            name: name.into(),
            default,
        }
    }

    #[test]
    fn from_declarations_seeds_defaults_and_keeps_undefaulted_slots() {
        let m = Members::from_declarations(&[
            declared("port", Some(Value::Number(8080))),
            declared("name", None),
        ]);
        assert_eq!(m.get("port"), Some(&Value::Number(8080)));
        assert_eq!(m.get("name"), None);
        assert!(m.is_declared("name"), "undefaulted vars stay declared");
        assert!(!m.is_declared("missing"));
    }

    #[test]
    fn set_assigns_to_declared_slot() {
        let mut m = Members::from_declarations(&[declared("port", None)]);
        assert_eq!(m.set("port", Value::Number(5432)), Ok(()));
        assert_eq!(m.get("port"), Some(&Value::Number(5432)));
    }

    #[test]
    fn set_rejects_undeclared() {
        let mut m = Members::from_declarations(&[declared("port", None)]);
        assert_eq!(m.set("nope", Value::Number(1)), Err(Undeclared));
    }

    #[test]
    fn assigned_view_omits_unset_slots() {
        let mut m = Members::from_declarations(&[
            declared("port", Some(Value::Number(8080))),
            declared("host", None),
        ]);
        let view = m.assigned_view();
        assert_eq!(view.get("port"), Some(&Value::Number(8080)));
        assert!(!view.contains_key("host"));

        m.set("host", Value::String("localhost".into())).unwrap();
        let view = m.assigned_view();
        assert_eq!(view.get("host"), Some(&Value::String("localhost".into())));
    }
}
