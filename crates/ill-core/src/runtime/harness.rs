// Test harness. `run_test_file` lowers, validates, constructs actors, walks
// `as` blocks, then tears down every constructed instance regardless of outcome.
//
// The shape mirrors the validator: first pass registers/constructs actors,
// second pass walks `as` blocks in source order. This keeps check and run
// from drifting.

use std::path::{Path, PathBuf};

use crate::actor_type::ActorInstance;
use crate::ast::{
    ActorDeclaration, AsBlock, Command as CommandAst, Expr, KeywordArg, KeywordValue, Let,
    LetValue, SourceFile, Statement, TopLevel,
};
use crate::diagnostic::Severity;
use crate::registry::Registry;
use crate::validate::expr_starts_with_ident;

use super::assert::eval_assert;
use super::eval::{eval, Scope};
use super::report::{StatementReport, TeardownReport, TestReport};
use super::{
    CommandArgs, ConstructArgs, DeclaredVar, Dict, RunOutcome, RuntimeError, TeardownOutcome, Value,
};

/// Run a single .ill test file and return a structured report.
pub async fn run_test_file(path: &Path, src: &str) -> TestReport {
    let source_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let ast = match crate::lower::lower(src) {
        Ok(a) => a,
        Err(errors) => {
            let messages = errors.iter().map(|e| e.to_string()).collect();
            return TestReport {
                path: path.to_path_buf(),
                passed: false,
                statements: vec![StatementReport::ParseFailure(messages)],
                teardown: Vec::new(),
            };
        }
    };

    let diags = crate::validate::validate(&ast);
    let errors: Vec<_> = diags
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        return TestReport {
            path: path.to_path_buf(),
            passed: false,
            statements: vec![StatementReport::ValidationFailure(errors)],
            teardown: Vec::new(),
        };
    }

    execute(path, &ast, &source_dir).await
}

async fn execute(path: &Path, source: &SourceFile, source_dir: &Path) -> TestReport {
    let registry = Registry::global();
    let mut statements: Vec<StatementReport> = Vec::new();
    let mut actors = InstantiatedActors::new();

    // Walk top-level items in source order: construct actors as we encounter
    // them, run `as` blocks against whatever's already live. Any failure stops
    // the walk — teardown still runs via `actors` for everything constructed
    // so far.
    for item in &source.items {
        match item {
            TopLevel::ActorDeclaration(decl) => {
                match construct_actor(registry, decl, source_dir).await {
                    Ok(inst) => actors.push(decl.name.name.clone(), inst),
                    Err(msg) => {
                        statements.push(StatementReport::ConstructFailure {
                            actor: decl.name.name.clone(),
                            message: msg,
                            span: decl.span,
                        });
                        break;
                    }
                }
            }
            TopLevel::AsBlock(block) => {
                if let Err(stmt) = run_as_block(block, &mut actors).await {
                    statements.push(stmt);
                    break;
                }
            }
        }
    }

    let passed = statements.is_empty();
    let teardown = actors.teardown_all().await;
    TestReport {
        path: path.to_path_buf(),
        passed,
        statements,
        teardown,
    }
}

async fn construct_actor(
    registry: &Registry,
    decl: &ActorDeclaration,
    source_dir: &Path,
) -> Result<Box<dyn ActorInstance>, String> {
    let actor_type = registry
        .get(&decl.actor_type.name)
        .ok_or_else(|| format!("unknown actor type `{}`", decl.actor_type.name))?;

    let empty = Scope::new();
    let keyword = eval_keyword_args(&decl.keyword_args, &empty).map_err(|e| e.to_string())?;

    let mut vars = Vec::with_capacity(decl.vars.len());
    for var in &decl.vars {
        let default = match &var.default {
            Some(expr) => Some(eval(expr, &empty).map_err(|e| e.to_string())?),
            None => None,
        };
        vars.push(DeclaredVar {
            name: var.name.name.clone(),
            default,
        });
    }

    let args = ConstructArgs {
        keyword,
        source_dir: source_dir.to_path_buf(),
        vars,
    };
    actor_type.construct(&args).await.map_err(|e| e.to_string())
}

