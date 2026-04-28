// Runtime half of the `mqtt_client` actor. State machine over the actor's
// modes: `Disconnected` carries no live broker resource; `Connected` owns
// the `rumqttc::v5::AsyncClient` plus the `JoinHandle` of the background
// event-loop task and a few channels for routing incoming packets.
//
// rumqttc serialises outgoing requests, so we track in-flight ack waiters as
// per-shape FIFO queues (one for SubAck, one each for QoS-1 PubAck and
// QoS-2 PubComp). The event-loop task pops the front waiter of the matching
// queue when an ack arrives. Incoming Publish and Disconnect packets get
// dropped onto two inbox channels which the `receive_publish` /
// `receive_disconnect` commands drain.
//
// Construct is lazy — no I/O at declaration time. Connection only happens
// on `connect`, matching the example `actor alice = mqtt_client` (no kwargs
// on declaration). This mirrors `pg_client` and keeps connect-time errors
// surfaced from the connect command itself.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rumqttc::v5::mqttbytes::v5::{
    ConnAck, ConnectProperties, ConnectReturnCode, Disconnect as DisconnectPacket, PubAck,
    PubAckReason, PubComp, Publish, PublishProperties, SubAck, SubscribeProperties,
    SubscribeReasonCode,
};
use rumqttc::v5::mqttbytes::QoS;
use rumqttc::v5::{
    AsyncClient, ConnectionError, Event, EventLoop, Incoming, MqttOptions, StateError,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::commands::{
    ConnectOk, MqttError, NetworkError, ReceiveDisconnectOk, ReceivePublishOk, TimeoutError,
};
use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, Dict, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 1883;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_KEEP_ALIVE_SECS: u64 = 60;
/// Buffer between `AsyncClient` callers and the rumqttc event loop; sized
/// well above any single command's request fan-out (subscribe / publish ≤ 1
/// per call) so steady-state operation never blocks on send.
const REQUEST_CHANNEL_CAP: usize = 10;
/// Inbox buffer for incoming PUBLISH packets between event-loop and
/// `receive_publish`. Tests with very fast-talking brokers and a
/// slow-draining receiver could fill this; the publish_tx send then awaits.
/// 64 is a deliberate compromise between memory and back-pressure latency.
const INBOX_CAP: usize = 64;

// Network atoms — share the set with `pg_client`.
const NET_HOST_UNREACHABLE: &str = "host_unreachable";
const NET_CONNECTION_REFUSED: &str = "connection_refused";
const NET_CONNECTION_LOST: &str = "connection_lost";
const NET_TIMEOUT: &str = "timeout";
const NET_OTHER: &str = "other";

// Sentinel reason_code for client-side `error.mqtt` outcomes that did not
// come from the broker. `0` is a real v5 success code (UNSPECIFIED), so we
// use `-1` to make it impossible to confuse.
const CLIENT_SIDE_REASON_CODE: i64 = -1;

// ── Outcome construction helpers ──────────────────────────────────────────────

fn network_error(reason: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "network",
        fields: NetworkError {
            reason: reason.into(),
        }
        .into_dict(),
    }
}

fn mqtt_error(reason: &str, reason_code: i64) -> RunOutcome {
    RunOutcome::Error {
        variant: "mqtt",
        fields: MqttError {
            reason: reason.into(),
            reason_code,
        }
        .into_dict(),
    }
}

fn timeout_error() -> RunOutcome {
    RunOutcome::Error {
        variant: "timeout",
        fields: TimeoutError {}.into_dict(),
    }
}

// ── Mode + instance ───────────────────────────────────────────────────────────

pub struct MqttClientInstance {
    mode: MqttMode,
}

pub enum MqttMode {
    Disconnected(Disconnected),
    Connected(Connected),
}

impl Default for MqttMode {
    fn default() -> Self {
        MqttMode::Disconnected(Disconnected)
    }
}

pub struct Disconnected;

pub struct Connected {
    client: AsyncClient,
    /// Drives the rumqttc `EventLoop`. Aborting closes the network connection
    /// without any further packets — used as the cleanup path for both
    /// graceful disconnect and panic-mid-test recovery via `Drop`.
    event_loop_task: JoinHandle<()>,
    /// Incoming PUBLISH packets from the broker; drained by `receive_publish`.
    publish_inbox: mpsc::Receiver<Publish>,
    /// Incoming DISCONNECT from the broker (e.g. session takeover, quota,
    /// shutdown); drained by `receive_disconnect`. Single-shot: once we've
    /// observed the disconnect, the event loop has already exited.
    disconnect_inbox: mpsc::Receiver<DisconnectPacket>,
    waiters: Waiters,
}

/// Three FIFO queues for in-flight broker round-trips, one per ack type.
/// rumqttc preserves request order, so popping the front of the matching
/// queue when an ack arrives wakes the right caller without tracking pkids.
#[derive(Clone)]
struct Waiters {
    /// Waiters for in-flight SUBACKs.
    sub: WaiterQueue,
    /// Waiters for in-flight QoS-1 PUBACKs.
    qos1: WaiterQueue,
    /// Waiters for in-flight QoS-2 PUBCOMPs (the QoS-2 handshake's terminal
    /// ack — rumqttc handles the PUBREC/PUBREL relay internally).
    qos2: WaiterQueue,
}

impl Waiters {
    fn new() -> Self {
        Self {
            sub: new_waiter_queue(),
            qos1: new_waiter_queue(),
            qos2: new_waiter_queue(),
        }
    }

