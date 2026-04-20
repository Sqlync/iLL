// Runtime half of the `exec` actor. Modelled as a state machine over the
// actor's modes: `Stopped` carries no runtime data, `Running` owns the live
// child. Shared actor identity (the target shell string, the source dir)
// lives on `ExecInstance`; valid iLL commands per mode live as methods on
// the mode variants. Invalid operations in the wrong mode are rejected by
// pattern match rather than an implicit flag check. Stdout/stderr inherit
// from the runner; a bounded-buffer capture mechanism is tracked in
// DEFERRED.md.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::{Child, Command as TokioCommand};

use super::commands::{ExecError, RunOk};
use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

/// Reason atoms surfaced on `error.exec.reason` for `run` failures. Kept as
/// string literals so the macro-generated outcome type stays plain.
const REASON_INVALID_COMMAND: &str = "invalid_command";
const REASON_COMMAND_NOT_FOUND: &str = "command_not_found";
const REASON_PERMISSION_DENIED: &str = "permission_denied";
const REASON_SPAWN_FAILED: &str = "spawn_failed";
const REASON_BAD_ENV: &str = "bad_env";
const REASON_ALREADY_RUNNING: &str = "already_running";

fn run_error(reason: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "exec",
        fields: ExecError {
            reason: reason.into(),
        }
        .into_record(),
    }
}

fn classify_spawn_error(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NotFound => REASON_COMMAND_NOT_FOUND,
        io::ErrorKind::PermissionDenied => REASON_PERMISSION_DENIED,
        _ => REASON_SPAWN_FAILED,
    }
}

/// Time between SIGTERM and SIGKILL during teardown.
const TEARDOWN_GRACE: Duration = Duration::from_secs(2);

pub struct ExecInstance {
    target: String,
    source_dir: PathBuf,
    mode: ExecMode,
}

pub enum ExecMode {
    Stopped(Stopped),
    Running(Running),
}

impl Default for ExecMode {
    fn default() -> Self {
        ExecMode::Stopped(Stopped)
    }
}

pub struct Stopped;

pub struct Running {
    /// The spawned child. Tokio's `.kill_on_drop(true)` on the Command (set
    /// at spawn time in `Stopped::run`) means a panic that drops this field
    /// before teardown runs will still SIGKILL + reap the child via the
    /// runtime's child reaper. Happy-path teardown via `Running::stop`
    /// explicitly SIGTERMs, waits, and only SIGKILLs on timeout.
    child: Child,
}

impl ExecInstance {
    pub fn construct(args: &ConstructArgs) -> Result<Self, RuntimeError> {
        let target = match args.kw("command") {
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
            target,
            source_dir: args.source_dir.clone(),
            mode: ExecMode::Stopped(Stopped),
        })
    }
}

impl Stopped {
    async fn execute(
        self,
        target: &str,
        source_dir: &Path,
        cmd: &'static str,
        args: &CommandArgs,
    ) -> (ExecMode, RunOutcome) {
        match cmd {
            "run" => self.run(target, source_dir, args.kw("env")).await,
            other => (
                ExecMode::Stopped(self),
                RunOutcome::NotImplemented {
                    actor: "exec",
                    cmd: other,
                },
            ),
        }
    }

    /// Spawn the configured target. On success, transitions to `Running`;
    /// on any pre-spawn or spawn error, stays in `Stopped`.
    async fn run(
        self,
        target: &str,
        source_dir: &Path,
        env: Option<&Value>,
    ) -> (ExecMode, RunOutcome) {
        let parts = match shlex::split(target) {
            Some(p) if !p.is_empty() => p,
            _ => {
                return (ExecMode::Stopped(self), run_error(REASON_INVALID_COMMAND));
            }
        };

        let (program, rest) = parts.split_first().unwrap();
        let resolved = resolve_program(program, source_dir);

        let mut cmd = TokioCommand::new(&resolved);
        cmd.args(rest).current_dir(source_dir).kill_on_drop(true);

        if let Some(env_val) = env {
            if apply_env(&mut cmd, env_val).is_err() {
                return (ExecMode::Stopped(self), run_error(REASON_BAD_ENV));
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let reason = classify_spawn_error(&e);
                return (ExecMode::Stopped(self), run_error(reason));
            }
        };

        // `Child::id()` returns `None` after the child has been awaited;
        // right after spawn it's always `Some`.
        let pid = child.id().unwrap_or(0);
        (
            ExecMode::Running(Running { child }),
            RunOutcome::Ok(RunOk { pid: pid as i64 }.into_record()),
        )
    }
}

