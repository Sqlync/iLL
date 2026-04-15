// Global registry of actor types.
//
// Built once via `Registry::global()`. Phase 4 reads it for validation;
// Phase 5 will extend each entry with runtime execution; Phase 6 reads it
// for LSP completions.
//
// Cross-actor consistency (a command's referenced modes belong to the same
// actor as the command) is asserted at registry-build time. The unit test at
// the bottom ensures `cargo test` catches buggy actor impls.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::actor_type::{args_actor, container, http_client, mqtt_client, pg_client, ActorType};

pub struct Registry {
    actors: HashMap<&'static str, &'static dyn ActorType>,
}

impl Registry {
    pub fn global() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(Registry::build)
    }

    fn build() -> Registry {
        let mut r = Registry {
            actors: HashMap::new(),
        };
        r.register(pg_client::PG_CLIENT);
        r.register(container::CONTAINER);
        r.register(http_client::HTTP_CLIENT);
        r.register(mqtt_client::MQTT_CLIENT);
        r.register(args_actor::ARGS_ACTOR);
        r.validate();
        r
    }

    fn register(&mut self, a: &'static dyn ActorType) {
        let prev = self.actors.insert(a.name(), a);
        assert!(prev.is_none(), "duplicate actor type: {}", a.name());
    }

    pub fn get(&self, name: &str) -> Option<&'static dyn ActorType> {
        self.actors.get(name).copied()
    }

    pub fn actor_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.actors.keys().copied()
    }

    /// Assert every command's referenced modes belong to its actor type.
    ///
    /// This is the runtime substitute for the compile-time check we'd get from
    /// associated types, paid once at startup. A buggy impl would otherwise
    /// silently pass validation with wrong-actor modes.
    fn validate(&self) {
        for (actor_name, actor) in &self.actors {
            assert_eq!(
                actor.initial_mode().actor_type(),
                *actor_name,
                "actor `{}` has initial mode `{}` from actor `{}`",
                actor_name,
                actor.initial_mode().name(),
                actor.initial_mode().actor_type(),
            );

            for m in actor.modes() {
                assert_eq!(
                    m.actor_type(),
                    *actor_name,
                    "actor `{}` lists mode `{}` from actor `{}`",
                    actor_name,
                    m.name(),
                    m.actor_type(),
                );
            }

            for cmd in actor.commands() {
                for m in cmd.valid_in_modes() {
                    assert_eq!(
                        m.actor_type(),
                        *actor_name,
                        "command `{}` on actor `{}` references mode `{}` from actor `{}`",
                        cmd.name(),
                        actor_name,
                        m.name(),
                        m.actor_type(),
                    );
                }
                if let Some(m) = cmd.transitions_to() {
                    assert_eq!(
                        m.actor_type(),
                        *actor_name,
                        "command `{}` on actor `{}` transitions to mode `{}` from actor `{}`",
                        cmd.name(),
                        actor_name,
                        m.name(),
                        m.actor_type(),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_builds_and_validates() {
        // Touching the global registry runs `validate()`. If any actor type
        // impl references a wrong-actor mode, this test panics.
        let r = Registry::global();

        let pg = r.get("pg_client").expect("pg_client registered");
        assert_eq!(pg.initial_mode().name(), "disconnected");

        let connect = pg.command("connect").expect("connect command");
        assert_eq!(
            connect.transitions_to().map(|m| m.name()),
            Some("connected")
        );

        assert!(pg.command("nonexistent").is_none());
    }
}
