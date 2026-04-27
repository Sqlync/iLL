// The `mqtt_client` actor type — an MQTT v5 publish/subscribe client.

pub mod commands;
pub mod modes;

use crate::ast::Expr;

use super::{ActorType, Command, Mode};

pub struct MqttClient;

impl ActorType for MqttClient {
    fn name(&self) -> &'static str {
        "mqtt_client"
    }

    fn initial_mode(&self) -> &'static dyn Mode {
        modes::DISCONNECTED
    }

    fn modes(&self) -> &'static [&'static dyn Mode] {
        static MODES: &[&dyn Mode] = &[modes::DISCONNECTED, modes::CONNECTED];
        MODES
    }

    fn commands(&self) -> &'static [&'static dyn Command] {
        static COMMANDS: &[&dyn Command] = &[
            commands::CONNECT,
            commands::DISCONNECT,
            commands::SUBSCRIBE_0,
            commands::SUBSCRIBE_1,
            commands::SUBSCRIBE_2,
            commands::PUBLISH_0,
            commands::PUBLISH_1,
            commands::PUBLISH_2,
            commands::RECEIVE_PUBLISH,
            commands::RECEIVE_DISCONNECT,
        ];
        COMMANDS
    }

    /// `receive publish` / `receive disconnect` are the surface forms for
    /// `receive_publish` / `receive_disconnect`. The event keyword is parsed
    /// as a leading ident positional; we fuse it into the command name here
    /// so the validator and harness can resolve the per-event command shape
    /// (different `ok.*` fields, different mode transitions) without any AST
    /// rewriting.
    fn resolve_command(
        &self,
        name: &str,
        positional: &[Expr],
    ) -> Option<(&'static dyn Command, usize)> {
        if name == "receive" {
            if let Some(Expr::Ident(event)) = positional.first() {
                let fused = format!("receive_{}", event.name);
                if let Some(cmd) = self.command(&fused) {
                    return Some((cmd, 1));
                }
            }
        }
        self.command(name).map(|c| (c, 0))
    }
}

pub static MQTT_CLIENT: &dyn ActorType = &MqttClient;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, Span};

    fn ident(name: &str) -> Expr {
        Expr::Ident(Ident {
            name: name.into(),
            span: Span { start: 0, end: 0 },
        })
    }

    #[test]
    fn resolve_receive_publish_fuses_and_consumes_one() {
        let pos = [ident("publish")];
        let (cmd, consumed) = MqttClient.resolve_command("receive", &pos).unwrap();
        assert_eq!(cmd.name(), "receive_publish");
        assert_eq!(consumed, 1);
    }

    #[test]
    fn resolve_receive_disconnect_fuses_and_consumes_one() {
        let pos = [ident("disconnect")];
        let (cmd, consumed) = MqttClient.resolve_command("receive", &pos).unwrap();
        assert_eq!(cmd.name(), "receive_disconnect");
        assert_eq!(consumed, 1);
    }

    #[test]
    fn resolve_plain_command_consumes_nothing() {
        let (cmd, consumed) = MqttClient.resolve_command("connect", &[]).unwrap();
        assert_eq!(cmd.name(), "connect");
        assert_eq!(consumed, 0);
    }

    #[test]
    fn resolve_receive_unknown_event_returns_none() {
        let pos = [ident("garbage")];
        assert!(MqttClient.resolve_command("receive", &pos).is_none());
    }

    #[test]
    fn resolve_receive_without_event_returns_none() {
        // Bare `receive` with no event keyword has no fused match and there's
        // no plain `receive` command — so resolution fails.
        assert!(MqttClient.resolve_command("receive", &[]).is_none());
    }
}