/// Walk an `as` block. `Err` signals that a failure was recorded and the test
/// should stop (the caller still runs teardown).
async fn run_as_block(
    block: &AsBlock,
    actors: &mut InstantiatedActors,
) -> Result<(), StatementReport> {
    let registry = Registry::global();
    let actor_name = &block.actor.name;

    // Resolve the actor type for command lookup. Validation has already
    // ensured both exist — defend anyway so a harness/validator drift surfaces
    // as a recorded failure instead of a silently-passing test.
    let type_name = actors
        .get(actor_name)
        .map(|i| i.type_name())
        .ok_or_else(|| StatementReport::EvalError {
            actor: actor_name.clone(),
            span: block.span,
            message: format!("actor `{actor_name}` has no live instance"),
        })?;
    let actor_type = registry
        .get(type_name)
        .ok_or_else(|| StatementReport::EvalError {
            actor: actor_name.clone(),
            span: block.span,
            message: format!("unknown actor type `{type_name}` in registry"),
        })?;

    let mut scope = Scope::new();

    for (idx, stmt) in block.body.iter().enumerate() {
        bind_self(&mut scope, actors, actor_name);
        match stmt {
            Statement::Command(cmd) => {
                scope.unbind("ok");
                scope.unbind("error");

                let args =
                    eval_command_args(cmd, &scope).map_err(|e| StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: cmd.span,
                        message: e.to_string(),
                    })?;

                // Validator should have caught an unknown command; be defensive.
                let cmd_def = actor_type.command(&cmd.name.name).ok_or_else(|| {
                    StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: cmd.span,
                        message: format!("unknown command `{}`", cmd.name.name),
                    }
                })?;

                let instance =
                    actors
                        .get_mut(actor_name)
                        .ok_or_else(|| StatementReport::EvalError {
                            actor: actor_name.clone(),
                            span: cmd.span,
                            message: format!("actor `{actor_name}` has no live instance"),
                        })?;

                match instance.execute(cmd_def.name(), &args).await {
                    RunOutcome::Ok(fields) => {
                        scope.bind("ok", Value::Dict(fields));
                    }
                    RunOutcome::Error { variant, fields } => {
                        // An Error is a failure unless the following statements
                        // reference `error.*`, which commits the command to the
                        // error branch (matching validator semantics).
                        let was_expected = block_has_error_ref_after(block, idx);
                        let error_dict = build_error_dict(variant, fields);
                        if !was_expected {
                            let expect = cmd.annotation.as_ref().and_then(|a| a.value.clone());
                            scope.bind("error", Value::Dict(error_dict.clone()));
                            return Err(StatementReport::CommandFailure {
                                actor: actor_name.clone(),
                                command: cmd.name.name.clone(),
                                span: cmd.span,
                                error_fields: error_dict,
                                expect,
                            });
                        }
                        scope.bind("error", Value::Dict(error_dict));
                    }
                    RunOutcome::NotImplemented { actor, cmd: c } => {
                        return Err(StatementReport::CommandNotImplemented {
                            actor: actor.to_string(),
                            command: c.to_string(),
                            span: cmd.span,
                        });
                    }
                }
            }
            Statement::Let(let_stmt) => {
                run_let(let_stmt, &mut scope).map_err(|e| StatementReport::EvalError {
                    actor: actor_name.clone(),
                    span: let_stmt.span,
                    message: e.to_string(),
                })?;
            }
            Statement::Assignment(_) => {
                // Not supported in Phase 5. Validator accepts it; runtime
                // flags it so a test can't silently no-op.
                return Err(StatementReport::EvalError {
                    actor: actor_name.clone(),
                    span: block.span,
                    message: "assignment statements are not yet supported in runtime".into(),
                });
            }
            Statement::Assert(a) => {
                let result = eval_assert(a, &scope).map_err(|e| StatementReport::EvalError {
                    actor: actor_name.clone(),
                    span: a.span,
                    message: e.to_string(),
                })?;
                if !result.passed {
                    let expect = a.annotation.as_ref().and_then(|an| an.value.clone());
                    return Err(StatementReport::AssertFailure {
                        actor: actor_name.clone(),
                        span: a.span,
                        left: result.left,
                        right: result.right,
                        op: result.op,
                        expect,
                    });
                }
            }
        }
    }

    Ok(())
}

fn run_let(let_stmt: &Let, scope: &mut Scope) -> Result<(), RuntimeError> {
    let value = match &let_stmt.value {
        LetValue::Expr(expr) => eval(expr, scope)?,
        LetValue::Parse { .. } => {
            return Err(RuntimeError::Eval(
                "`let ... parse as ...` is not yet supported in runtime".into(),
            ));
        }
    };
    scope.bind(let_stmt.name.name.clone(), value);
    Ok(())
}

fn eval_command_args(cmd: &CommandAst, scope: &Scope) -> Result<CommandArgs, RuntimeError> {
    let mut positional = Vec::with_capacity(cmd.positional_args.len());
    for expr in &cmd.positional_args {
        positional.push(eval(expr, scope)?);
    }
    let keyword = eval_keyword_args(&cmd.keyword_args, scope)?;
    Ok(CommandArgs {
        positional,
        keyword,
    })
}

