use super::modes::{CONNECTED, DISCONNECTED};
use crate::actor_type::{
    ArgDef, Command, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType,
};
use crate::define_outcome;
use crate::runtime::{Dict, Value};

// ── Outcome shapes ────────────────────────────────────────────────────────────
//
// `Connect` / `receive_publish` / `receive_disconnect` carry MQTT v5 user
// properties — a key→value map that has no first-class `ValueType`. Those
// outcome structs are hand-written so the `user_properties` field can be a
// `Dict` (declared as `Dynamic` in `OutcomeField`, the same way `pg_client`
// declares its `row` and `col` fields). Pure-scalar shapes still go through
// `define_outcome!`.

/// `connect` ok shape. `session_present` is the CONNACK flag;
/// `assigned_client_id` is the broker-assigned client identifier (empty
/// string when the client supplied its own); `user_properties` is the dict
/// of CONNACK user properties.
pub struct ConnectOk {
    pub session_present: bool,
    pub assigned_client_id: String,
    pub user_properties: Dict,
}

impl ConnectOk {
    pub const FIELDS: &'static [OutcomeField] = &[
        OutcomeField {
            name: "session_present",
            ty: ValueType::Bool,
        },
        OutcomeField {
            name: "assigned_client_id",
            ty: ValueType::String,
        },
        OutcomeField {
            name: "user_properties",
            ty: ValueType::Dynamic,
        },
    ];

    pub fn into_dict(self) -> Dict {
        let mut m = Dict::new();
        m.insert(
            "session_present".into(),
            Value::Bool(self.session_present),
        );
        m.insert(
            "assigned_client_id".into(),
            Value::String(self.assigned_client_id),
        );
        m.insert(
            "user_properties".into(),
            Value::Dict(self.user_properties),
        );
        m
    }
}

/// `receive_publish` ok shape. `payload` is `Value` (rather than `String` or
/// `Vec<u8>`) because MQTT publishes carry opaque bytes; the runtime decides
/// whether to surface them as `Value::String` or `Value::Bytes` based on the
/// payload format indicator. `qos` is the published QoS as observed by the
/// subscriber.
pub struct ReceivePublishOk {
    pub topic: String,
    pub payload: Value,
    pub qos: i64,
    pub user_properties: Dict,
}

impl ReceivePublishOk {
    pub const FIELDS: &'static [OutcomeField] = &[
        OutcomeField {
            name: "topic",
            ty: ValueType::String,
        },
        OutcomeField {
            name: "payload",
            ty: ValueType::Unknown,
        },
        OutcomeField {
            name: "qos",
            ty: ValueType::Number,
        },
        OutcomeField {
            name: "user_properties",
            ty: ValueType::Dynamic,
        },
    ];

    pub fn into_dict(self) -> Dict {
        let mut m = Dict::new();
        m.insert("topic".into(), Value::String(self.topic));
        m.insert("payload".into(), self.payload);
        m.insert("qos".into(), Value::Number(self.qos));
        m.insert(
            "user_properties".into(),
            Value::Dict(self.user_properties),
        );
        m
    }
}

/// `receive_disconnect` ok shape — the broker pushed a DISCONNECT frame.
/// `reason_code` is the v5 reason code (e.g. 142 for session takeover).
pub struct ReceiveDisconnectOk {
    pub reason_code: i64,
    pub user_properties: Dict,
}

impl ReceiveDisconnectOk {
    pub const FIELDS: &'static [OutcomeField] = &[
        OutcomeField {
            name: "reason_code",
            ty: ValueType::Number,
        },
        OutcomeField {
            name: "user_properties",
            ty: ValueType::Dynamic,
        },
    ];

    pub fn into_dict(self) -> Dict {
        let mut m = Dict::new();
        m.insert("reason_code".into(), Value::Number(self.reason_code));
        m.insert(
            "user_properties".into(),
            Value::Dict(self.user_properties),
        );
        m
    }
}

define_outcome! {
    /// `error.network.*` — transport-level failures shared with `pg_client`'s
    /// network reasons: `:host_unreachable`, `:connection_refused`,
    /// `:connection_lost`, `:timeout`, `:tls`, `:other`.
    pub NetworkError {
        reason: Atom,
    }
}

define_outcome! {
    /// `error.mqtt.*` — broker-side rejections and client-side protocol
    /// errors. `reason` is a per-command atom (see each command's docs);
    /// `reason_code` is the raw MQTT v5 byte (e.g. 135 for `:not_authorized`,
    /// 142 for session takeover) and is always populated when the broker
    /// supplied one — `0` when the rejection was generated client-side.
    pub MqttError {
        reason: Atom,
        reason_code: Number,
    }
}

define_outcome! {
    /// `error.timeout.*` — `receive_publish` / `receive_disconnect` exhausted
    /// their timeout budget. No fields beyond the variant tag.
    pub TimeoutError {}
}

