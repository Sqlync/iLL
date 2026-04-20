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
                name: "user",
                ty: ValueType::String,
                required: true,
            },
            KeywordArgDef {
                name: "password",
                ty: ValueType::String,
                required: false,
            },
            KeywordArgDef {
                name: "database",
                ty: ValueType::String,
                required: true,
            },
        ]
    }
}

pub struct Query;

impl Command for Query {
    fn name(&self) -> &'static str {
        "query"
    }

    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[CONNECTED];
        VALID
    }

    fn positional(&self) -> &'static [ArgDef] {
        const POSITIONAL: &[ArgDef] = &[ArgDef {
            name: "sql",
            ty: ValueType::String,
        }];
        POSITIONAL
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
                name: "rows",
                ty: ValueType::Dynamic,
                fields: &[],
            },
            OutcomeField {
                name: "count",
                ty: ValueType::Number,
                fields: &[],
            },
        ];
        FIELDS
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
}

pub static CONNECT: &dyn Command = &Connect;
pub static QUERY: &dyn Command = &Query;
pub static DISCONNECT: &dyn Command = &Disconnect;
