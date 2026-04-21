use super::modes::{CONNECTED, DISCONNECTED};
use crate::actor_type::{
    ArgDef, Command, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType,
};
use crate::define_outcome;

// ── Outcome shapes ────────────────────────────────────────────────────────────
//
// `Connect` and `Query` declare structured results that assertions address as
// `ok.is_connected`, `ok.row[0]`, `ok.col["name"]`, `ok.cell[i, j]`,
// `ok.row_count`, `ok.col_count`. Row / column / cell structures carry
// heterogeneous cell values and go through as `Dynamic` — the validator can't
// reason about their inner shape, but `assert ok.row[0] == [1, "alice"]` is
// resolved by the runtime's indexing.

define_outcome! {
    /// Result of `pg_client.connect`. `is_connected` is always `true` on the
    /// ok branch — examples use `assert ok.is_connected` as a smoke test.
    pub ConnectOk {
        is_connected: Bool,
    }
}

define_outcome! {
    /// `error.network.*` — transport-level failures shared by `connect` and
    /// `query`. `reason`:
    ///   - connect: `:host_unreachable`, `:connection_refused`, `:timeout`, `:tls`
    ///   - query:   `:connection_lost`
    pub NetworkError {
        reason: Atom,
    }
}

define_outcome! {
    /// `error.connect.*` — authentication and database-selection failures.
    /// Atoms: `:authentication_failed`, `:bad_database`.
    pub ConnectError {
        reason: Atom,
    }
}

define_outcome! {
    /// `error.query.*` — failures returned by the server in response to a
    /// well-formed SQL roundtrip. `reason` atoms include `:syntax_error`,
    /// `:constraint_violation`, `:timeout`, and `:other` as a catch-all.
    /// `sqlstate` is the raw Postgres SQLSTATE code (5-char string) when the
    /// server returned one; empty when the error was generated client-side.
    /// `message` is the server-side error message.
    pub QueryError {
        reason: Atom,
        sqlstate: String,
        message: String,
    }
}

static CONNECT_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "connect",
        fields: ConnectError::FIELDS,
    },
];

static QUERY_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "query",
        fields: QueryError::FIELDS,
    },
];

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
            KeywordArgDef {
                name: "timeout",
                ty: ValueType::Number,
                required: false,
            },
        ]
    }

    fn ok_fields(&self) -> &'static [OutcomeField] {
        ConnectOk::FIELDS
    }

    fn error_types(&self) -> &'static [ErrorTypeDef] {
        CONNECT_ERROR_TYPES
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
        // Declared `Dynamic` because the inner structure is row-shaped and
        // cell-typed at runtime; the validator does not reason about cell
        // types. Assertions like `ok.row[0] == [1, "alice"]` resolve through
        // eval's indexing over the Array/Dict the runtime produces.
        const FIELDS: &[OutcomeField] = &[
            OutcomeField {
                name: "row",
                ty: ValueType::Dynamic,
            },
            OutcomeField {
                name: "col",
                ty: ValueType::Dynamic,
            },
            OutcomeField {
                name: "cell",
                ty: ValueType::Dynamic,
            },
            OutcomeField {
                name: "row_count",
                ty: ValueType::Number,
            },
            OutcomeField {
                name: "col_count",
                ty: ValueType::Number,
            },
        ];
        FIELDS
    }

    fn error_types(&self) -> &'static [ErrorTypeDef] {
        QUERY_ERROR_TYPES
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
