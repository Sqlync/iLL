use super::modes::{CONNECTED, DISCONNECTED};
use crate::actor_type::{
    ArgDef, Command, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType,
};
use crate::define_outcome;

// ── Outcome shapes ────────────────────────────────────────────────────────────
//
// MQTT payloads are wire-bytes, so `payload` is `Bytes`; `user_properties` is
// a v5 key→value map (`Dict`). Byte/string coercion in payload assertions is
// the assertion layer's job.

define_outcome! {
    /// `connect` ok shape. `session_present` is the CONNACK flag;
    /// `assigned_client_id` is the broker-assigned client identifier (empty
    /// string when the client supplied its own); `user_properties` is the
    /// dict of CONNACK user properties.
    pub ConnectOk {
        session_present: Bool,
        assigned_client_id: String,
        user_properties: Dict,
    }
}

define_outcome! {
    /// `receive_publish` ok shape. `payload` is the wire bytes of the
    /// publish; `qos` is the published QoS as observed by the subscriber.
    pub ReceivePublishOk {
        topic: String,
        payload: Bytes,
        qos: Number,
        user_properties: Dict,
    }
}

define_outcome! {
    /// `receive_disconnect` ok shape — the broker pushed a DISCONNECT frame.
    /// `reason_code` is the v5 reason code (e.g. 142 for session takeover).
    pub ReceiveDisconnectOk {
        reason_code: Number,
        user_properties: Dict,
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
    /// errors. `reason` is a per-command atom (see each command's docs).
    ///
    /// `reason_code` is the raw MQTT v5 byte (e.g. 135 for
    /// `:not_authorized`, 142 for session takeover). When the rejection is
    /// generated client-side (no broker round-trip) the code is set to `-1`
    /// — distinct from `0`, which is a valid v5 success code (UNSPECIFIED)
    /// and could otherwise be confused with "the broker said OK."
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

/// Most commands that traverse the wire can fail either way: the connection
/// itself drops (`network`) or the broker rejects with a v5 reason code
/// (`mqtt`). Used by `connect`, `subscribe_*`, and `publish_1` / `publish_2`.
static NETWORK_AND_MQTT_ERROR_TYPES: &[ErrorTypeDef] = &[
    ErrorTypeDef {
        name: "network",
        fields: NetworkError::FIELDS,
    },
    ErrorTypeDef {
        name: "mqtt",
        fields: MqttError::FIELDS,
    },
];

/// `publish_0` is fire-and-forget — there's no broker round-trip and
/// therefore no `network` failure surface; only client-side rejections
/// (e.g. invalid topic) show up.
static PUBLISH_QOS0_ERROR_TYPES: &[ErrorTypeDef] = &[ErrorTypeDef {
    name: "mqtt",
    fields: MqttError::FIELDS,
}];

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
        NETWORK_AND_MQTT_ERROR_TYPES
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
                NETWORK_AND_MQTT_ERROR_TYPES
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
publish_cmd!(Publish1, "publish_1", PUBLISH_1, NETWORK_AND_MQTT_ERROR_TYPES);
publish_cmd!(Publish2, "publish_2", PUBLISH_2, NETWORK_AND_MQTT_ERROR_TYPES);

// ── Receive: split into receive_publish / receive_disconnect ─────────────────
//
// Source syntax is `receive publish` / `receive disconnect`. The mqtt_client
// actor type fuses the leading event ident into the command name at
// resolution time (see `MqttClient::resolve_command`), so the validator and
// harness see a fully-named command and can statically type-check the
// per-event `ok.*` fields and mode transitions.

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
