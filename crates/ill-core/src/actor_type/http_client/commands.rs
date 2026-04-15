use super::modes::READY;
use crate::actor_type::{ArgDef, Command, KeywordArgDef, Mode, ValueType};

// All HTTP verbs share the same shape: positional URL, optional headers/body/timeout.

macro_rules! http_verb {
    ($struct:ident, $name:literal, $static:ident) => {
        pub struct $struct;

        impl Command for $struct {
            fn name(&self) -> &'static str {
                $name
            }
            fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
                static VALID: &[&dyn Mode] = &[READY];
                VALID
            }
            fn positional(&self) -> &'static [ArgDef] {
                const POS: &[ArgDef] = &[ArgDef {
                    name: "url",
                    ty: ValueType::String,
                }];
                POS
            }
            fn keyword(&self) -> &'static [KeywordArgDef] {
                &[
                    KeywordArgDef {
                        name: "headers",
                        ty: ValueType::Unknown,
                        required: false,
                    },
                    KeywordArgDef {
                        name: "body",
                        ty: ValueType::Unknown,
                        required: false,
                    },
                    KeywordArgDef {
                        name: "timeout",
                        ty: ValueType::Number,
                        required: false,
                    },
                ]
            }
        }

        pub static $static: &dyn Command = &$struct;
    };
}

http_verb!(Get, "get", GET);
http_verb!(Post, "post", POST);
http_verb!(Put, "put", PUT);
http_verb!(Delete, "delete", DELETE);