// ── Per-command error variant lists ───────────────────────────────────────────
//
// MQTT v5 reason codes are command-specific (the same byte means different
// things in CONNACK, SUBACK, PUBACK, etc.) but the outcome *shape* is uniform:
// a `reason` atom and the raw `reason_code`. So every command shares the
// `MqttError` shape and the per-command atom set is enforced by the runtime,
// not the validator. `Publish_0` has no broker round-trip — only client-side
// rejections (e.g. invalid topic) come back through `error.mqtt`.

static CONNECT_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "mqtt",
        fields: MqttError::FIELDS,
    },
];

static SUBSCRIBE_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "mqtt",
        fields: MqttError::FIELDS,
    },
];

static PUBLISH_QOS0_ERROR_TYPES: &[ErrorTypeDef] = &[ErrorTypeDef {
    name: "mqtt",
    fields: MqttError::FIELDS,
}];

static PUBLISH_QOSN_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "mqtt",
        fields: MqttError::FIELDS,
    },
];

static RECEIVE_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "timeout",
        fields: TimeoutError::FIELDS,
    },
    ErrorTypeDef {
        name: "mqtt",
        fields: MqttError::FIELDS,
    },
];

// ── Commands ──────────────────────────────────────────────────────────────────

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
    fn ok_fields(&self) -> &'static [OutcomeField] {
        ConnectOk::FIELDS
    }
    fn error_types(&self) -> &'static [ErrorTypeDef] {
        CONNECT_ERROR_TYPES
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
            fn error_types(&self) -> &'static [ErrorTypeDef] {
                SUBSCRIBE_ERROR_TYPES
            }
        }
        pub static $static: &dyn Command = &$struct;
    };
}

macro_rules! publish_cmd {
    ($struct:ident, $name:literal, $static:ident, $errors:ident) => {
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
            fn error_types(&self) -> &'static [ErrorTypeDef] {
                $errors
            }
        }
        pub static $static: &dyn Command = &$struct;
    };
}

subscribe_cmd!(Subscribe0, "subscribe_0", SUBSCRIBE_0);
subscribe_cmd!(Subscribe1, "subscribe_1", SUBSCRIBE_1);
subscribe_cmd!(Subscribe2, "subscribe_2", SUBSCRIBE_2);

publish_cmd!(Publish0, "publish_0", PUBLISH_0, PUBLISH_QOS0_ERROR_TYPES);
publish_cmd!(Publish1, "publish_1", PUBLISH_1, PUBLISH_QOSN_ERROR_TYPES);
publish_cmd!(Publish2, "publish_2", PUBLISH_2, PUBLISH_QOSN_ERROR_TYPES);

// ── Receive: split into receive_publish / receive_disconnect ─────────────────
//
// Source syntax is `receive publish` / `receive disconnect`. The lowerer
// rewrites these into `receive_publish` / `receive_disconnect` command names
// before validation runs (see `lower::Lowerer::lower_command`), so the
// validator and harness see a fully-named command and can statically
// type-check the per-event `ok.*` fields.

pub struct ReceivePublish;
impl Command for ReceivePublish {
    fn name(&self) -> &'static str {
        "receive_publish"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[CONNECTED];
        VALID
    }
    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[KeywordArgDef {
            name: "timeout",
            ty: ValueType::Number,
            required: false,
        }]
    }
    fn ok_fields(&self) -> &'static [OutcomeField] {
        ReceivePublishOk::FIELDS
    }
    fn error_types(&self) -> &'static [ErrorTypeDef] {
        RECEIVE_ERROR_TYPES
    }
}

pub struct ReceiveDisconnect;
impl Command for ReceiveDisconnect {
    fn name(&self) -> &'static str {
        "receive_disconnect"
    }
    fn valid_in_modes(&self) -> &'static [&'static dyn Mode] {
        static VALID: &[&dyn Mode] = &[CONNECTED];
        VALID
    }
    /// A broker DISCONNECT terminates the session; once observed, the actor
    /// is back in `disconnected`.
    fn transitions_to(&self) -> Option<&'static dyn Mode> {
        Some(DISCONNECTED)
    }
    fn keyword(&self) -> &'static [KeywordArgDef] {
        &[KeywordArgDef {
            name: "timeout",
            ty: ValueType::Number,
            required: false,
        }]
    }
    fn ok_fields(&self) -> &'static [OutcomeField] {
        ReceiveDisconnectOk::FIELDS
    }
    fn error_types(&self) -> &'static [ErrorTypeDef] {
        RECEIVE_ERROR_TYPES
    }
}

pub static CONNECT: &dyn Command = &Connect;
pub static DISCONNECT: &dyn Command = &Disconnect;
pub static RECEIVE_PUBLISH: &dyn Command = &ReceivePublish;
pub static RECEIVE_DISCONNECT: &dyn Command = &ReceiveDisconnect;
