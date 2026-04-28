// Runtime half of the `container` actor. Modelled as a state machine over
// the actor's modes: `Stopped` carries no live docker resource; `Running`
// owns a `ContainerAsync` whose Drop removes the docker container.
//
// Image preparation is eager: at construct time we either pull the image
// (for `image:`) or build it from a Dockerfile (for `dockerfile:`), so by
// the time `run` executes the image is already on the local daemon. This
// collapses the error surface on `run` to runtime-only atoms. Image/build
// failures surface as construct failures, not run errors.

use std::path::Path;
use std::time::Duration;

use testcontainers::core::{error::WaitContainerError, IntoContainerPort};
use testcontainers::runners::{AsyncBuilder, AsyncRunner};
use testcontainers::{
    ContainerAsync, ContainerRequest, GenericBuildableImage, GenericImage, ImageExt,
    TestcontainersError,
};

use super::commands::{ContainerError, RunOk};
use crate::actor_type::ActorInstance;
use crate::runtime::{
    CommandArgs, ConstructArgs, Dict, Members, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

/// Reason atoms surfaced on `error.container.reason`. Run: `:timeout`,
/// `:already_running`, `:docker_unavailable`, `:bad_env`, `:bad_port`,
/// `:port_in_use`. Stop: `:not_running`, `:timeout`, `:docker_unavailable`.
const REASON_TIMEOUT: &str = "timeout";
const REASON_ALREADY_RUNNING: &str = "already_running";
const REASON_NOT_RUNNING: &str = "not_running";
const REASON_DOCKER_UNAVAILABLE: &str = "docker_unavailable";
const REASON_BAD_ENV: &str = "bad_env";
const REASON_BAD_PORT: &str = "bad_port";
const REASON_PORT_IN_USE: &str = "port_in_use";

/// Label every container we create so a future startup sweep (not yet
/// implemented — see ROADMAP "Docker optimizations → zombies") can find and
/// reap orphans left behind by aborts or crashes.
const LABEL_KEY: &str = "ill.test";
const LABEL_VALUE: &str = "1";

/// Default `run` startup timeout when `timeout:` is not supplied.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on `stop`/teardown. Docker's own default for SIGTERM→SIGKILL
/// escalation is 10s; match it here so the harness doesn't wait longer.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(15);

fn run_error(reason: &str) -> RunOutcome {
    RunOutcome::Error {
        variant: "container",
        fields: ContainerError {
            reason: reason.into(),
        }
        .into_dict(),
    }
}

/// Map a testcontainers runtime error (from `start()` or `rm()`) to one of
/// our atom reasons. `StartupTimeout` is structural; port-bind failures
/// surface only through Docker's error stream, so we string-match the
/// rendered message. Unrecognized errors collapse to `:docker_unavailable`.
fn classify_run_error(e: &TestcontainersError) -> &'static str {
    if matches!(
        e,
        TestcontainersError::WaitContainer(WaitContainerError::StartupTimeout)
    ) {
        return REASON_TIMEOUT;
    }
    let rendered = e.to_string();
    if rendered.contains("port is already allocated")
        || rendered.contains("address already in use")
    {
        return REASON_PORT_IN_USE;
    }
    REASON_DOCKER_UNAVAILABLE
}

pub struct ContainerInstance {
    image_name: String,
    image_tag: String,
    /// Port the process inside the container listens on. Image fact, set
    /// at construct from the `internal_port:` kwarg, paired with the
    /// per-invocation `external_port:` to drive `with_mapped_port`. `None`
    /// when the actor never publishes a port.
    internal_port: Option<u16>,
    mode: ContainerMode,
    members: Members,
}

pub enum ContainerMode {
    Stopped(Stopped),
    Running(Running),
}

impl Default for ContainerMode {
    fn default() -> Self {
        ContainerMode::Stopped(Stopped)
    }
}

pub struct Stopped;

pub struct Running {
    /// The live container. `ContainerAsync` removes the docker container
    /// when dropped (testcontainers handles the async teardown via
    /// `block_in_place` on the current runtime), so a mid-test panic that
    /// drops this field before teardown runs still cleans up.
    container: ContainerAsync<GenericImage>,
}