fn eval_keyword_args(args: &[KeywordArg], scope: &Scope) -> Result<Dict, RuntimeError> {
    let mut out = Dict::new();
    for kw in args {
        let v = match &kw.value {
            KeywordValue::Expr(e) => eval(e, scope)?,
            KeywordValue::Map(pairs) => {
                let mut rec = Dict::new();
                for (k_expr, v_expr) in pairs {
                    // Bare identifier keys (e.g. `NGINX_HOST:` inside an
                    // `env:` block) are taken as literal key names rather
                    // than scope lookups — that's the established pattern
                    // for env vars across the example suite. String literals
                    // (e.g. `"Content-Type":` in `headers:`) are evaluated
                    // so that interpolation still works.
                    let key = match k_expr {
                        Expr::Ident(ident) => ident.name.clone(),
                        other_expr => match eval(other_expr, scope)? {
                            Value::String(s) => s,
                            Value::Atom(a) => a,
                            other => {
                                return Err(RuntimeError::Eval(format!(
                                    "map key must be identifier, string, or atom, got {}",
                                    other.type_name()
                                )));
                            }
                        },
                    };
                    let value = eval(v_expr, scope)?;
                    rec.insert(key, value);
                }
                Value::Dict(rec)
            }
        };
        out.insert(kw.key.name.clone(), v);
    }
    Ok(out)
}

/// Refresh actor-state bindings in `scope`. Called at the top of every
/// statement so command-driven mutations are visible to the statements
/// that follow.
///
/// Every constructed actor's `self_view` is bound under its declared
/// name, so cross-actor reads (`<other>.<field>`, e.g. `db.port`)
/// resolve through normal `MemberAccess`. The current actor is also
/// bound as `self` for the shorthand form. Actors that don't expose
/// state (`self_view() == None`) have their name unbound, so accesses
/// surface as "undefined name" at eval time.
fn bind_self(scope: &mut Scope, actors: &InstantiatedActors, actor_name: &str) {
    for (name, inst) in actors.entries() {
        match inst.self_view() {
            Some(dict) => scope.bind(name.clone(), Value::Dict(dict)),
            None => scope.unbind(name),
        }
    }
    match actors.get(actor_name).and_then(|i| i.self_view()) {
        Some(dict) => scope.bind("self", Value::Dict(dict)),
        None => scope.unbind("self"),
    }
}

/// Assemble the scope-visible `error` dict from a `RunOutcome::Error`.
/// Every error exposes `type` (atom naming the variant) and `message` so
/// tests can report on an unexpected variant without knowing its schema.
/// Variant-specific fields live under `error.<variant>`.
fn build_error_dict(variant: &'static str, fields: Dict) -> Dict {
    let mut out = Dict::new();
    out.insert("type".into(), Value::Atom(variant.into()));
    out.insert(variant.into(), Value::Dict(fields));
    out
}