    /// Resolve every pending waiter on every queue with the same error,
    /// used when the connection drops and nothing further can ack.
    fn fail_all_with(&self, factory: impl Fn() -> RunOutcome) {
        let f: &dyn Fn() -> RunOutcome = &factory;
        drain_waiters_with_error(&self.sub, f);
        drain_waiters_with_error(&self.qos1, f);
        drain_waiters_with_error(&self.qos2, f);
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        self.event_loop_task.abort();
    }
}

/// A `oneshot::Sender` for an ack outcome. `Ok(())` means broker accepted;
/// `Err(RunOutcome)` carries a structured error to surface to the caller.
type WaiterTx = oneshot::Sender<Result<(), RunOutcome>>;
type WaiterQueue = Arc<Mutex<VecDeque<WaiterTx>>>;

fn new_waiter_queue() -> WaiterQueue {
    Arc::new(Mutex::new(VecDeque::new()))
}

fn push_waiter(q: &WaiterQueue) -> oneshot::Receiver<Result<(), RunOutcome>> {
    let (tx, rx) = oneshot::channel();
    q.lock().unwrap().push_back(tx);
    rx
}

fn pop_waiter(q: &WaiterQueue) -> Option<WaiterTx> {
    q.lock().unwrap().pop_front()
}

fn drain_waiters_with_error(q: &WaiterQueue, err_factory: &dyn Fn() -> RunOutcome) {
    let drained: Vec<WaiterTx> = q.lock().unwrap().drain(..).collect();
    for w in drained {
        let _ = w.send(Err(err_factory()));
    }
}

// ── Construct ─────────────────────────────────────────────────────────────────

impl MqttClientInstance {
    pub async fn construct(_args: &ConstructArgs) -> Result<Self, RuntimeError> {
        Ok(MqttClientInstance {
            mode: MqttMode::default(),
        })
    }
}

// ── Connect ───────────────────────────────────────────────────────────────────

impl Disconnected {
    async fn connect(self, kw: &Dict) -> (MqttMode, RunOutcome) {
        let cfg = match build_connect_config(kw) {
            Ok(c) => c,
            // The validator enforces kwarg types; this only fires if a test
            // bypasses validation. Surface as `:other` rather than panic.
            Err(()) => return (MqttMode::Disconnected(self), mqtt_error("other", CLIENT_SIDE_REASON_CODE)),
        };

        // Retry transient transport failures inside the timeout budget — same
        // shape as pg_client, since a freshly-started broker container needs
        // a moment to bind its port.
        let deadline = tokio::time::Instant::now() + cfg.timeout;
        let mut last: Option<RunOutcome> = None;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return (
                    MqttMode::Disconnected(self),
                    last.unwrap_or_else(|| network_error(NET_TIMEOUT)),
                );
            }
            match tokio::time::timeout(remaining, attempt_connect(&cfg)).await {
                Err(_) => return (MqttMode::Disconnected(self), network_error(NET_TIMEOUT)),
                Ok(Ok(connected)) => {
                    return (MqttMode::Connected(connected.0), connected.1);
                }
                Ok(Err(outcome)) => {
                    if !is_transient_connect_error(&outcome) {
                        return (MqttMode::Disconnected(self), outcome);
                    }
                    last = Some(outcome);
                    let backoff = std::cmp::min(Duration::from_millis(200), remaining);
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}

struct ConnectConfig {
    options: MqttOptions,
    timeout: Duration,
}

fn build_connect_config(kw: &Dict) -> Result<ConnectConfig, ()> {
    let host = match kw.get("host") {
        Some(Value::String(s)) => s.clone(),
        None => DEFAULT_HOST.to_string(),
        Some(_) => return Err(()),
    };
    let port = match kw.get("port") {
        Some(Value::Number(n)) if *n > 0 && *n <= u16::MAX as i64 => *n as u16,
        None => DEFAULT_PORT,
        Some(_) => return Err(()),
    };
    let client_id = match kw.get("client_id") {
        Some(Value::String(s)) => s.clone(),
        None => String::new(),
        Some(_) => return Err(()),
    };

    let mut options = MqttOptions::new(client_id, host, port);
    options.set_request_channel_capacity(REQUEST_CHANNEL_CAP);

    if let Some(Value::Bool(clean)) = kw.get("clean_start") {
        options.set_clean_start(*clean);
    }
    if let Some(Value::Number(secs)) = kw.get("keep_alive") {
        if *secs > 0 {
            options.set_keep_alive(Duration::from_secs(*secs as u64));
        }
    } else {
        options.set_keep_alive(Duration::from_secs(DEFAULT_KEEP_ALIVE_SECS));
    }

    match (kw.get("username"), kw.get("password")) {
        (Some(Value::String(u)), Some(Value::String(p))) => {
            options.set_credentials(u.clone(), p.clone());
        }
        (Some(Value::String(u)), None) => {
            options.set_credentials(u.clone(), String::new());
        }
        (None, _) => {}
        _ => return Err(()),
    }

    if let Some(props) = dict_to_user_properties(kw.get("user_properties"))? {
        let mut connect_props = ConnectProperties::new();
        connect_props.user_properties = props;
        options.set_connect_properties(connect_props);
    }

    let timeout = match kw.get("timeout") {
        Some(Value::Number(n)) if *n > 0 => Duration::from_millis(*n as u64),
        _ => DEFAULT_CONNECT_TIMEOUT,
    };

    Ok(ConnectConfig { options, timeout })
}

/// One attempt at completing the MQTT handshake. Returns `Ok((Connected,
/// outcome))` on broker-accepted ConnAck, `Err(outcome)` on any other
/// terminal result (network failure, broker rejection, protocol error).
async fn attempt_connect(cfg: &ConnectConfig) -> Result<(Connected, RunOutcome), RunOutcome> {
    let (client, mut eventloop) = AsyncClient::new(cfg.options.clone(), REQUEST_CHANNEL_CAP);
    // Drive the event loop until ConnAck arrives (success) or the broker /
    // network rejects the handshake.
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(ack))) => {
                if ack.code == ConnectReturnCode::Success {
                    let connected = spawn_connected(client, eventloop);
                    let outcome = RunOutcome::Ok(connack_to_ok(&ack).into_dict());
                    return Ok((connected, outcome));
                } else {
                    let (atom, code) = connack_failure_atom(ack.code);
                    return Err(mqtt_error(atom, code));
                }
            }
            Ok(_) => {
                // PingReq/Resp/Outgoing/etc. — keep polling until ConnAck.
                continue;
            }
            Err(e) => return Err(classify_connect_error(&e)),
        }
    }
}

