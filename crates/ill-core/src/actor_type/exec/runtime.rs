// Runtime half of the `exec` actor. Modelled as a state machine over the
// actor's modes: `Stopped` carries only declaration data, `Running` owns the
// live child. Transitions happen inside `execute` / `teardown` — invalid
// operations in the wrong mode are rejected by pattern match rather than an
// implicit flag check. Stdout/stderr inherit from the runner; a bounded-buffer
// capture mechanism is tracked in ROADMAP (Deferred).

use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};

use super::commands::RunOk;
use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

/// Time between SIGTERM and SIGKILL during teardown.
const TEARDOWN_GRACE: Duration = Duration::from_secs(2);

pub enum ExecInstance {
    Stopped(Stopped),
    Running(Running),
}

pub struct Stopped {
    command: String,
    source_dir: PathBuf,
}

pub struct Running {
    command: String,
    source_dir: PathBuf,
    child: Child,
}

impl ExecInstance {
    pub fn construct(args: &ConstructArgs) -> Result<Self, RuntimeError> {
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

        Ok(Self::Stopped(Stopped {
            command,
            source_dir: args.source_dir.clone(),
        }))
    }

    // Cheap `Stopped` used as a scratch value for `mem::replace` so we can
    // move `self` by value into the per-mode handlers. Never observable —
    // `execute` / `teardown` always write a real value back before returning.
    fn placeholder() -> Self {
        Self::Stopped(Stopped {
            command: String::new(),
            source_dir: PathBuf::new(),
        })
    }
}

impl Stopped {
    /// Spawn the configured command. On success, transitions to `Running`;
    /// on any pre-spawn or spawn error, stays in `Stopped`.
    fn run(self, env: Option<&Value>) -> (ExecInstance, RunOutcome) {
        let parts = match shlex::split(&self.command) {
            Some(p) if !p.is_empty() => p,
            _ => {
                let msg = format!("invalid command: {:?}", self.command);
                return (ExecInstance::Stopped(self), RunOutcome::error(1, msg));
            }
        };

        let (program, rest) = parts.split_first().unwrap();
        let resolved = resolve_program(program, &self.source_dir);

        let mut cmd = StdCommand::new(&resolved);
        cmd.args(rest).current_dir(&self.source_dir);

        if let Some(env_val) = env {
            if let Err(e) = apply_env(&mut cmd, env_val) {
                return (ExecInstance::Stopped(self), RunOutcome::error(1, e));
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("spawn failed: {e}");
                return (ExecInstance::Stopped(self), RunOutcome::error(1, msg));
            }
        };

        let pid = child.id();
        let running = Running {
            command: self.command,
            source_dir: self.source_dir,
            child,
        };
        (
            ExecInstance::Running(running),
            RunOutcome::Ok(RunOk { pid: pid as i64 }.into_record()),
        )
    }
}

impl Running {
    /// SIGTERM, wait up to `TEARDOWN_GRACE`, then SIGKILL. Always transitions
    /// back to `Stopped` so a second teardown is a no-op.
    fn stop(mut self) -> (ExecInstance, TeardownOutcome) {
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        let deadline = Instant::now() + TEARDOWN_GRACE;
        let mut outcome = TeardownOutcome::ok();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return (self.into_stopped(), outcome),
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

        // `Child::kill` is Ok if the child has already exited, so any Err here
        // is a genuine failure (e.g. permission denied).
        if let Err(e) = self.child.kill() {
            outcome = TeardownOutcome::failed(format!("kill failed: {e}"));
        }
        let _ = self.child.wait();
        (self.into_stopped(), outcome)
    }

    fn into_stopped(self) -> ExecInstance {
        ExecInstance::Stopped(Stopped {
            command: self.command,
            source_dir: self.source_dir,
        })
    }
}