impl ContainerInstance {
    /// Eagerly prepare the image so that `run` has nothing left to fetch.
    /// Returns `RuntimeError::Construct` on pull/build failure — the harness
    /// surfaces this as a `ConstructFailure` in the test report.
    pub async fn construct(args: &ConstructArgs) -> Result<Self, RuntimeError> {
        let image_kw = args.kw("image");
        let dockerfile_kw = args.kw("dockerfile");
        let internal_port = match args.kw("internal_port") {
            None => None,
            Some(Value::Number(n)) => match u16::try_from(*n) {
                Ok(p) => Some(p),
                Err(_) => {
                    return Err(RuntimeError::Construct(format!(
                        "container `internal_port` {n} is out of range for a u16"
                    )))
                }
            },
            Some(other) => {
                return Err(RuntimeError::TypeMismatch {
                    expected: "number",
                    got: other.type_name(),
                    context: "container `internal_port`".into(),
                })
            }
        };
        let members = Members::from_declarations(&args.vars);

        match (image_kw, dockerfile_kw) {
            (Some(_), Some(_)) => Err(RuntimeError::Construct(
                "container requires exactly one of `image:` or `dockerfile:`, not both".to_string(),
            )),
            (None, None) => Err(RuntimeError::Construct(
                "container requires either `image:` or `dockerfile:`".to_string(),
            )),

            (Some(value), None) => match value {
                Value::String(image_ref) => {
                    prepare_from_image(image_ref, internal_port, members).await
                }
                other => Err(RuntimeError::TypeMismatch {
                    expected: "string",
                    got: other.type_name(),
                    context: "container `image`".into(),
                }),
            },

            (None, Some(value)) => match value {
                Value::String(path_ref) => {
                    prepare_from_dockerfile(path_ref, &args.source_dir, internal_port, members)
                        .await
                }
                other => Err(RuntimeError::TypeMismatch {
                    expected: "string",
                    got: other.type_name(),
                    context: "container `dockerfile`".into(),
                }),
            },
        }
    }
}

async fn prepare_from_image(
    image_ref: &str,
    internal_port: Option<u16>,
    members: Members,
) -> Result<ContainerInstance, RuntimeError> {
    let (name, tag) = split_image_ref(image_ref);

    // Eager pull. We discard the returned ContainerRequest — the image is
    // now on the local daemon and a fresh `start()` at run time will reuse
    // it without re-pulling.
    let _pulled = GenericImage::new(&name, &tag)
        .pull_image()
        .await
        .map_err(|e| RuntimeError::Construct(format!("pulling `{name}:{tag}`: {e}")))?;

    Ok(ContainerInstance {
        image_name: name,
        image_tag: tag,
        internal_port,
        mode: ContainerMode::default(),
        members,
    })
}

async fn prepare_from_dockerfile(
    dockerfile: &str,
    source_dir: &Path,
    internal_port: Option<u16>,
    members: Members,
) -> Result<ContainerInstance, RuntimeError> {
    let resolved = source_dir.join(dockerfile);
    if !resolved.is_file() {
        return Err(RuntimeError::Construct(format!(
            "dockerfile not found: {}",
            resolved.display()
        )));
    }

    // Synthesize a stable image name from the resolved path so repeat runs
    // reuse docker's layer cache.
    let tag = "latest".to_string();
    let name = synthesize_image_name(&resolved);

    // The build context is the Dockerfile's parent directory, matching
    // `docker build .` semantics so `COPY foo` finds a sibling `foo`. A
    // `.dockerignore` next to the Dockerfile is honored using Docker's
    // gitignore-style syntax — same engine `docker build` uses.
    let context_dir = resolved.parent().ok_or_else(|| {
        RuntimeError::Construct(format!(
            "dockerfile has no parent directory: {}",
            resolved.display()
        ))
    })?;

    let mut builder = GenericBuildableImage::new(&name, &tag).with_dockerfile(&resolved);
    builder = add_build_context(builder, context_dir, &resolved)?;

    let _built = builder
        .build_image()
        .await
        .map_err(|e| RuntimeError::Construct(format!("building `{}`: {e}", resolved.display())))?;

    Ok(ContainerInstance {
        image_name: name,
        image_tag: tag,
        internal_port,
        mode: ContainerMode::default(),
        members,
    })
}