fn spawn_connected(client: AsyncClient, eventloop: EventLoop) -> Connected {
    let (publish_tx, publish_inbox) = mpsc::channel::<Publish>(INBOX_CAP);
    let (disconnect_tx, disconnect_inbox) = mpsc::channel::<DisconnectPacket>(1);
    let waiters = Waiters::new();

    let event_loop_task = tokio::spawn(run_event_loop(
        eventloop,
        publish_tx,
        disconnect_tx,
        waiters.clone(),
    ));

    Connected {
        client,
        event_loop_task,
        publish_inbox,
        disconnect_inbox,
        waiters,
    }
}

fn classify_connect_error(e: &ConnectionError) -> RunOutcome {
    match e {
        ConnectionError::ConnectionRefused(code) => {
            let (atom, byte) = connack_failure_atom(*code);
            mqtt_error(atom, byte)
        }
        ConnectionError::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => network_error(NET_CONNECTION_REFUSED),
            std::io::ErrorKind::TimedOut => network_error(NET_TIMEOUT),
            std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe => network_error(NET_CONNECTION_LOST),
            _ => network_error(NET_HOST_UNREACHABLE),
        },
        ConnectionError::Timeout(_) => network_error(NET_TIMEOUT),
        ConnectionError::NotConnAck(_) => mqtt_error("protocol_error", CLIENT_SIDE_REASON_CODE),
        _ => network_error(NET_OTHER),
    }
}

fn is_transient_connect_error(outcome: &RunOutcome) -> bool {
    matches!(
        outcome,
        RunOutcome::Error {
            variant: "network",
            ..
        }
    )
}

fn connack_to_ok(ack: &ConnAck) -> ConnectOk {
    let (assigned_client_id, user_properties) = match &ack.properties {
        Some(p) => (
            p.assigned_client_identifier.clone().unwrap_or_default(),
            user_properties_to_dict(&p.user_properties),
        ),
        None => (String::new(), Dict::new()),
    };
    ConnectOk {
        session_present: ack.session_present,
        assigned_client_id,
        user_properties,
    }
}

/// Map a non-Success `ConnectReturnCode` to (`error.mqtt.reason` atom, raw v5 byte).
fn connack_failure_atom(code: ConnectReturnCode) -> (&'static str, i64) {
    match code {
        ConnectReturnCode::Success => ("other", 0),
        ConnectReturnCode::UnspecifiedError => ("unspecified_error", 0x80),
        ConnectReturnCode::MalformedPacket => ("malformed_packet", 0x81),
        ConnectReturnCode::ProtocolError => ("protocol_error", 0x82),
        ConnectReturnCode::ImplementationSpecificError => ("implementation_specific_error", 0x83),
        ConnectReturnCode::UnsupportedProtocolVersion | ConnectReturnCode::RefusedProtocolVersion => {
            ("unsupported_protocol_version", 0x84)
        }
        ConnectReturnCode::ClientIdentifierNotValid | ConnectReturnCode::BadClientId => {
            ("client_identifier_not_valid", 0x85)
        }
        ConnectReturnCode::BadUserNamePassword => ("bad_user_name_or_password", 0x86),
        ConnectReturnCode::NotAuthorized => ("not_authorized", 0x87),
        ConnectReturnCode::ServerUnavailable | ConnectReturnCode::ServiceUnavailable => {
            ("server_unavailable", 0x88)
        }
        ConnectReturnCode::ServerBusy => ("server_busy", 0x89),
        ConnectReturnCode::Banned => ("banned", 0x8A),
        ConnectReturnCode::BadAuthenticationMethod => ("bad_authentication_method", 0x8C),
        ConnectReturnCode::TopicNameInvalid => ("topic_name_invalid", 0x90),
        ConnectReturnCode::PacketTooLarge => ("packet_too_large", 0x95),
        ConnectReturnCode::QuotaExceeded => ("quota_exceeded", 0x97),
        ConnectReturnCode::PayloadFormatInvalid => ("payload_format_invalid", 0x99),
        ConnectReturnCode::RetainNotSupported => ("retain_not_supported", 0x9A),
        ConnectReturnCode::QoSNotSupported => ("qos_not_supported", 0x9B),
        ConnectReturnCode::UseAnotherServer => ("use_another_server", 0x9C),
        ConnectReturnCode::ServerMoved => ("server_moved", 0x9D),
        ConnectReturnCode::ConnectionRateExceeded => ("connection_rate_exceeded", 0x9F),
    }
}