impl Running {
    async fn execute(self, cmd: &'static str, _args: &CommandArgs) -> (ExecMode, RunOutcome) {
        match cmd {
            "run" => (ExecMode::Running(self), run_error(REASON_ALREADY_RUNNING)),
            other => (
                ExecMode::Running(self),
                RunOutcome::NotImplemented {
                    actor: "exec",
                    cmd: other,
                },
            ),
        }
    }

    /// SIGTERM, wait up to `TEARDOWN_GRACE`, then SIGKILL. Always transitions
    /// back to `Stopped` so a second teardown is a no-op.
    async fn stop(mut self) -> (ExecMode, TeardownOutcome) {
        // Send SIGTERM. `libc::kill` is a syscall, not I/O — safe to call
        // from async context.
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }

        // Await exit up to the grace period. If the child exits cleanly
        // within the window, we're done. On timeout, escalate to SIGKILL.
        match tokio::time::timeout(TEARDOWN_GRACE, self.child.wait()).await {
            Ok(Ok(_)) => (ExecMode::Stopped(Stopped), TeardownOutcome::ok()),
            Ok(Err(e)) => (
                ExecMode::Stopped(Stopped),
                TeardownOutcome::failed(format!("wait failed: {e}")),
            ),
            Err(_timeout) => {
                // SIGKILL + reap. `start_kill` is Ok if the child has
                // already exited, so any Err here is a genuine failure.
                let mut outcome = TeardownOutcome::ok();
                if let Err(e) = self.child.start_kill() {
                    outcome = TeardownOutcome::failed(format!("kill failed: {e}"));
                }
                let _ = self.child.wait().await;
                (ExecMode::Stopped(Stopped), outcome)
            }
        }
    }
}

#[async_trait::async_trait]
impl ActorInstance for ExecInstance {
    fn type_name(&self) -> &'static str {
        "exec"
    }

    async fn execute(&mut self, cmd: &'static str, args: &CommandArgs) -> RunOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            ExecMode::Stopped(s) => s.execute(&self.target, &self.source_dir, cmd, args).await,
            ExecMode::Running(r) => r.execute(cmd, args).await,
        };
        self.mode = next;
        outcome
    }

    async fn teardown(&mut self) -> TeardownOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            ExecMode::Stopped(s) => (ExecMode::Stopped(s), TeardownOutcome::ok()),
            ExecMode::Running(r) => r.stop().await,
        };
        self.mode = next;
        outcome
    }
}

fn resolve_program(program: &str, source_dir: &Path) -> PathBuf {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Bare name (no separator) → PATH lookup by tokio::process::Command.
    if !program.contains('/') && !program.contains('\\') {
        return PathBuf::from(program);
    }
    // Relative path → resolve from the .ill file's directory.
    source_dir.join(program)
}