/// Walk `context_dir` and add every regular file to the build context, except
/// the Dockerfile itself (already added separately) and the `.dockerignore`
/// file (it controls the build but isn't a build input). Files matched by
/// `.dockerignore` patterns are skipped.
fn add_build_context(
    mut builder: GenericBuildableImage,
    context_dir: &Path,
    dockerfile: &Path,
) -> Result<GenericBuildableImage, RuntimeError> {
    let walker = ignore::WalkBuilder::new(context_dir)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .add_custom_ignore_filename(".dockerignore")
        .build();

    let dockerfile_canon = dockerfile.canonicalize().ok();

    for entry in walker {
        let entry = entry.map_err(|e| {
            RuntimeError::Construct(format!(
                "walking build context `{}`: {e}",
                context_dir.display()
            ))
        })?;
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some(".dockerignore") {
            continue;
        }
        if let Some(df) = dockerfile_canon.as_deref() {
            if path.canonicalize().ok().as_deref() == Some(df) {
                continue;
            }
        }
        let rel = path.strip_prefix(context_dir).map_err(|_| {
            RuntimeError::Construct(format!(
                "build context entry `{}` is not under context dir `{}`",
                path.display(),
                context_dir.display()
            ))
        })?;
        builder = builder.with_file(path.to_path_buf(), rel.to_string_lossy().into_owned());
    }
    Ok(builder)
}

/// Split `"name[:tag]"` into `(name, tag)` with `tag` defaulting to `"latest"`.
fn split_image_ref(image_ref: &str) -> (String, String) {
    // Be careful with registry hosts like `ghcr.io:443/org/image:tag` — split
    // on the LAST colon, and only treat it as a tag if what follows doesn't
    // contain a `/` (which would indicate it's part of a registry path).
    if let Some(idx) = image_ref.rfind(':') {
        let (name, rest) = image_ref.split_at(idx);
        let tag = &rest[1..];
        if !tag.contains('/') && !tag.is_empty() {
            return (name.to_string(), tag.to_string());
        }
    }
    (image_ref.to_string(), "latest".to_string())
}

/// Derive a stable docker image name from a Dockerfile path. Uses the
/// default hasher — this only needs to be stable within a single build
/// session (collisions across sessions just mean we rebuild).
fn synthesize_image_name(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("ill-build-{:016x}", h.finish())
}

impl Stopped {
    async fn run(
        self,
        image_name: &str,
        image_tag: &str,
        internal_port: Option<u16>,
        external_port_kw: Option<&Value>,
        env_kw: Option<&Value>,
        timeout_kw: Option<&Value>,
    ) -> (ContainerMode, RunOutcome) {
        // Build a fresh ContainerRequest on top of the prepared image. The
        // image already exists locally (eager construct), so `start()` here
        // is purely container-create + run + wait.
        //
        // Port mapping is user-controlled: `external_port:` (per-invocation
        // host port) pairs with the actor's declaration-level `internal_port:`
        // to drive `with_mapped_port(host, container)`. When either side is
        // missing, we don't publish a port — the container still runs, it
        // just isn't reachable from the host.
        //
        // If `external_port:` is supplied but doesn't parse as a u16, surface
        // `:bad_port`. Surfacing failure beats silently starting with no
        // mapping — the user asked for something and we couldn't deliver.
        let external_port = match external_port_kw.map(value_as_u16) {
            None => None,             // not supplied
            Some(Some(p)) => Some(p), // supplied and valid
            Some(None) => return (ContainerMode::Stopped(self), run_error(REASON_BAD_PORT)),
        };
        // `external_port:` without a declaration-level `internal_port:` is
        // meaningless — the user asked us to publish on a host port but
        // didn't say which container port to map from. Surface the same
        // `:bad_port` atom rather than silently starting with no mapping.
        if external_port.is_some() && internal_port.is_none() {
            return (ContainerMode::Stopped(self), run_error(REASON_BAD_PORT));
        }
        let mut req: ContainerRequest<GenericImage> = GenericImage::new(image_name, image_tag).into();
        if let (Some(host), Some(container)) = (external_port, internal_port) {
            req = req.with_mapped_port(host, container.tcp());
        }
        req = req.with_label(LABEL_KEY, LABEL_VALUE);

        // Optional: env vars.
        if let Some(env_val) = env_kw {
            req = match apply_env(req, env_val) {
                Ok(r) => r,
                Err(_) => return (ContainerMode::Stopped(self), run_error(REASON_BAD_ENV)),
            };
        }

        // Optional: startup timeout. `timeout:` is in milliseconds to match
        // the example `.ill` files.
        let timeout = timeout_kw
            .and_then(|v| match v {
                Value::Number(n) if *n > 0 => Some(Duration::from_millis(*n as u64)),
                _ => None,
            })
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT);
        req = req.with_startup_timeout(timeout);

