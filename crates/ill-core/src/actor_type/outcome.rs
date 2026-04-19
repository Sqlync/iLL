//! Typed outcome shapes.
//!
//! `define_outcome!` generates a struct, a `FIELDS` constant, and an
//! `into_record` method from a single declaration. Actors build the typed
//! struct and call `into_record()` at the `RunOutcome` boundary; the `FIELDS`
//! constant is what commands return from `ok_fields` / `error_fields`.
//! Using struct literal syntax gives the compiler full coverage on field
//! names and types, so declared schema and constructed value can't drift.

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
/// // struct RunOk { pub pid: i64 }
/// // RunOk::FIELDS: &'static [OutcomeField]
/// // RunOk::into_record(self) -> BTreeMap<String, Value>
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

            pub fn into_record(
                self,
            ) -> ::std::collections::BTreeMap<::std::string::String, $crate::runtime::Value> {
                let mut m = ::std::collections::BTreeMap::new();
                $(m.insert(
                    stringify!($field).into(),
                    $crate::__outcome_wrap!($kind, self.$field),
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
