//! Typed outcome shapes.
//!
//! `define_outcome!` generates a struct, a `FIELDS` constant, and an
//! `into_record` method from a single declaration. Actors build the typed
//! struct and call `into_record()` at the `RunOutcome` boundary; the `FIELDS`
//! constant is what commands return from `ok_fields` / `error_fields`.
//! Using struct literal syntax gives the compiler full coverage on field
//! names and types, so declared schema and constructed value can't drift.
//!
//! Nested records are expressed with `Record(Type)`, where `Type` is another
//! `define_outcome!`-generated struct. The outer struct's `FIELDS` embeds the
//! inner struct's `FIELDS`, and `into_record()` wraps the inner value with
//! `Value::Record`.

// Helper macros — `#[macro_export]` so `define_outcome!` can reference them
// via `$crate::` when invoked from any module.

#[doc(hidden)]
#[macro_export]
macro_rules! __outcome_rust_type {
    (Number) => { i64 };
    (String) => { ::std::string::String };
    (Bool)   => { bool };
    (Atom)   => { ::std::string::String };
    (Bytes)  => { ::std::vec::Vec<u8> };
    (Record, $t:ty) => { $t };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __outcome_value_type {
    (Number) => {
        $crate::actor_type::ValueType::Number
    };
    (String) => {
        $crate::actor_type::ValueType::String
    };
    (Bool) => {
        $crate::actor_type::ValueType::Bool
    };
    (Atom) => {
        $crate::actor_type::ValueType::Atom
    };
    (Bytes) => {
        $crate::actor_type::ValueType::Bytes
    };
    (Record, $t:ty) => {
        $crate::actor_type::ValueType::Record
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __outcome_nested_fields {
    (Number) => {
        &[]
    };
    (String) => {
        &[]
    };
    (Bool) => {
        &[]
    };
    (Atom) => {
        &[]
    };
    (Bytes) => {
        &[]
    };
    (Record, $t:ty) => {
        <$t>::FIELDS
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __outcome_wrap {
    (Number, $e:expr) => {
        $crate::runtime::Value::Number($e)
    };
    (String, $e:expr) => {
        $crate::runtime::Value::String($e)
    };
    (Bool,   $e:expr) => {
        $crate::runtime::Value::Bool($e)
    };
    (Atom,   $e:expr) => {
        $crate::runtime::Value::Atom($e)
    };
    (Bytes,  $e:expr) => {
        $crate::runtime::Value::Bytes($e)
    };
    (Record, $t:ty, $e:expr) => {
        $crate::runtime::Value::Record($e.into_record())
    };
}

/// Define an outcome type: one declaration yields the struct, the `FIELDS`
/// metadata, and an `into_record` conversion to the scope-visible map.
///
/// ```ignore
/// define_outcome! {
///     pub RunOk {
///         pid: Number,
///     }
/// }
///
/// // Nested record — `exec` is another outcome struct.
/// define_outcome! { pub ExecErrorDetails { reason: Atom } }
/// define_outcome! {
///     pub RunError {
///         code: Number,
///         message: String,
///         exec: Record(ExecErrorDetails),
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_outcome {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident {
            $($(#[$fmeta:meta])* $field:ident : $kind:ident $(($inner:ty))? ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $($(#[$fmeta])* pub $field: $crate::__outcome_rust_type!($kind $(, $inner)?),)*
        }

        impl $name {
            pub const FIELDS: &'static [$crate::actor_type::OutcomeField] = &[
                $($crate::actor_type::OutcomeField {
                    name: stringify!($field),
                    ty: $crate::__outcome_value_type!($kind $(, $inner)?),
                    fields: $crate::__outcome_nested_fields!($kind $(, $inner)?),
                },)*
            ];

            pub fn into_record(
                self,
            ) -> ::std::collections::BTreeMap<::std::string::String, $crate::runtime::Value> {
                let mut m = ::std::collections::BTreeMap::new();
                $(m.insert(
                    stringify!($field).into(),
                    $crate::__outcome_wrap!($kind $(, $inner)?, self.$field),
                );)*
                m
            }
        }
    };
}

define_outcome! {
    /// Default error shape. Commands without richer error data use this —
    /// `code` is a numeric signal, `message` is human-readable detail.
    pub StandardError {
        code: Number,
        message: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor_type::ValueType;
    use crate::runtime::Value;

    define_outcome! {
        pub TestInner {
            reason: Atom,
        }
    }

    define_outcome! {
        pub TestOuter {
            code: Number,
            message: String,
            detail: Record(TestInner),
        }
    }

    #[test]
    fn leaf_fields_match_declaration() {
        let fields = StandardError::FIELDS;
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "code");
        assert_eq!(fields[0].ty, ValueType::Number);
        assert!(fields[0].fields.is_empty());
        assert_eq!(fields[1].name, "message");
        assert_eq!(fields[1].ty, ValueType::String);
        assert!(fields[1].fields.is_empty());
    }

    #[test]
    fn leaf_into_record_produces_correct_values() {
        let err = StandardError {
            code: 7,
            message: "boom".into(),
        };
        let rec = err.into_record();
        assert_eq!(rec.get("code"), Some(&Value::Number(7)));
        assert_eq!(rec.get("message"), Some(&Value::String("boom".into())));
    }

    #[test]
    fn nested_fields_embed_inner_schema() {
        let outer = TestOuter::FIELDS;
        assert_eq!(outer.len(), 3);
        let detail = &outer[2];
        assert_eq!(detail.name, "detail");
        assert_eq!(detail.ty, ValueType::Record);
        assert_eq!(detail.fields.len(), 1);
        assert_eq!(detail.fields[0].name, "reason");
        assert_eq!(detail.fields[0].ty, ValueType::Atom);
    }

    #[test]
    fn nested_into_record_wraps_inner() {
        let outer = TestOuter {
            code: 1,
            message: "m".into(),
            detail: TestInner {
                reason: "nope".into(),
            },
        };
        let rec = outer.into_record();
        match rec.get("detail") {
            Some(Value::Record(inner)) => {
                assert_eq!(inner.get("reason"), Some(&Value::Atom("nope".into())));
            }
            other => panic!("expected Value::Record, got {other:?}"),
        }
    }
}
