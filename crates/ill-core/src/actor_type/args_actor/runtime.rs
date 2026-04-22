// Runtime half of the `args_actor`. Holds the resolved member variables
// computed at construct time from declared defaults + `--arg KEY=VALUE`
// overrides. `check` is a no-op; the following asserts in the `as` block
// do the real work.

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
        // A typo in a `--arg` key would otherwise silently fall through to
        // the default and look like the default was used.
        for key in args.cli_args.keys() {
            if !args.vars.iter().any(|v| &v.name == key) {
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
/// type. Non-scalar defaults (dict, array, bytes) can't be overridden from
/// the command line.
fn coerce_to(raw: &str, target: &Value, var_name: &str) -> Result<Value, RuntimeError> {
    match target {
        Value::String(_) => Ok(Value::String(raw.to_string())),
        Value::Number(_) => raw.parse::<i64>().map(Value::Number).map_err(|_| {
            RuntimeError::Construct(format!(
                "--arg `{var_name}`: cannot parse `{raw}` as a number"
            ))
        }),
        Value::Bool(_) => raw.parse::<bool>().map(Value::Bool).map_err(|_| {
            RuntimeError::Construct(format!(
                "--arg `{var_name}`: expected `true` or `false`, got `{raw}`"
            ))
        }),
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

    fn args_with(vars: Vec<DeclaredVar>, cli: &[(&str, &str)]) -> ConstructArgs {
        let cli_args = cli
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        ConstructArgs {
            vars,
            cli_args,
            ..Default::default()
        }
    }

    fn base_args(vars: Vec<DeclaredVar>) -> ConstructArgs {
        args_with(vars, &[])
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
        let args = args_with(vec![var("required", None)], &[("required", "hello")]);
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
        let args = args_with(vec![var("n", Some(Value::Number(7)))], &[("n", "42")]);
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("n"), Some(&Value::Number(42)));
    }

    #[test]
    fn number_coercion_failure_errors() {
        let args = args_with(
            vec![var("n", Some(Value::Number(7)))],
            &[("n", "not-a-number")],
        );
        let err = ArgsActorInstance::construct(&args).unwrap_err();
        assert!(matches!(&err, RuntimeError::Construct(m) if m.contains("cannot parse")));
    }

    #[test]
    fn bool_coercion() {
        let args = args_with(
            vec![var("flag", Some(Value::Bool(false)))],
            &[("flag", "true")],
        );
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("flag"), Some(&Value::Bool(true)));
    }

    #[test]
    fn bool_coercion_failure_errors() {
        let args = args_with(
            vec![var("flag", Some(Value::Bool(false)))],
            &[("flag", "yes")],
        );
        let err = ArgsActorInstance::construct(&args).unwrap_err();
        assert!(matches!(&err, RuntimeError::Construct(m) if m.contains("expected `true`")));
    }

    #[test]
    fn atom_coercion_strips_colon() {
        let args = args_with(
            vec![var("mode", Some(Value::Atom("slow".into())))],
            &[("mode", ":fast")],
        );
        let inst = ArgsActorInstance::construct(&args).unwrap();
        assert_eq!(inst.members.get("mode"), Some(&Value::Atom("fast".into())));
    }

    #[test]
    fn unknown_cli_arg_errors() {
        let args = args_with(
            vec![var("opt", Some(Value::String("foo".into())))],
            &[("nope", "x")],
        );
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