fn apply_env(cmd: &mut TokioCommand, env: &Value) -> Result<(), String> {
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

    fn construct_args(target: &str) -> ConstructArgs {
        let mut kw = BTreeMap::new();
        kw.insert("command".into(), Value::String(target.into()));
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

    #[tokio::test]
    async fn run_spawns_and_populates_pid() {
        // `sleep 60` stays alive long enough for teardown to exercise SIGTERM.
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        assert!(matches!(inst.mode, ExecMode::Stopped(_)));

        let outcome = inst.execute("run", &empty_args()).await;
        match outcome {
            RunOutcome::Ok(fields) => {
                let pid = fields.get("pid").expect("pid field");
                assert!(matches!(pid, Value::Number(n) if *n > 0));
            }
            RunOutcome::Error { variant, fields } => {
                panic!("expected Ok, got Error: variant={variant}, fields={fields:?}")
            }
            RunOutcome::NotImplemented { .. } => panic!("expected Ok"),
        }
        assert!(matches!(inst.mode, ExecMode::Running(_)));

        let td = inst.teardown().await;
        assert!(td.ok, "teardown failed: {:?}", td.message);
        assert!(matches!(inst.mode, ExecMode::Stopped(_)));
    }

    fn assert_exec_reason(outcome: &RunOutcome, expected: &str) {
        let (variant, fields) = match outcome {
            RunOutcome::Error {
                variant, fields, ..
            } => (*variant, fields),
            RunOutcome::Ok(_) => panic!("expected Error, got Ok"),
            RunOutcome::NotImplemented { .. } => panic!("expected Error, got NotImplemented"),
        };
        assert_eq!(variant, "exec", "expected error.exec variant");
        match fields.get("reason") {
            Some(Value::Atom(a)) => assert_eq!(a, expected, "error.exec.reason mismatch"),
            other => panic!("expected error.exec.reason atom, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn double_run_reports_already_running() {
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let _ = inst.execute("run", &empty_args()).await;
        let second = inst.execute("run", &empty_args()).await;
        assert_exec_reason(&second, "already_running");
        let _ = inst.teardown().await;
    }

    #[tokio::test]
    async fn nonexistent_program_reports_command_not_found() {
        let mut inst =
            ExecInstance::construct(&construct_args("definitely_not_a_real_program_xyz")).unwrap();
        let outcome = inst.execute("run", &empty_args()).await;
        assert_exec_reason(&outcome, "command_not_found");
        assert!(matches!(inst.mode, ExecMode::Stopped(_)));
    }

    #[tokio::test]
    async fn empty_command_reports_invalid_command() {
        let mut inst = ExecInstance::construct(&construct_args("   ")).unwrap();
        let outcome = inst.execute("run", &empty_args()).await;
        assert_exec_reason(&outcome, "invalid_command");
    }

    #[tokio::test]
    async fn non_record_env_reports_bad_env() {
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let mut kw = BTreeMap::new();
        kw.insert("env".into(), Value::Number(42));
        let args = CommandArgs {
            positional: Vec::new(),
            keyword: kw,
        };
        let outcome = inst.execute("run", &args).await;
        assert_exec_reason(&outcome, "bad_env");
        assert!(matches!(inst.mode, ExecMode::Stopped(_)));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn non_executable_file_reports_permission_denied() {
        // Create a non-executable regular file and point the actor at it.
        // `execve` on a non-executable file returns EACCES, which io::Error
        // surfaces as `PermissionDenied`.
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!(
            "ill-exec-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not_executable");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\necho hi").unwrap();

        let target = path.to_str().unwrap().to_string();
        let mut kw = BTreeMap::new();
        kw.insert("command".into(), Value::String(target));
        let args = ConstructArgs {
            keyword: kw,
            source_dir: dir.clone(),
        };
        let mut inst = ExecInstance::construct(&args).unwrap();
        let outcome = inst.execute("run", &empty_args()).await;
        assert_exec_reason(&outcome, "permission_denied");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn dropped_running_instance_reaps_child() {
        // Spawn via the normal `run` path, then drop the instance without
        // calling `teardown`. Tokio's `.kill_on_drop(true)` on the Command
        // should still SIGKILL + reap the child via the runtime's reaper.
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let outcome = inst.execute("run", &empty_args()).await;
        let pid = match outcome {
            RunOutcome::Ok(fields) => match fields.get("pid") {
                Some(Value::Number(n)) => *n as i32,
                _ => panic!("expected numeric pid"),
            },
            RunOutcome::Error { variant, fields } => {
                panic!("expected Ok, got Error: variant={variant}, fields={fields:?}")
            }
            RunOutcome::NotImplemented { .. } => panic!("expected Ok"),
        };

        drop(inst);

        // Tokio's kill_on_drop reaps asynchronously. Poll up to 2s for the
        // kernel to forget the pid. `kill(pid, 0)` probes existence without
        // delivering a signal.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let res = unsafe { libc::kill(pid, 0) };
            if res == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("process {pid} still exists after instance drop");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn teardown_when_stopped_is_noop() {
        let mut inst = ExecInstance::construct(&construct_args("sleep 60")).unwrap();
        let td = inst.teardown().await;
        assert!(td.ok);
        // Second teardown is still a no-op.
        let td = inst.teardown().await;
        assert!(td.ok);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn teardown_sigkills_sigterm_ignoring_child() {
        // bash traps TERM and sleeps; teardown must escalate to SIGKILL after
        // TEARDOWN_GRACE (2s) and still report ok.
        let mut inst =
            ExecInstance::construct(&construct_args("bash -c 'trap \"\" TERM; sleep 60'")).unwrap();
        let outcome = inst.execute("run", &empty_args()).await;
        assert!(matches!(outcome, RunOutcome::Ok(_)));

        // Give bash time to install the trap before we send SIGTERM, otherwise
        // the signal races with bash's startup and the test measures the wrong
        // path.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let start = std::time::Instant::now();
        let td = inst.teardown().await;
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
