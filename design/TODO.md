# TODO

## Current


## Next


## Near Term

pg_client.connect accepts application
Restore testing of MQTT client cases that CC removed / couldn't implement
- see: https://github.com/Sqlync/iLL/pull/28/changes
- user properties
```
connect,
    host: "localhost"
    port: broker.port
assert ok.user_properties["server-version"] == "5.0"
```
- auth failure in `mqtt/connection_failures.ill`
```
# mqtt-level failure — broker rejected with CONNACK reason_code 135
as alice:
  connect,
    host: "localhost"
    port: broker.port
    username: "wrong"
    password: "wrong"
  assert error.mqtt.reason == :not_authorized
  assert error.mqtt.reason_code == 135
```
- disconnection
```
receive disconnect
assert ok.reason_code == 142
```
create ~json_b squiggle
squiggles
- actually validate squiggle contents
- split into separate crates
consider implementing a ValueType instead of using stringly typed types
split actors into separate crates, this will allow users to easily create their own actors