        match req.start().await {
            Ok(container) => {
                let id = container.id().to_string();
                let ok = RunOk { id };
                (
                    ContainerMode::Running(Running { container }),
                    RunOutcome::Ok(ok.into_dict()),
                )
            }
            Err(e) => {
                let reason = classify_run_error(&e);
                (ContainerMode::Stopped(self), run_error(reason))
            }
        }
    }
}

impl Running {
    /// Tear down via teardown path (reverse-construction cleanup). Always
    /// transitions back to `Stopped` so a second teardown is a no-op.
    async fn teardown(self) -> (ContainerMode, TeardownOutcome) {
        match tokio::time::timeout(TEARDOWN_TIMEOUT, self.container.rm()).await {
            Ok(Ok(_)) => (ContainerMode::Stopped(Stopped), TeardownOutcome::ok()),
            Ok(Err(e)) => (
                ContainerMode::Stopped(Stopped),
                TeardownOutcome::failed(format!("rm failed: {e}")),
            ),
            Err(_timeout) => (
                ContainerMode::Stopped(Stopped),
                TeardownOutcome::failed(format!("rm timed out after {TEARDOWN_TIMEOUT:?}")),
            ),
        }
    }

    /// Tear down via the user's explicit `stop` command.
    ///
    /// Note: all three outcomes transition the actor back to `Stopped`,
    /// even when `rm()` errors or times out. That's intentional — the
    /// state machine tracks what we *attempted*, not what the docker
    /// daemon acknowledged. On a failed `rm` the container may still be
    /// alive on the daemon, but from the test's perspective we're done
    /// with it; a retry of `stop` would return `:not_running` rather
    /// than re-attempting the `rm`. Orphan cleanup is the responsibility
    /// of the future label-sweep (see ROADMAP "Docker optimizations").
    async fn stop(self) -> (ContainerMode, RunOutcome) {
        match tokio::time::timeout(TEARDOWN_TIMEOUT, self.container.rm()).await {
            Ok(Ok(_)) => (
                ContainerMode::Stopped(Stopped),
                RunOutcome::Ok(Default::default()),
            ),
            Ok(Err(_)) => (
                ContainerMode::Stopped(Stopped),
                run_error(REASON_DOCKER_UNAVAILABLE),
            ),
            Err(_timeout) => (ContainerMode::Stopped(Stopped), run_error(REASON_TIMEOUT)),
        }
    }
}

fn apply_env(
    req: ContainerRequest<GenericImage>,
    env: &Value,
) -> Result<ContainerRequest<GenericImage>, String> {
    match env {
        Value::Dict(fields) => {
            let mut out = req;
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
                out = out.with_env_var(k.clone(), s);
            }
            Ok(out)
        }
        other => Err(format!("`env` must be a dict, got {}", other.type_name())),
    }
}

fn value_as_u16(v: &Value) -> Option<u16> {
    match v {
        Value::Number(n) if *n >= 0 && *n <= u16::MAX as i64 => Some(*n as u16),
        _ => None,
    }
}