/// Check whether any statement after index `after` in the block references
/// `error.*` — matching the validator's rule that an `error.*` reference
/// commits the preceding command to the error branch.
fn block_has_error_ref_after(block: &AsBlock, after: usize) -> bool {
    for stmt in block.body.iter().skip(after + 1) {
        // Stop at the next command — that opens a new command window.
        if matches!(stmt, Statement::Command(_)) {
            return false;
        }
        match stmt {
            Statement::Assert(a) => {
                if expr_starts_with_ident(&a.left, "error") {
                    return true;
                }
            }
            Statement::Let(l) => {
                if let LetValue::Expr(e) = &l.value {
                    if expr_starts_with_ident(e, "error") {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

// ── Live actor instances ─────────────────────────────────────────────────────
//
// Holds instances in construction order. `teardown_all` tears down in
// reverse. Each actor is responsible for its own panic-safe cleanup via its
// own `Drop` impl (e.g. exec wires `tokio::process::Command::kill_on_drop(true)`
// at spawn time so the child is SIGKILLed when `Child` drops). There is no
// `Drop` on this container because `teardown` is async and `Drop` can't await.

struct InstantiatedActors {
    /// (actor name, instance). Ordered by construction.
    entries: Vec<(String, Box<dyn ActorInstance>)>,
}

impl InstantiatedActors {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn push(&mut self, name: String, instance: Box<dyn ActorInstance>) {
        self.entries.push((name, instance));
    }

    fn get(&self, name: &str) -> Option<&dyn ActorInstance> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, i)| i.as_ref())
    }

    /// Borrow every (name, instance) pair in construction order. Used by
    /// `bind_self` to expose every actor's `self_view` in scope under its
    /// own name for cross-actor reads.
    fn entries(&self) -> impl Iterator<Item = (&String, &dyn ActorInstance)> {
        self.entries.iter().map(|(n, i)| (n, i.as_ref()))
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut dyn ActorInstance> {
        for (n, inst) in self.entries.iter_mut() {
            if n == name {
                return Some(&mut **inst);
            }
        }
        None
    }

    async fn teardown_all(&mut self) -> Vec<TeardownReport> {
        let mut reports = Vec::with_capacity(self.entries.len());
        while let Some((name, mut inst)) = self.entries.pop() {
            // Run each teardown on a spawned task so a panic in one actor's
            // teardown reports as a failure for that actor instead of
            // aborting the whole teardown walk. `JoinError::is_panic`
            // surfaces the panic; we map it to the same "teardown panicked"
            // message the sync version used.
            let outcome = match tokio::spawn(async move { inst.teardown().await }).await {
                Ok(o) => o,
                Err(e) if e.is_panic() => TeardownOutcome::failed("teardown panicked"),
                Err(_) => TeardownOutcome::failed("teardown cancelled"),
            };
            reports.push(TeardownReport {
                actor: name,
                outcome,
            });
        }
        reports.reverse(); // Report in construction order for readability.
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_failure_reports_fail() {
        let report = run_test_file(Path::new("bogus.ill"), "actor !!!!").await;
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::ParseFailure(_))
        ));
    }

    #[tokio::test]
    async fn validation_failure_reports_fail() {
        let report = run_test_file(Path::new("bogus.ill"), "actor bob = nope_actor\n").await;
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::ValidationFailure(_))
        ));
    }

    #[tokio::test]
    async fn exec_basic_construct_and_teardown() {
        // Uses `sleep 60` so the process has time to be observed and torn down.
        let src = "\
actor server = exec,
  command: \"sleep 60\"

as server:
  run
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(report.passed, "statements: {}", report.statements.len());
        assert_eq!(report.teardown.len(), 1);
        assert!(report.teardown[0].outcome.ok);
    }

    #[tokio::test]
    async fn assert_failure_is_recorded() {
        // Hand-roll an assertion that must fail: `assert 1 == 2`.
        // We need an actor declaration for the `as` block to be valid.
        let src = "\
actor server = exec,
  command: \"sleep 60\"

as server:
  run
  assert 1 == 2
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::AssertFailure { .. })
        ));
        // Teardown should still have run.
        assert_eq!(report.teardown.len(), 1);
    }

    #[tokio::test]
    async fn unknown_program_is_command_failure() {
        let src = "\
actor server = exec,
  command: \"definitely_not_a_real_program_xyz\"

as server:
  run
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::CommandFailure { .. })
        ));
    }

    #[tokio::test]
    async fn expected_command_not_found_passes_via_error_branch() {
        // Mirrors examples/exec/failing.ill: a run that fails to spawn is
        // committed to the error branch by the `error.exec.reason` assert, so
        // the test passes.
        let src = "\
actor never_runs = exec,
  command: \"definitely_not_a_real_program_xyz\"

as never_runs:
  run
  assert error.exec.reason == :command_not_found
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(
            report.passed,
            "expected pass, got {} statement(s)",
            report.statements.len()
        );
        assert_eq!(report.teardown.len(), 1);
    }

    #[tokio::test]
    async fn wrong_reason_assert_fails() {
        // Same setup, but assert on the wrong reason — must record an
        // AssertFailure, not a CommandFailure.
        let src = "\
actor never_runs = exec,
  command: \"definitely_not_a_real_program_xyz\"

as never_runs:
  run
  assert error.exec.reason == :permission_denied
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::AssertFailure { .. })
        ));
    }

    #[tokio::test]
    async fn cross_actor_member_var_resolves_in_assert() {
        // `args_actor` has a no-op `check` command and exposes its declared
        // vars via `self_view`. Two actors in scope; from inside `as a:` we
        // read `b.name` and confirm it resolves to b's declared default.
        let src = "\
actor a = args_actor,
  vars:
    name: \"alice\"
actor b = args_actor,
  vars:
    name: \"bob\"

as a:
  check
  assert b.name == \"bob\"
  assert self.name == \"alice\"
";
        let report = run_test_file(Path::new("t.ill"), src).await;
        assert!(
            report.passed,
            "expected pass, got {} failed statement(s)",
            report.statements.len()
        );
    }
}
