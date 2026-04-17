// Runtime half of the `exec` actor. Spawns the configured command as a child
// process. `run` is spawn-only — the process stays alive until teardown sends
// SIGTERM / SIGKILL. Stdout/stderr inherit from the runner; a bounded-buffer
// capture mechanism is tracked in ROADMAP (Deferred).

use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};

use crate::actor_type::ActorInstance;
use crate::runtime::{RunOutcome, RuntimeError, SpawnArgs, TeardownOutcome, Value};

/// Time between SIGTERM and SIGKILL during teardown.
const TEARDOWN_GRACE: Duration = Duration::from_secs(2);

pub struct ExecInstance {
    command: String,
    source_dir: PathBuf,
    child: Option<Child>,
}

impl ExecInstance {
    pub fn spawn(args: &SpawnArgs) -> Result<Self, RuntimeError> {
        let command = match args.kw("command") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(RuntimeError::TypeMismatch {
                    expected: "string",
                    got: other.type_name(),
                    context: "exec `command`".into(),
                });
            }
            None => return Err(RuntimeError::MissingKwarg { name: "command" }),
        };

        Ok(Self {
            command,
            source_dir: args.source_dir.clone(),
            child: None,
        })
    }

    /// Start the process. Non-blocking: returns as soon as the child is
    /// spawned. `ok.pid` carries the process id for later assertions.
    pub fn run(&mut self, env: Option<&Value>) -> RunOutcome {
        if self.child.is_some() {
            return RunOutcome::error(1, "exec process already running");
        }

        let parts = match shlex::split(&self.command) {
            Some(p) if !p.is_empty() => p,
            _ => return RunOutcome::error(1, format!("invalid command: {:?}", self.command)),
        };

        let (program, rest) = parts.split_first().unwrap();
        let resolved = resolve_program(program, &self.source_dir);

        let mut cmd = StdCommand::new(&resolved);
        cmd.args(rest).current_dir(&self.source_dir);

        if let Some(env_val) = env {
            if let Err(e) = apply_env(&mut cmd, env_val) {
                return RunOutcome::error(1, e);
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return RunOutcome::error(1, format!("spawn failed: {e}")),
        };

        let pid = child.id();
        self.child = Some(child);

        let mut ok = BTreeMap::new();
        ok.insert("pid".into(), Value::Number(pid as i64));
        RunOutcome::Ok(ok)
    }
}

impl ActorInstance for ExecInstance {
    fn type_name(&self) -> &'static str {
        "exec"
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn teardown(&mut self) -> TeardownOutcome {
        let Some(mut child) = self.child.take() else {
            return TeardownOutcome::ok();
        };

        // Try a graceful stop first.
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }

        let deadline = Instant::now() + TEARDOWN_GRACE;
        let mut outcome = TeardownOutcome::ok();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return outcome,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    outcome = TeardownOutcome::failed(format!("wait failed: {e}"));
                    break;
                }
            }
        }

        if let Err(e) = child.kill() {
            // ESRCH means the child exited between try_wait and kill — not an error.
            if e.kind() != std::io::ErrorKind::NotFound {
                outcome = TeardownOutcome::failed(format!("kill failed: {e}"));
            }
        }
        let _ = child.wait();
        outcome
    }
}

impl Drop for ExecInstance {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.teardown();
        }
    }
}

fn resolve_program(program: &str, source_dir: &Path) -> PathBuf {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Bare name (no separator) → PATH lookup by std::process::Command.
    if !program.contains('/') && !program.contains('\\') {
        return PathBuf::from(program);
    }
    // Relative path → resolve from the .ill file's directory.
    source_dir.join(program)
}

fn apply_env(cmd: &mut StdCommand, env: &Value) -> Result<(), String> {
    match env {
        Value::Record(fields) => {
            for (k, v) in fields {
                let s = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Atom(a) => a.clone(),
                    other => {
                        return Err(format!(
                            "env value for `{k}` is {} (expected string-like)",
                            other.type_name()
                        ));
                    }
                };
                cmd.env(k, s);
            }
            Ok(())
        }
        other => Err(format!("`env` must be a record, got {}", other.type_name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_args(command: &str) -> SpawnArgs {
        let mut kw = BTreeMap::new();
        kw.insert("command".into(), Value::String(command.into()));
        SpawnArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
        }
    }

    #[test]
    fn missing_command_kwarg_errors() {
        let args = SpawnArgs {
            keyword: BTreeMap::new(),
            source_dir: std::env::temp_dir(),
        };
        let err = match ExecInstance::spawn(&args) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(matches!(err, RuntimeError::MissingKwarg { .. }));
    }

    #[test]
    fn run_spawns_and_populates_pid() {
        // `sleep 60` stays alive long enough for teardown to exercise SIGTERM.
        let mut inst = ExecInstance::spawn(&spawn_args("sleep 60")).unwrap();
        let outcome = inst.run(None);
        match outcome {
            RunOutcome::Ok(fields) => {
                let pid = fields.get("pid").expect("pid field");
                assert!(matches!(pid, Value::Number(n) if *n > 0));
            }
            RunOutcome::Error(f) => panic!("expected Ok, got Error: {f:?}"),
            RunOutcome::NotImplemented { .. } => panic!("expected Ok"),
        }
        let td = inst.teardown();
        assert!(td.ok, "teardown failed: {:?}", td.message);
    }

    #[test]
    fn double_run_returns_error() {
        let mut inst = ExecInstance::spawn(&spawn_args("sleep 60")).unwrap();
        let _ = inst.run(None);
        let second = inst.run(None);
        assert!(matches!(second, RunOutcome::Error(_)));
        let _ = inst.teardown();
    }

    #[test]
    fn ok_fields_match_command_declaration() {
        use crate::actor_type::exec::commands::RUN;
        let mut inst = ExecInstance::spawn(&spawn_args("sleep 60")).unwrap();
        let outcome = inst.run(None);
        let RunOutcome::Ok(fields) = outcome else {
            panic!("expected ok");
        };
        let declared: Vec<&'static str> = RUN.ok_fields().iter().map(|f| f.name).collect();
        let returned: Vec<&str> = fields.keys().map(|s| s.as_str()).collect();
        assert_eq!(declared, returned);
        let _ = inst.teardown();
    }
}