impl ActorInstance for ExecInstance {
    fn type_name(&self) -> &'static str {
        "exec"
    }

    fn execute(&mut self, cmd: &'static str, args: &CommandArgs) -> RunOutcome {
        let taken = std::mem::replace(self, Self::placeholder());
        let (next, outcome) = match (taken, cmd) {
            (Self::Stopped(s), "run") => s.run(args.kw("env")),
            (Self::Running(r), "run") => (
                Self::Running(r),
                RunOutcome::error(1, "exec process already running"),
            ),
            (other, cmd) => (other, RunOutcome::NotImplemented { actor: "exec", cmd }),
        };
        *self = next;
        outcome
    }

    fn teardown(&mut self) -> TeardownOutcome {
        let taken = std::mem::replace(self, Self::placeholder());
        let (next, outcome) = match taken {
            Self::Stopped(s) => (Self::Stopped(s), TeardownOutcome::ok()),
            Self::Running(r) => r.stop(),
        };
        *self = next;
        outcome
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
    use std::collections::BTreeMap;

    fn construct_args(command: &str) -> ConstructArgs {
        let mut kw = BTreeMap::new();
        kw.insert("command".into(), Value::String(command.into()));
        ConstructArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
        }
    }

    fn empty_args() -> CommandArgs {
        CommandArgs {
            positional: Vec::new(),
            keyword: BTreeMap::new(),
        }
    }

    #[test]
    fn missing_command_kwarg_errors() {
        let args = ConstructArgs {
            keyword: BTreeMap::new(),
            source_dir: std::env::temp_dir(),
        };
        let err = match ExecInstance::construct(&args) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(matches!(err, RuntimeError::MissingKwarg { .. }));
    }

    #[test]
    fn run_spawns_and_populates_pid() {
        // `sleep 60` stays alive long enough for teardown to exercise SIGTERM.
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        assert!(matches!(inst, ExecInstance::Stopped(_)));

        let outcome = inst.execute("run", &empty_args());
        match outcome {
            RunOutcome::Ok(fields) => {
                let pid = fields.get("pid").expect("pid field");
                assert!(matches!(pid, Value::Number(n) if *n > 0));
            }
            RunOutcome::Error(f) => panic!("expected Ok, got Error: {f:?}"),
            RunOutcome::NotImplemented { .. } => panic!("expected Ok"),
        }
        assert!(matches!(inst, ExecInstance::Running(_)));

        let td = inst.teardown();
        assert!(td.ok, "teardown failed: {:?}", td.message);
        assert!(matches!(inst, ExecInstance::Stopped(_)));
    }

    #[test]
    fn double_run_returns_error() {
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let _ = inst.execute("run", &empty_args());
        let second = inst.execute("run", &empty_args());
        assert!(matches!(second, RunOutcome::Error(_)));
        let _ = inst.teardown();
    }

    #[test]
    fn teardown_when_stopped_is_noop() {
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let td = inst.teardown();
        assert!(td.ok);
        // Second teardown is still a no-op.
        let td = inst.teardown();
        assert!(td.ok);
    }

    #[test]
    #[cfg(unix)]
    fn teardown_sigkills_sigterm_ignoring_child() {
        // bash traps TERM and sleeps; teardown must escalate to SIGKILL after
        // TEARDOWN_GRACE (2s) and still report ok.
        let mut inst =
            ExecInstance::construct(&construct_args("bash -c 'trap \"\" TERM; sleep 60'")).unwrap();
        let outcome = inst.execute("run", &empty_args());
        assert!(matches!(outcome, RunOutcome::Ok(_)));

        // Give bash time to install the trap before we send SIGTERM, otherwise
        // the signal races with bash's startup and the test measures the wrong
        // path.
        std::thread::sleep(Duration::from_millis(300));

        let start = std::time::Instant::now();
        let td = inst.teardown();
        let elapsed = start.elapsed();

        assert!(td.ok, "teardown failed: {:?}", td.message);
        assert!(
            elapsed >= TEARDOWN_GRACE,
            "teardown returned before grace period: {elapsed:?}"
        );
        assert!(
            elapsed < TEARDOWN_GRACE + Duration::from_secs(2),
            "teardown took too long: {elapsed:?}"
        );
    }
}