// ── Event-loop task ───────────────────────────────────────────────────────────

async fn run_event_loop(
    mut eventloop: EventLoop,
    publish_tx: mpsc::Sender<Publish>,
    disconnect_tx: mpsc::Sender<DisconnectPacket>,
    waiters: Waiters,
) {
    loop {
        let event = match eventloop.poll().await {
            Ok(e) => e,
            Err(err) => {
                // rumqttc surfaces incoming v5 DISCONNECT packets as
                // `StateError::ServerDisconnect` rather than as a separate
                // `Event::Incoming(Packet::Disconnect)`, so a broker
                // disconnect (session takeover, server going away, etc.)
                // arrives here. Synthesize the packet for the inbox so
                // `receive_disconnect` returns the reason code instead of
                // `connection_lost`.
                if let ConnectionError::MqttState(StateError::ServerDisconnect {
                    reason_code,
                    ..
                }) = &err
                {
                    let synth = DisconnectPacket {
                        reason_code: *reason_code,
                        properties: None,
                    };
                    let _ = disconnect_tx.send(synth).await;
                }
                waiters.fail_all_with(|| network_error(NET_CONNECTION_LOST));
                return;
            }
        };

        match event {
            Event::Incoming(Incoming::Publish(p)) => {
                // If the inbox is full or the receiver was dropped, the
                // message is lost. Tests that exercise back-pressure aren't
                // in scope; matches `pg_client`'s "fail loud later" stance.
                let _ = publish_tx.send(p).await;
            }
            Event::Incoming(Incoming::Disconnect(d)) => {
                let _ = disconnect_tx.send(d).await;
                waiters.fail_all_with(|| network_error(NET_CONNECTION_LOST));
                return;
            }
            Event::Incoming(Incoming::SubAck(sa)) => {
                if let Some(w) = pop_waiter(&waiters.sub) {
                    let _ = w.send(suback_outcome(&sa));
                }
            }
            Event::Incoming(Incoming::PubAck(pa)) => {
                if let Some(w) = pop_waiter(&waiters.qos1) {
                    let _ = w.send(puback_outcome(&pa));
                }
            }
            Event::Incoming(Incoming::PubComp(pc)) => {
                if let Some(w) = pop_waiter(&waiters.qos2) {
                    let _ = w.send(pubcomp_outcome(&pc));
                }
            }
            // PubRec / PubRel are relayed internally by rumqttc as part of
            // the QoS-2 handshake; PubComp is the only ack we observe.
            _ => {}
        }
    }
}

fn suback_outcome(sa: &SubAck) -> Result<(), RunOutcome> {
    // SUBACK return codes: each subscribed topic gets one. We currently
    // subscribe to a single topic per command, so a single non-success code
    // becomes the error.
    for code in &sa.return_codes {
        if let Some((atom, byte)) = suback_failure_atom(*code) {
            return Err(mqtt_error(atom, byte));
        }
    }
    Ok(())
}

fn suback_failure_atom(code: SubscribeReasonCode) -> Option<(&'static str, i64)> {
    match code {
        SubscribeReasonCode::Success(_) => None,
        SubscribeReasonCode::Failure => Some(("unspecified_error", 0x80)),
        SubscribeReasonCode::Unspecified => Some(("unspecified_error", 0x80)),
        SubscribeReasonCode::ImplementationSpecific => {
            Some(("implementation_specific_error", 0x83))
        }
        SubscribeReasonCode::NotAuthorized => Some(("not_authorized", 0x87)),
        SubscribeReasonCode::TopicFilterInvalid => Some(("topic_filter_invalid", 0x8F)),
        SubscribeReasonCode::PkidInUse => Some(("packet_identifier_in_use", 0x91)),
        SubscribeReasonCode::QuotaExceeded => Some(("quota_exceeded", 0x97)),
        SubscribeReasonCode::SharedSubscriptionsNotSupported => {
            Some(("shared_subscriptions_not_supported", 0x9E))
        }
        SubscribeReasonCode::SubscriptionIdNotSupported => {
            Some(("subscription_identifiers_not_supported", 0xA1))
        }
        SubscribeReasonCode::WildcardSubscriptionsNotSupported => {
            Some(("wildcard_subscriptions_not_supported", 0xA2))
        }
    }
}

fn puback_outcome(pa: &PubAck) -> Result<(), RunOutcome> {
    if let Some((atom, byte)) = puback_failure_atom(pa.reason) {
        Err(mqtt_error(atom, byte))
    } else {
        Ok(())
    }
}

fn puback_failure_atom(reason: PubAckReason) -> Option<(&'static str, i64)> {
    match reason {
        PubAckReason::Success | PubAckReason::NoMatchingSubscribers => None,
        PubAckReason::UnspecifiedError => Some(("unspecified_error", 0x80)),
        PubAckReason::ImplementationSpecificError => Some(("implementation_specific_error", 0x83)),
        PubAckReason::NotAuthorized => Some(("not_authorized", 0x87)),
        PubAckReason::TopicNameInvalid => Some(("topic_name_invalid", 0x90)),
        PubAckReason::PacketIdentifierInUse => Some(("packet_identifier_in_use", 0x91)),
        PubAckReason::QuotaExceeded => Some(("quota_exceeded", 0x97)),
        PubAckReason::PayloadFormatInvalid => Some(("payload_format_invalid", 0x99)),
    }
}

