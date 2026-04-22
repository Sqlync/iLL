// Runtime half of the `args_actor`. Holds the resolved member variables
// computed at construct time from declared defaults + `--arg KEY=VALUE`
// overrides. All members are public read-only; `check` is a no-op command
// (the following asserts in the `as` block do the real work).

use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, Dict, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

#[derive(Debug)]
pub struct ArgsActorInstance {
    members: Dict,
}

impl ArgsActorInstance {
    pub fn construct(args: &ConstructArgs) -> Result<Self, RuntimeError> {
        // Unknown `--arg` keys error loudly rather than silently dropping —
        // a typo would otherwise look like the default was used.
        let declared: std::collections::HashSet<&str> =
            args.vars.iter().map(|v| v.name.as_str()).collect();
        for key in args.cli_args.keys() {
            if !declared.contains(key.as_str()) {
                return Err(RuntimeError::Construct(format!(
                    "--arg `{key}` is not a declared var on this args_actor"
                )));
            }
        }

        let mut members = Dict::new();
        for var in &args.vars {
            let value = match (args.cli_args.get(&var.name), &var.default) {
                (Some(raw), Some(default)) => coerce_to(raw, default, &var.name)?,
                (Some(raw), None) => Value::String(raw.clone()),
                (None, Some(default)) => default.clone(),
                (None, None) => {
                    return Err(RuntimeError::Construct(format!(
                        "missing required --arg `{}`",
                        var.name
                    )));
                }
            };
            members.insert(var.name.clone(), value);
        }

        Ok(Self { members })
    }
}

/// Coerce a raw CLI string into the same `Value` variant as `target`. Used
/// only when a declared var has a default — the default fixes the expected
/// type. Fails loudly if the raw string can't be parsed; the user said
/// "error if they do not translate".
fn coerce_to(raw: &str, target: &Value, var_name: &str) -> Result<Value, RuntimeError> {
    match target {
        Value::String(_) => Ok(Value::String(raw.to_string())),
        Value::Number(_) => raw.parse::<i64>().map(Value::Number).map_err(|_| {
            RuntimeError::Construct(format!(
                "--arg `{var_name}`: cannot parse `{raw}` as a number"
            ))
        }),
        Value::Bool(_) => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(RuntimeError::Construct(format!(
                "--arg `{var_name}`: expected `true` or `false`, got `{other}`"
            ))),
        },
        Value::Atom(_) => {
            let atom = raw.strip_prefix(':').unwrap_or(raw);
            Ok(Value::Atom(atom.to_string()))
        }
        other => Err(RuntimeError::Construct(format!(
            "--arg `{var_name}`: default has type {}, which cannot be set from the command line",
            other.type_name()
        ))),
    }
}

#[async_trait::async_trait]
impl ActorInstance for ArgsActorInstance {
    fn type_name(&self) -> &'static str {
        "args_actor"
    }

    async fn execute(&mut self, cmd: &'static str, _args: &CommandArgs) -> RunOutcome {
        match cmd {
            // `check` is a marker — the asserts that follow in the `as` block
            // do the actual validation.
            "check" => RunOutcome::Ok(Dict::new()),
            other => RunOutcome::NotImplemented {
                actor: "args_actor",
                cmd: other,
            },
        }
    }

    async fn teardown(&mut self) -> TeardownOutcome {
        TeardownOutcome::ok()
    }

    fn self_view(&self) -> Option<Dict> {
        Some(self.members.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::DeclaredVar;

    fn base_args(vars: Vec<DeclaredVar>) -> ConstructArgs {
        ConstructArgs {
            vars,
            ..Default::default()
        }
    }

    fn var(name: &str, default: Option<Value>) -> DeclaredVar {
        DeclaredVar {
            name: name.into(),
            default,
        }
    }

    #[test]
    fn missing_required_errors() {
        let args = base_args(vec![var("required", None)]);
        let err = ArgsActorInstance::construct(&args).unwrap_err();
        assert!(
            matches!(&err, RuntimeError::Construct(m) if m.contains("missing required")),
            "got: {err:?}"
        );
    }

    #[test]
    fn required_uses_cli_override() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("required".into(), "hello".into());
        let args = ConstructArgs {
            vars: vec![var("required", None)],
            cli_args: cli,
            ..Default::default()
        };
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(
            inst.members.get("required"),
            Some(&Value::String("hello".into()))
        );
    }

    #[test]
    fn default_used_when_no_override() {
        let args = base_args(vec![var("opt", Some(Value::String("foo".into())))]);
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("opt"), Some(&Value::String("foo".into())));
    }

    #[test]
    fn number_default_coerces_cli_string() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("n".into(), "42".into());
        let args = ConstructArgs {
            vars: vec![var("n", Some(Value::Number(7)))],
            cli_args: cli,
            ..Default::default()
        };
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("n"), Some(&Value::Number(42)));
    }

    #[test]
    fn number_coercion_failure_errors() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("n".into(), "not-a-number".into());
        let args = ConstructArgs {
            vars: vec![var("n", Some(Value::Number(7)))],
            cli_args: cli,
            ..Default::default()
        };
        let err = ArgsActorInstance::construct(&args).unwrap_err();
        assert!(matches!(&err, RuntimeError::Construct(m) if m.contains("cannot parse")));
    }

    #[test]
    fn bool_coercion() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("flag".into(), "true".into());
        let args = ConstructArgs {
            vars: vec![var("flag", Some(Value::Bool(false)))],
            cli_args: cli,
            ..Default::default()
        };
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("flag"), Some(&Value::Bool(true)));
    }

    #[test]
    fn bool_coercion_failure_errors() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("flag".into(), "yes".into());
        let args = ConstructArgs {
            vars: vec![var("flag", Some(Value::Bool(false)))],
            cli_args: cli,
            ..Default::default()
        };
        assert!(ArgsActorInstance::construct(&args).is_err());
    }

    #[test]
    fn atom_coercion_strips_colon() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("mode".into(), ":fast".into());
        let args = ConstructArgs {
            vars: vec![var("mode", Some(Value::Atom("slow".into())))],
            cli_args: cli,
            ..Default::default()
        };
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("mode"), Some(&Value::Atom("fast".into())));
    }

    #[test]
    fn unknown_cli_arg_errors() {
        let mut cli = std::collections::BTreeMap::new();
        cli.insert("nope".into(), "x".into());
        let args = ConstructArgs {
            vars: vec![var("opt", Some(Value::String("foo".into())))],
            cli_args: cli,
            ..Default::default()
        };
        let err = ArgsActorInstance::construct(&args).unwrap_err();
        assert!(matches!(&err, RuntimeError::Construct(m) if m.contains("not a declared var")));
    }

    #[tokio::test]
    async fn self_view_exposes_members() {
        let args = base_args(vec![var("x", Some(Value::Number(1)))]);
        let inst = ArgsActorInstance::construct(&args).unwrap();
        let view = inst.self_view().expect("self_view populated");
        assert_eq!(view.get("x"), Some(&Value::Number(1)));
    }

    #[tokio::test]
    async fn check_command_is_ok() {
        let args = base_args(vec![]);
        let mut inst = ArgsActorInstance::construct(&args).unwrap();
        let outcome = inst
            .execute(
                "check",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: Dict::new(),
                },
            )
            .await;
        assert!(matches!(outcome, RunOutcome::Ok(_)));
    }
}
