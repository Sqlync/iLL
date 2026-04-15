// The `mqtt_client` actor type — an MQTT v5 publish/subscribe client.

pub mod commands;
pub mod modes;

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
            commands::RECEIVE,
        ];
        COMMANDS
    }
}

pub static MQTT_CLIENT: &dyn ActorType = &MqttClient;
