use super::modes::{CONNECTED, DISCONNECTED};
use crate::actor_type::{ArgDef, Command, KeywordArgDef, Mode, OutcomeField, ValueType};

pub struct Connect;
impl Command for Connect {
    fn name(&self) -> &'static str {
        "connect"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[DISCONNECTED];
        VALID
    }
    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(CONNECTED)
    }
    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[
            KeywordArgDef {
                name: "host",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "port",
                ty: ValueType::Number,
                required: false,
            },
            KeywordArgDef {
                name: "username",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "password",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "client_id",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "clean_start",
                ty: ValueType::Bool,
                required: false,
            },
            KeywordArgDef {
                name: "keep_alive",
                ty: ValueType::Number,
                required: false,
            },
            KeywordArgDef {
                name: "user_properties",
                ty: ValueType::Unknown,
                required: false,
            },
        ]
    }
}

pub struct Disconnect;
impl Command for Disconnect {
    fn name(&self) -> &'static str {
        "disconnect"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[CONNECTED];
        VALID
    }
    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(DISCONNECTED)
    }
    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[
            KeywordArgDef {
                name: "reason_code",
                ty: ValueType::Number,
                required: false,
            },
            KeywordArgDef {
                name: "user_properties",
                ty: ValueType::Unknown,
                required: false,
            },
        ]
    }
}

macro_rules! subscribe_cmd {
    ($struct:ident, $name:literal, $static:ident) => {
        pub struct $struct;
        impl Command for $struct {
            fn name(&self) -> &'static str {
                $name
            }
            fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
                static VALID: &[&dyn Mode] = &[CONNECTED];
                VALID
            }
            fn positional(&self) -> &'static [ArgDef] {
                const POS: &[ArgDef] = &[ArgDef {
                    name: "topic",
                    ty: ValueType::String,
                }];
                POS
            }
            fn keyword(&self) -> &'static [KeywordArgDef] {
                &[KeywordArgDef {
                    name: "user_properties",
                    ty: ValueType::Unknown,
                    required: false,
                }]
            }
        }
        pub static $static: &dyn Command = &$struct;
    };
}

macro_rules! publish_cmd {
    ($struct:ident, $name:literal, $static:ident) => {
        pub struct $struct;
        impl Command for $struct {
            fn name(&self) -> &'static str {
                $name
            }
            fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
                static VALID: &[&dyn Mode] = &[CONNECTED];
                VALID
            }
            fn positional(&self) -> &'static [ArgDef] {
                const POS: &[ArgDef] = &[
                    ArgDef {
                        name: "topic",
                        ty: ValueType::String,
                    },
                    ArgDef {
                        name: "payload",
                        ty: ValueType::Unknown,
                    },
                ];
                POS
            }
            fn keyword(&self) -> &'static [KeywordArgDef] {
                &[KeywordArgDef {
                    name: "user_properties",
                    ty: ValueType::Unknown,
                    required: false,
                }]
            }
        }
        pub static $static: &dyn Command = &$struct;
    };
}

subscribe_cmd!(Subscribe0, "subscribe_0", SUBSCRIBE_0);
subscribe_cmd!(Subscribe1, "subscribe_1", SUBSCRIBE_1);
subscribe_cmd!(Subscribe2, "subscribe_2", SUBSCRIBE_2);

publish_cmd!(Publish0, "publish_0", PUBLISH_0);
publish_cmd!(Publish1, "publish_1", PUBLISH_1);
publish_cmd!(Publish2, "publish_2", PUBLISH_2);

pub struct Receive;
impl Command for Receive {
    fn name(&self) -> &'static str {
        "receive"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[CONNECTED];
        VALID
    }
    fn positional(&self) -> &'static [ArgDef] {
        // `receive publish` — the event kind is an identifier, currently
        // positional. Typed as Unknown since it's an identifier, not a value.
        const POS: &[ArgDef] = &[ArgDef {
            name: "event",
            ty: ValueType::Unknown,
        }];
        POS
    }
    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[KeywordArgDef {
            name: "timeout",
            ty: ValueType::Number,
            required: false,
        }]
    }

    fn ok_fields(&self) -> &'static [OutcomeField] {
        const FIELDS: &[OutcomeField] = &[
            OutcomeField {
                name: "topic",
                ty: ValueType::String,
            },
            OutcomeField {
                name: "payload",
                ty: ValueType::Unknown,
            },
        ];
        FIELDS
    }
}

pub static CONNECT: &dyn Command = &Connect;
pub static DISCONNECT: &dyn Command = &Disconnect;
pub static RECEIVE: &dyn Command = &Receive;