fn pubcomp_outcome(_pc: &PubComp) -> Result<(), RunOutcome> {
    // PubComp reason codes mirror the PUBACK set; the rumqttc enum doesn't
    // expose a distinct typed reason, so we treat any received PubComp as
    // success. If broker QoS-2 rejection diagnostics become important, route
    // PubRec failures here instead — but the typical surface is
    // `unspecified_error` / `not_authorized`, which the broker delivers via
    // PUBREC, terminating the handshake before PUBCOMP.
    Ok(())
}

// ── Disconnect ────────────────────────────────────────────────────────────────

impl Connected {
    async fn disconnect(self, kw: &Dict) -> (MqttMode, RunOutcome) {
        // We currently surface no kwargs to rumqttc beyond the bare disconnect
        // request — its v5 API doesn't expose a builder for outgoing
        // DISCONNECT properties on the AsyncClient surface yet. The reason_code
        // and user_properties kwargs are accepted for forward compatibility;
        // they're a no-op on the wire today. Validator already declares the
        // shape so users can write the test once.
        let _ = kw;
        let _ = self.client.disconnect().await;
        // `Drop for Connected` aborts the event loop task once `self` falls
        // out of scope at the end of this function.
        (MqttMode::Disconnected(Disconnected), RunOutcome::Ok(Dict::new()))
    }

    async fn teardown(self) -> (MqttMode, TeardownOutcome) {
        let _ = self.client.disconnect().await;
        (MqttMode::Disconnected(Disconnected), TeardownOutcome::ok())
    }
}

// ── Subscribe / Publish ───────────────────────────────────────────────────────

impl Connected {
    async fn subscribe(&mut self, qos: QoS, args: &CommandArgs) -> RunOutcome {
        let topic = match args.positional.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return mqtt_error("topic_filter_invalid", CLIENT_SIDE_REASON_CODE),
        };
        let user_props = match dict_to_user_properties(args.kw("user_properties")) {
            Ok(p) => p,
            Err(()) => return mqtt_error("other", CLIENT_SIDE_REASON_CODE),
        };

        let rx = push_waiter(&self.waiters.sub);
        let send_result = if let Some(props) = user_props {
            let sub_props = SubscribeProperties {
                id: None,
                user_properties: props,
            };
            self.client
                .subscribe_with_properties(topic, qos, sub_props)
                .await
        } else {
            self.client.subscribe(topic, qos).await
        };
        if let Err(_e) = send_result {
            // ClientError means the request channel is closed (event loop
            // exited). Pop our just-pushed waiter so it doesn't leak.
            let _ = pop_waiter(&self.waiters.sub);
            return network_error(NET_CONNECTION_LOST);
        }

        match rx.await {
            Ok(Ok(())) => RunOutcome::Ok(Dict::new()),
            Ok(Err(outcome)) => outcome,
            Err(_) => network_error(NET_CONNECTION_LOST),
        }
    }

    async fn publish(&mut self, qos: QoS, args: &CommandArgs) -> RunOutcome {
        let topic = match args.positional.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return mqtt_error("topic_name_invalid", CLIENT_SIDE_REASON_CODE),
        };
        // Empty topic isn't a valid PUBLISH topic name. Reject client-side so
        // QoS 0 (no broker round-trip) still gets a synchronous error.
        if topic.is_empty() {
            return mqtt_error("invalid_topic", CLIENT_SIDE_REASON_CODE);
        }

        let payload: Vec<u8> = match args.positional.get(1) {
            Some(Value::Bytes(b)) => b.clone(),
            Some(Value::String(s)) => s.as_bytes().to_vec(),
            _ => return mqtt_error("payload_format_invalid", CLIENT_SIDE_REASON_CODE),
        };

        let user_props = match dict_to_user_properties(args.kw("user_properties")) {
            Ok(p) => p,
            Err(()) => return mqtt_error("other", CLIENT_SIDE_REASON_CODE),
        };

        // QoS 0: no ack — succeed as soon as rumqttc accepts the request.
        if qos == QoS::AtMostOnce {
            let send_result = if let Some(props) = user_props {
                let pub_props = PublishProperties {
                    user_properties: props,
                    ..Default::default()
                };
                self.client
                    .publish_with_properties(topic, qos, false, payload, pub_props)
                    .await
            } else {
                self.client.publish(topic, qos, false, payload).await
            };
            return match send_result {
                Ok(()) => RunOutcome::Ok(Dict::new()),
                Err(_) => network_error(NET_CONNECTION_LOST),
            };
        }

        // QoS 1/2: register the appropriate waiter before sending so we can't
        // miss the ack even if it arrives before we await.
        let waiters = if qos == QoS::AtLeastOnce {
            &self.waiters.qos1
        } else {
            &self.waiters.qos2
        };
        let rx = push_waiter(waiters);

        let send_result = if let Some(props) = user_props {
            let pub_props = PublishProperties {
                user_properties: props,
                ..Default::default()
            };
            self.client
                .publish_with_properties(topic, qos, false, payload, pub_props)
                .await
        } else {
            self.client.publish(topic, qos, false, payload).await
        };
        if let Err(_e) = send_result {
            let _ = pop_waiter(waiters);
            return network_error(NET_CONNECTION_LOST);
        }

        match rx.await {
            Ok(Ok(())) => RunOutcome::Ok(Dict::new()),
            Ok(Err(outcome)) => outcome,
            Err(_) => network_error(NET_CONNECTION_LOST),
        }
    }
}