#[async_trait::async_trait]
impl ActorInstance for ContainerInstance {
    fn type_name(&self) -> &'static str {
        "container"
    }

    async fn execute(&mut self, cmd: &'static str, args: &CommandArgs) -> RunOutcome {
        let (next, outcome) = match std::mem::take(&mut self.mode) {
            ContainerMode::Stopped(s) => match cmd {
                "run" => {
                    s.run(
                        &self.image_name,
                        &self.image_tag,
                        self.internal_port,
                        args.kw("external_port"),
                        args.kw("env"),
                        args.kw("timeout"),
                    )
                    .await
                }
                "stop" => (ContainerMode::Stopped(s), run_error(REASON_NOT_RUNNING)),
                other => (
                    ContainerMode::Stopped(s),
                    RunOutcome::NotImplemented {
                        actor: "container",
                        cmd: other,
                    },
                ),
            },
            ContainerMode::Running(r) => match cmd {
                "run" => (ContainerMode::Running(r), run_error(REASON_ALREADY_RUNNING)),
                "stop" => r.stop().await,
                other => (
                    ContainerMode::Running(r),
                    RunOutcome::NotImplemented {
                        actor: "container",
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
            ContainerMode::Stopped(s) => (ContainerMode::Stopped(s), TeardownOutcome::ok()),
            ContainerMode::Running(r) => r.teardown().await,
        };
        self.mode = next;
        outcome
    }

    fn self_view(&self) -> Option<Dict> {
        Some(self.members.assigned_view())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_image_ref_bare_name_defaults_to_latest() {
        let (n, t) = split_image_ref("nginx");
        assert_eq!(n, "nginx");
        assert_eq!(t, "latest");
    }

    #[test]
    fn split_image_ref_extracts_tag() {
        let (n, t) = split_image_ref("nginx:1.25");
        assert_eq!(n, "nginx");
        assert_eq!(t, "1.25");
    }

    #[test]
    fn split_image_ref_handles_registry_port() {
        // `ghcr.io:443/org/img:tag` — only the last colon separates tag, and
        // the middle colon is part of the registry host.
        let (n, t) = split_image_ref("ghcr.io:443/org/img:v2");
        assert_eq!(n, "ghcr.io:443/org/img");
        assert_eq!(t, "v2");
    }

    #[test]
    fn split_image_ref_handles_registry_port_without_tag() {
        let (n, t) = split_image_ref("ghcr.io:443/org/img");
        assert_eq!(n, "ghcr.io:443/org/img");
        assert_eq!(t, "latest");
    }

    #[test]
    fn synthesize_name_is_stable_for_same_path() {
        let p = Path::new("/tmp/some/Dockerfile");
        assert_eq!(synthesize_image_name(p), synthesize_image_name(p));
    }

    #[test]
    fn synthesize_name_differs_for_different_paths() {
        let a = synthesize_image_name(Path::new("/a/Dockerfile"));
        let b = synthesize_image_name(Path::new("/b/Dockerfile"));
        assert_ne!(a, b);
    }

    #[test]
    fn value_as_u16_rejects_out_of_range() {
        assert_eq!(value_as_u16(&Value::Number(80)), Some(80));
        assert_eq!(value_as_u16(&Value::Number(-1)), None);
        assert_eq!(value_as_u16(&Value::Number(70_000)), None);
        assert_eq!(value_as_u16(&Value::String("80".into())), None);
    }

    // ── Docker-gated tests ─────────────────────────────────────────────────
    //
    // These hit a live Docker daemon. They are `#[ignore]` by default so
    // `cargo test` stays offline-friendly. Run locally with:
    //
    //     cargo test -p ill-core --lib container -- --ignored
    //
    // and expect each test to take multiple seconds (first run pulls the
    // base images; subsequent runs hit the daemon cache).

    use crate::runtime::Dict;

    fn empty_args() -> CommandArgs {
        CommandArgs {
            positional: Vec::new(),
            keyword: Dict::new(),
        }
    }

    fn image_args(image: &str) -> ConstructArgs {
        let mut kw = Dict::new();
        kw.insert("image".into(), Value::String(image.into()));
        ConstructArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
            ..Default::default()
        }
    }

    /// Helper: assert a construct result was an `Err(RuntimeError::Construct(_))`
    /// without relying on `Debug` impls.
    fn expect_construct_err(res: Result<ContainerInstance, RuntimeError>) -> String {
        match res {
            Ok(_) => panic!("expected construct failure, got Ok"),
            Err(RuntimeError::Construct(msg)) => msg,
            Err(_) => panic!("expected Construct error, got different RuntimeError"),
        }
    }

    fn assert_container_reason(outcome: &RunOutcome, expected: &str) {
        match outcome {
            RunOutcome::Error { variant, fields } => {
                assert_eq!(*variant, "container", "expected error.container variant");
                match fields.get("reason") {
                    Some(Value::Atom(a)) => {
                        assert_eq!(a, expected, "error.container.reason mismatch")
                    }
                    other => panic!("expected error.container.reason atom, got {other:?}"),
                }
            }
            RunOutcome::Ok(_) => panic!("expected Error, got Ok"),
            RunOutcome::NotImplemented { actor, cmd } => {
                panic!("expected Error, got NotImplemented({actor}, {cmd})")
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn image_pull_construct_run_teardown() {
        // alpine is small (~3MB), universally available. With no CMD
        // override it exits immediately; that's fine — we're checking the
        // state-machine transitions and that `rm()` on teardown cleans up.
        let args = image_args("alpine:3.19");
        let mut inst = ContainerInstance::construct(&args)
            .await
            .ok()
            .expect("construct failed");
        assert!(matches!(inst.mode, ContainerMode::Stopped(_)));

        let outcome = inst.execute("run", &empty_args()).await;
        match outcome {
            RunOutcome::Ok(fields) => match fields.get("id") {
                Some(Value::String(id)) => assert!(!id.is_empty(), "empty container id"),
                other => panic!("expected ok.id string, got {other:?}"),
            },
            _ => panic!("expected Ok from run"),
        }
        assert!(matches!(inst.mode, ContainerMode::Running(_)));

        let td = inst.teardown().await;
        assert!(td.ok, "teardown failed: {:?}", td.message);
        assert!(matches!(inst.mode, ContainerMode::Stopped(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn nonexistent_image_fails_at_construct() {
        // Image refs under `localhost/` won't resolve anywhere, so the pull
        // fails with a real daemon error — the shape we want at construct.
        let res =
            ContainerInstance::construct(&image_args("localhost/ill-nonexistent-test-image:nope"))
                .await;
        let _msg = expect_construct_err(res);
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn double_run_returns_already_running() {
        let mut inst = ContainerInstance::construct(&image_args("alpine:3.19"))
            .await
            .ok()
            .expect("construct failed");
        let _ = inst.execute("run", &empty_args()).await;
        let second = inst.execute("run", &empty_args()).await;
        assert_container_reason(&second, "already_running");
        let _ = inst.teardown().await;
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn stop_without_run_returns_not_running() {
        let mut inst = ContainerInstance::construct(&image_args("alpine:3.19"))
            .await
            .ok()
            .expect("construct failed");
        let outcome = inst.execute("stop", &empty_args()).await;
        assert_container_reason(&outcome, "not_running");
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn bad_env_returns_bad_env_atom() {
        let mut inst = ContainerInstance::construct(&image_args("alpine:3.19"))
            .await
            .ok()
            .expect("construct failed");
        let mut kw = Dict::new();
        // A bare number where a dict is required.
        kw.insert("env".into(), Value::Number(42));
        let outcome = inst
            .execute(
                "run",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        assert_container_reason(&outcome, "bad_env");
        assert!(matches!(inst.mode, ContainerMode::Stopped(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn bad_port_returns_bad_port_atom() {
        let mut inst = ContainerInstance::construct(&image_args("alpine:3.19"))
            .await
            .ok()
            .expect("construct failed");
        let mut kw = Dict::new();
        // Out of u16 range — should surface `:bad_port`, not silently start
        // the container with no port mapping.
        kw.insert("external_port".into(), Value::Number(70_000));
        let outcome = inst
            .execute(
                "run",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        assert_container_reason(&outcome, "bad_port");
        assert!(matches!(inst.mode, ContainerMode::Stopped(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn external_port_without_internal_port_returns_bad_port() {
        // `external_port:` on `run` is meaningless without a declaration-
        // level `internal_port:` — the runtime should reject rather than
        // silently start with no port mapping.
        let mut inst = ContainerInstance::construct(&image_args("alpine:3.19"))
            .await
            .ok()
            .expect("construct failed");
        let mut kw = Dict::new();
        kw.insert("external_port".into(), Value::Number(8080));
        let outcome = inst
            .execute(
                "run",
                &CommandArgs {
                    positional: Vec::new(),
                    keyword: kw,
                },
            )
            .await;
        assert_container_reason(&outcome, "bad_port");
        assert!(matches!(inst.mode, ContainerMode::Stopped(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn dockerfile_build_construct_run_teardown() {
        // Write a trivial Dockerfile to a tempdir and build it.
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!(
            "ill-container-df-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let df_path = dir.join("Dockerfile");
        let mut f = std::fs::File::create(&df_path).unwrap();
        writeln!(f, "FROM alpine:3.19\nCMD [\"echo\", \"hello from ill\"]").unwrap();

        let mut kw = Dict::new();
        kw.insert("dockerfile".into(), Value::String("Dockerfile".into()));
        let args = ConstructArgs {
            keyword: kw,
            source_dir: dir.clone(),
            ..Default::default()
        };

        let mut inst = ContainerInstance::construct(&args)
            .await
            .ok()
            .expect("build failed");

        let outcome = inst.execute("run", &empty_args()).await;
        assert!(matches!(outcome, RunOutcome::Ok(_)));
        let td = inst.teardown().await;
        assert!(td.ok, "teardown failed: {:?}", td.message);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "requires docker"]
    async fn missing_dockerfile_fails_at_construct() {
        let mut kw = Dict::new();
        kw.insert("dockerfile".into(), Value::String("nope.Dockerfile".into()));
        let args = ConstructArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
            ..Default::default()
        };
        let msg = expect_construct_err(ContainerInstance::construct(&args).await);
        assert!(
            msg.contains("dockerfile not found"),
            "unexpected message: {msg}"
        );
    }

    #[tokio::test]
    async fn both_image_and_dockerfile_rejected() {
        // No docker needed — fails before any I/O.
        let mut kw = Dict::new();
        kw.insert("image".into(), Value::String("alpine:3.19".into()));
        kw.insert("dockerfile".into(), Value::String("./Dockerfile".into()));
        let args = ConstructArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
            ..Default::default()
        };
        let _msg = expect_construct_err(ContainerInstance::construct(&args).await);
    }

    #[tokio::test]
    async fn neither_image_nor_dockerfile_rejected() {
        let args = ConstructArgs {
            keyword: Dict::new(),
            source_dir: std::env::temp_dir(),
            ..Default::default()
        };
        let _msg = expect_construct_err(ContainerInstance::construct(&args).await);
    }

    #[tokio::test]
    async fn internal_port_out_of_u16_range_rejected_at_construct() {
        // No docker needed — `internal_port` validation runs before any
        // image I/O. Both negative and above-u16 produce a Construct error
        // rather than silently truncating.
        for n in [-1i64, 70_000] {
            let mut kw = Dict::new();
            kw.insert("image".into(), Value::String("alpine:3.19".into()));
            kw.insert("internal_port".into(), Value::Number(n));
            let args = ConstructArgs {
                keyword: kw,
                source_dir: std::env::temp_dir(),
                ..Default::default()
            };
            let msg = expect_construct_err(ContainerInstance::construct(&args).await);
            assert!(msg.contains("u16"), "n={n}, unexpected message: {msg}");
        }
    }

    #[tokio::test]
    async fn internal_port_wrong_type_rejected_at_construct() {
        // A string where a number is required should surface as TypeMismatch,
        // not Construct, matching how `image` and `dockerfile` are handled.
        let mut kw = Dict::new();
        kw.insert("image".into(), Value::String("alpine:3.19".into()));
        kw.insert("internal_port".into(), Value::String("nope".into()));
        let args = ConstructArgs {
            keyword: kw,
            source_dir: std::env::temp_dir(),
            ..Default::default()
        };
        match ContainerInstance::construct(&args).await {
            Err(RuntimeError::TypeMismatch {
                expected,
                got,
                context,
            }) => {
                assert_eq!(expected, "number");
                assert_eq!(got, "string");
                assert!(
                    context.contains("internal_port"),
                    "unexpected context: {context}"
                );
            }
            Ok(_) => panic!("expected TypeMismatch, got Ok"),
            Err(_) => panic!("expected TypeMismatch, got different RuntimeError"),
        }
    }

}
