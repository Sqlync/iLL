//! Typed outcome shapes.
//!
//! `define_outcome!` generates a struct, a `FIELDS` constant, and an
//! `into_record` method from a single declaration. Actors build the typed
//! struct and call `into_record()` at the `RunOutcome` boundary; the `FIELDS`
//! constant is what commands return from `ok_fields` and what error variant
//! descriptors reference. Using struct literal syntax gives the compiler full
//! coverage on field names and types, so declared schema and constructed
//! value can't drift.

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
/// ```
#[macro_export]
macro_rules! define_outcome {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident {
            $($(#[$fmeta:meta])* $field:ident : $kind:ident),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $($(#[$fmeta])* pub $field: $crate::__outcome_rust_type!($kind),)*
        }

        impl $name {
            pub const FIELDS: &'static [$crate::actor_type::OutcomeField] = &[
                $($crate::actor_type::OutcomeField {
                    name: stringify!($field),
                    ty: $crate::actor_type::ValueType::$kind,
                },)*
            ];

            pub fn into_record(self) -> $crate::runtime::Record {
                let mut m = $crate::runtime::Record::new();
                $(m.insert(
                    stringify!($field).into(),
                    $crate::__outcome_wrap!($kind, self.$field),
                );)*
                m
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::actor_type::ValueType;
    use crate::runtime::Value;

    define_outcome! {
        pub SampleOutcome {
            code: Number,
            message: String,
        }
    }

    #[test]
    fn fields_match_declaration() {
        let fields = SampleOutcome::FIELDS;
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "code");
        assert_eq!(fields[0].ty, ValueType::Number);
        assert_eq!(fields[1].name, "message");
        assert_eq!(fields[1].ty, ValueType::String);
    }

    #[test]
    fn into_record_produces_correct_values() {
        let s = SampleOutcome {
            code: 7,
            message: "boom".into(),
        };
        let rec = s.into_record();
        assert_eq!(rec.get("code"), Some(&Value::Number(7)));
        assert_eq!(rec.get("message"), Some(&Value::String("boom".into())));
    }
}