// ── Receive ───────────────────────────────────────────────────────────────────

impl Connected {
    async fn receive_publish(&mut self, args: &CommandArgs) -> RunOutcome {
        let timeout = receive_timeout(args);
        match tokio::time::timeout(timeout, self.publish_inbox.recv()).await {
            Err(_) => timeout_error(),
            Ok(None) => network_error(NET_CONNECTION_LOST),
            Ok(Some(p)) => {
                let topic = String::from_utf8_lossy(&p.topic).into_owned();
                let user_properties = match &p.properties {
                    Some(props) => user_properties_to_dict(&props.user_properties),
                    None => Dict::new(),
                };
                RunOutcome::Ok(
                    ReceivePublishOk {
                        topic,
                        payload: p.payload.to_vec(),
                        qos: qos_to_byte(p.qos),
                        user_properties,
                    }
                    .into_dict(),
                )
            }
        }
    }

    async fn receive_disconnect(self, args: &CommandArgs) -> (MqttMode, RunOutcome) {
        let timeout = receive_timeout(args);
        let mut me = self;
        let outcome = match tokio::time::timeout(timeout, me.disconnect_inbox.recv()).await {
            Err(_) => return (MqttMode::Connected(me), timeout_error()),
            Ok(None) => network_error(NET_CONNECTION_LOST),
            Ok(Some(d)) => {
                let user_properties = match &d.properties {
                    Some(props) => user_properties_to_dict(&props.user_properties),
                    None => Dict::new(),
                };
                RunOutcome::Ok(
                    ReceiveDisconnectOk {
                        reason_code: d.reason_code as i64,
                        user_properties,
                    }
                    .into_dict(),
                )
            }
        };
        // The session has ended; transition to Disconnected. `Drop for
        // Connected` aborts the (already-exited) event loop task as `me`
        // falls out of scope.
        (MqttMode::Disconnected(Disconnected), outcome)
    }
}

fn receive_timeout(args: &CommandArgs) -> Duration {
    match args.kw("timeout") {
        Some(Value::Number(n)) if *n > 0 => Duration::from_secs(*n as u64),
        _ => DEFAULT_RECEIVE_TIMEOUT,
    }
}

fn qos_to_byte(qos: QoS) -> i64 {
    match qos {
        QoS::AtMostOnce => 0,
        QoS::AtLeastOnce => 1,
        QoS::ExactlyOnce => 2,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn user_properties_to_dict(props: &[(String, String)]) -> Dict {
    let mut d = Dict::new();
    for (k, v) in props {
        d.insert(k.clone(), Value::String(v.clone()));
    }
    d
}

/// Convert a kwarg-supplied `user_properties` Dict into the `Vec<(K, V)>`
/// rumqttc expects. Returns `Ok(None)` when the kwarg is absent, `Ok(Some)`
/// when it's a string-keyed string-valued dict, and `Err(())` when the
/// shape is wrong (validator should have caught; defensive).
fn dict_to_user_properties(value: Option<&Value>) -> Result<Option<Vec<(String, String)>>, ()> {
    let dict = match value {
        Some(Value::Dict(d)) => d,
        None => return Ok(None),
        _ => return Err(()),
    };
    let mut out = Vec::with_capacity(dict.len());
    for (k, v) in dict {
        match v {
            Value::String(s) => out.push((k.clone(), s.clone())),
            _ => return Err(()),
        }
    }
    Ok(Some(out))
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl ActorInstance for MqttClientInstance {
    fn type_name(&self) -> &'static str {
        "mqtt_client"
    }

    async fn execute(&mut self, cmd: &'static str, args: &CommandArgs) -> RunOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            MqttMode::Disconnected(d) => match cmd {
                "connect" => d.connect(&args.keyword).await,
                other => (
                    MqttMode::Disconnected(d),
                    RunOutcome::NotImplemented {
                        actor: "mqtt_client",
                        cmd: other,
                    },
                ),
            },
            MqttMode::Connected(mut c) => match cmd {
                "disconnect" => c.disconnect(&args.keyword).await,
                "subscribe_0" => {
                    let outcome = c.subscribe(QoS::AtMostOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "subscribe_1" => {
                    let outcome = c.subscribe(QoS::AtLeastOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "subscribe_2" => {
                    let outcome = c.subscribe(QoS::ExactlyOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "publish_0" => {
                    let outcome = c.publish(QoS::AtMostOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "publish_1" => {
                    let outcome = c.publish(QoS::AtLeastOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "publish_2" => {
                    let outcome = c.publish(QoS::ExactlyOnce, args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "receive_publish" => {
                    let outcome = c.receive_publish(args).await;
                    (MqttMode::Connected(c), outcome)
                }
                "receive_disconnect" => c.receive_disconnect(args).await,
                other => (
                    MqttMode::Connected(c),
                    RunOutcome::NotImplemented {
                        actor: "mqtt_client",
                        cmd: other,
                    },
                ),
            },
        };
        self.mode = next;
        outcome
    }

    async fn teardown(&mut self) -> TeardownOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            MqttMode::Disconnected(d) => (MqttMode::Disconnected(d), TeardownOutcome::ok()),
            MqttMode::Connected(c) => c.teardown().await,
        };
        self.mode = next;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mosquitto-gated tests ──────────────────────────────────────────────
    //
    // These spin up a real `eclipse-mosquitto` testcontainer and exercise
    // the actor end-to-end. `#[ignore]` by default so `cargo test` stays
    // offline-friendly. Run locally with:
    //
    //     cargo test -p ill-core --lib mqtt_client -- --ignored

    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt};

    /// Minimal mosquitto config: v5-capable listener on 1883, anonymous
    /// connections allowed. The default eclipse-mosquitto:2.x image config
    /// requires auth, so we override.
    const MOSQUITTO_CONF: &[u8] = b"listener 1883\nallow_anonymous true\n";

    async fn start_mosquitto() -> ContainerAsync<GenericImage> {
        GenericImage::new("eclipse-mosquitto", "2.0")
            .with_exposed_port(1883.tcp())
            .with_wait_for(WaitFor::message_on_stderr("running"))
            .with_copy_to("/mosquitto/config/mosquitto.conf", MOSQUITTO_CONF.to_vec())
            .with_cmd(vec![
                "mosquitto",
                "-c",
                "/mosquitto/config/mosquitto.conf",
            ])
            .start()
            .await
            .expect("start mosquitto container")
    }

    async fn host_port(c: &ContainerAsync<GenericImage>) -> u16 {
        c.get_host_port_ipv4(1883.tcp())
            .await
            .expect("get host port")
    }

    fn empty_construct() -> ConstructArgs {
        ConstructArgs {
            keyword: Dict::new(),
            source_dir: std::env::temp_dir(),
            vars: Vec::new(),
        }
    }

    fn connect_args(port: u16) -> CommandArgs {
        let mut kw = Dict::new();
        kw.insert("host".into(), Value::String("127.0.0.1".into()));
        kw.insert("port".into(), Value::Number(port as i64));
        kw.insert("timeout".into(), Value::Number(15_000));
        CommandArgs {
            positional: Vec::new(),
            keyword: kw,
        }
    }

    fn empty_args() -> CommandArgs {
        CommandArgs {
            positional: Vec::new(),
            keyword: Dict::new(),
        }
    }

    fn topic_args(topic: &str) -> CommandArgs {
        CommandArgs {
            positional: vec![Value::String(topic.into())],
            keyword: Dict::new(),
        }
    }

    fn publish_args(topic: &str, payload: Vec<u8>) -> CommandArgs {
        CommandArgs {
            positional: vec![Value::String(topic.into()), Value::Bytes(payload)],
            keyword: Dict::new(),
        }
    }

    fn expect_ok(o: RunOutcome) -> Dict {
        match o {
            RunOutcome::Ok(f) => f,
            RunOutcome::Error { variant, fields } => {
                panic!("expected Ok, got Error {variant}: {fields:?}")
            }
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected Ok, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    fn expect_error_variant(o: RunOutcome, expected: &str) -> Dict {
        match o {
            RunOutcome::Error { variant, fields } => {
                assert_eq!(variant, expected, "error variant mismatch: {fields:?}");
                fields
            }
            RunOutcome::Ok(f) => panic!("expected Error::{expected}, got Ok({f:?})"),
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected Error::{expected}, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_disconnect_happy_path() {
        let broker = start_mosquitto().await;
        let port = host_port(&broker).await;

        let mut inst = MqttClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        assert!(matches!(inst.mode, MqttMode::Disconnected(_)));

        let _ok = expect_ok(inst.execute("connect", &connect_args(port)).await);
        assert!(matches!(inst.mode, MqttMode::Connected(_)));

        let _ok = expect_ok(inst.execute("disconnect", &empty_args()).await);
        assert!(matches!(inst.mode, MqttMode::Disconnected(_)));

        let td = inst.teardown().await;
        assert!(td.ok, "teardown failed: {:?}", td.message);
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn connect_refused_when_no_listener() {
        // No broker on port 1 — TCP connect should be refused.
        let mut inst = MqttClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let mut kw = Dict::new();
        kw.insert("host".into(), Value::String("127.0.0.1".into()));
        kw.insert("port".into(), Value::Number(1));
        kw.insert("timeout".into(), Value::Number(1_000));
        let outcome = inst
            .execute(
                "connect",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        let fields = expect_error_variant(outcome, "network");
        // Linux usually surfaces ECONNREFUSED; macOS sometimes EHOSTUNREACH.
        // Both are network-class — assert the variant, not the atom.
        assert!(matches!(fields.get("reason"), Some(Value::Atom(_))));
        assert!(matches!(inst.mode, MqttMode::Disconnected(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn subscribe_publish_receive_qos0() {
        let broker = start_mosquitto().await;
        let port = host_port(&broker).await;

        let mut inst = MqttClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(inst.execute("connect", &connect_args(port)).await);
        let _ = expect_ok(inst.execute("subscribe_0", &topic_args("greetings")).await);
        let _ = expect_ok(
            inst.execute("publish_0", &publish_args("greetings", b"hello".to_vec()))
                .await,
        );
        let ok = expect_ok(inst.execute("receive_publish", &empty_args()).await);
        assert_eq!(
            ok.get("payload"),
            Some(&Value::Bytes(b"hello".to_vec()))
        );
        assert_eq!(
            ok.get("topic"),
            Some(&Value::String("greetings".into()))
        );
        let td = inst.teardown().await;
        assert!(td.ok);
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn publish_empty_topic_is_invalid() {
        let broker = start_mosquitto().await;
        let port = host_port(&broker).await;
        let mut inst = MqttClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(inst.execute("connect", &connect_args(port)).await);

        let outcome = inst
            .execute("publish_0", &publish_args("", b"x".to_vec()))
            .await;
        let fields = expect_error_variant(outcome, "mqtt");
        assert_eq!(fields.get("reason"), Some(&Value::Atom("invalid_topic".into())));
        assert_eq!(
            fields.get("reason_code"),
            Some(&Value::Number(CLIENT_SIDE_REASON_CODE))
        );
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn receive_timeout_when_no_publish_arrives() {
        let broker = start_mosquitto().await;
        let port = host_port(&broker).await;
        let mut inst = MqttClientInstance::construct(&empty_construct())
            .await
            .expect("construct");
        let _ = expect_ok(inst.execute("connect", &connect_args(port)).await);
        let _ = expect_ok(inst.execute("subscribe_0", &topic_args("nothing")).await);

        let mut kw = Dict::new();
        kw.insert("timeout".into(), Value::Number(1));
        let outcome = inst
            .execute(
                "receive_publish",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        let _ = expect_error_variant(outcome, "timeout");
    }

    /// End-to-end coverage through the `.ill` harness. The default
    /// `eclipse-mosquitto:2.x` image refuses anonymous connections, so we
    /// build a tiny derived image that bakes in `allow_anonymous true`. The
    /// `mosquitto.conf` lives next to the Dockerfile in the test's tempdir
    /// — the container actor's build context picks it up via the
    /// dockerfile-parent-as-context behaviour.
    #[tokio::test]
    #[ignore = "requires docker"]
    async fn end_to_end_through_harness() {
        use crate::runtime::harness::run_test_file;
        use std::io::Write;

        let tmp = tempdir_for_e2e();

        let conf = "listener 1883\nallow_anonymous true\n";
        std::fs::write(tmp.path().join("mosquitto.conf"), conf).unwrap();
        let dockerfile = "FROM eclipse-mosquitto:2.0\n\
            COPY mosquitto.conf /mosquitto/config/mosquitto.conf\n";
        std::fs::write(tmp.path().join("Dockerfile.mosquitto"), dockerfile).unwrap();

        let src = "\
actor broker = container,
  dockerfile: \"Dockerfile.mosquitto\"
  internal_port: 1883
  vars:
    @access read
    port: 1883
actor alice = mqtt_client

as broker:
  run,
    external_port: self.port

as alice:
  connect,
    host: \"127.0.0.1\"
    port: broker.port
    timeout: 30000

  subscribe_0 \"#\"
  publish_0 \"greetings\", \"hello\"
  receive publish
  assert ok.payload == \"hello\"
  assert ok.topic == \"greetings\"

  publish_1 \"greetings\", \"one\"
  receive publish
  assert ok.payload == \"one\"

  publish_2 \"greetings\", \"two\"
  receive publish
  assert ok.payload == \"two\"

  publish_0 \"sensors/data\", ~hex`DEADBEEF`
  receive publish
  assert ok.payload == ~hex`DEADBEEF`

  publish_0 \"greetings\", \"world\"
  receive publish
  let last_msg = ok.payload
  publish_0 \"greetings\", \"got: ${last_msg}\"
  receive publish
  assert ok.payload == \"got: world\"

  disconnect
";

        let ill_path = tmp.path().join("e2e.ill");
        let mut f = std::fs::File::create(&ill_path).unwrap();
        f.write_all(src.as_bytes()).unwrap();
        drop(f);

        let report = run_test_file(&ill_path, src).await;
        if !report.passed {
            panic!("expected pass; failures: {}", summarize_failures(&report));
        }
        assert_eq!(report.teardown.len(), 2);
        assert!(report.teardown.iter().all(|t| t.outcome.ok));
    }

    struct E2eTempDir {
        path: std::path::PathBuf,
    }

    impl E2eTempDir {
        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for E2eTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn tempdir_for_e2e() -> E2eTempDir {
        use std::time::{SystemTime, UNIX_EPOCH};
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!("ill-mqtt-e2e-{}-{suffix}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        E2eTempDir { path }
    }

    fn summarize_failures(report: &crate::runtime::report::TestReport) -> String {
        let summaries: Vec<String> = report
            .statements
            .iter()
            .map(|s| match s {
                crate::runtime::report::StatementReport::CommandFailure {
                    actor,
                    command,
                    error_fields,
                    ..
                } => format!("CommandFailure {actor}:{command} {error_fields:?}"),
                crate::runtime::report::StatementReport::AssertFailure {
                    actor,
                    left,
                    right,
                    op,
                    ..
                } => format!("AssertFailure {actor} {left} {op:?} {right:?}"),
                crate::runtime::report::StatementReport::EvalError {
                    actor, message, ..
                } => format!("EvalError {actor}: {message}"),
                crate::runtime::report::StatementReport::ConstructFailure {
                    actor, message, ..
                } => format!("ConstructFailure {actor}: {message}"),
                crate::runtime::report::StatementReport::CommandNotImplemented {
                    actor,
                    command,
                    ..
                } => format!("CommandNotImplemented {actor}:{command}"),
                crate::runtime::report::StatementReport::ValidationFailure(d) => {
                    let messages: Vec<String> = d.iter().map(|x| x.message.clone()).collect();
                    format!("ValidationFailure {messages:#?}")
                }
                crate::runtime::report::StatementReport::ParseFailure(errs) => {
                    format!("ParseFailure {errs:?}")
                }
            })
            .collect();
        format!("{summaries:#?}")
    }
}
