// Test harness. `run_test_file` lowers, validates, spawns actors, walks
// `as` blocks, then tears down every spawned instance regardless of outcome.
//
// The shape mirrors the validator: first pass registers/spawns actors, second
// pass walks `as` blocks in source order. This keeps check and run from
// drifting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::actor_type::ActorInstance;
use crate::ast::{
    ActorDeclaration, AsBlock, Command as CommandAst, KeywordArg, KeywordValue, Let, LetValue,
    SourceFile, Statement, TopLevel,
};
use crate::diagnostic::Severity;
use crate::registry::Registry;
use crate::validate::expr_starts_with_ident;

use super::assert::eval_assert;
use super::eval::{eval, Scope};
use super::report::{StatementReport, TeardownReport, TestReport};
use super::{CommandArgs, RunOutcome, RuntimeError, SpawnArgs, TeardownOutcome, Value};

/// Run a single .ill test file and return a structured report.
pub fn run_test_file(path: &Path, src: &str) -> TestReport {
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

    execute(path, &ast, &source_dir)
}

fn execute(path: &Path, source: &SourceFile, source_dir: &Path) -> TestReport {
    let registry = Registry::global();
    let mut statements: Vec<StatementReport> = Vec::new();
    let mut guard = TeardownGuard::new();

    // Pass 1: spawn every declared actor.
    for item in &source.items {
        let TopLevel::ActorDeclaration(decl) = item else {
            continue;
        };
        match spawn_actor(registry, decl, source_dir) {
            Ok(inst) => guard.push(decl.name.name.clone(), inst),
            Err(msg) => {
                statements.push(StatementReport::SpawnFailure {
                    actor: decl.name.name.clone(),
                    message: msg,
                    span: decl.span,
                });
            }
        }
    }

    // Bail early if any spawn failed — don't run `as` blocks against partial fixtures.
    if !statements.is_empty() {
        let teardown = guard.teardown_all();
        return TestReport {
            path: path.to_path_buf(),
            passed: false,
            statements,
            teardown,
        };
    }

    // Pass 2: walk each `as` block.
    for item in &source.items {
        let TopLevel::AsBlock(block) = item else {
            continue;
        };
        let block_failed = run_as_block(block, &mut guard, &mut statements);
        if block_failed {
            break;
        }
    }

    let passed = statements.is_empty();
    let teardown = guard.teardown_all();
    TestReport {
        path: path.to_path_buf(),
        passed,
        statements,
        teardown,
    }
}

fn spawn_actor(
    registry: &Registry,
    decl: &ActorDeclaration,
    source_dir: &Path,
) -> Result<Box<dyn ActorInstance>, String> {
    let actor_type = registry
        .get(&decl.actor_type.name)
        .ok_or_else(|| format!("unknown actor type `{}`", decl.actor_type.name))?;

    let empty = Scope::new();
    let keyword = eval_keyword_args(&decl.keyword_args, &empty).map_err(|e| e.to_string())?;

    let args = SpawnArgs {
        keyword,
        source_dir: source_dir.to_path_buf(),
    };
    actor_type.spawn(&args).map_err(|e| e.to_string())
}

/// Walk an `as` block. Returns true if a failure was recorded and the test
/// should stop (the caller still runs teardown).
fn run_as_block(
    block: &AsBlock,
    guard: &mut TeardownGuard,
    statements: &mut Vec<StatementReport>,
) -> bool {
    let registry = Registry::global();
    let actor_name = &block.actor.name;

    // Resolve the actor type for command lookup. Validation has already
    // ensured both exist — defend anyway so a harness/validator drift surfaces
    // as a recorded failure instead of a silently-passing test.
    let Some(type_name) = guard.get(actor_name).map(|i| i.type_name()) else {
        statements.push(StatementReport::EvalError {
            actor: actor_name.clone(),
            span: block.span,
            message: format!("actor `{actor_name}` has no live instance"),
        });
        return true;
    };
    let Some(actor_type) = registry.get(type_name) else {
        statements.push(StatementReport::EvalError {
            actor: actor_name.clone(),
            span: block.span,
            message: format!("unknown actor type `{type_name}` in registry"),
        });
        return true;
    };

    let mut scope = Scope::new();
    // `ok` and `error` are bound per-command; start unset.

    for (idx, stmt) in block.body.iter().enumerate() {
        match stmt {
            Statement::Command(cmd) => {
                scope.unbind("ok");
                scope.unbind("error");

                let args = match eval_command_args(cmd, &scope) {
                    Ok(a) => a,
                    Err(e) => {
                        statements.push(StatementReport::EvalError {
                            actor: actor_name.clone(),
                            span: cmd.span,
                            message: e.to_string(),
                        });
                        return true;
                    }
                };

                let Some(cmd_def) = actor_type.command(&cmd.name.name) else {
                    // Validator should have caught this; be defensive.
                    statements.push(StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: cmd.span,
                        message: format!("unknown command `{}`", cmd.name.name),
                    });
                    return true;
                };

                let Some(instance) = guard.get_mut(actor_name) else {
                    statements.push(StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: cmd.span,
                        message: format!("actor `{actor_name}` has no live instance"),
                    });
                    return true;
                };

                let outcome = actor_type.execute(cmd_def.name(), instance, &args);
                match outcome {
                    RunOutcome::Ok(fields) => {
                        scope.bind("ok", Value::Record(fields));
                    }
                    RunOutcome::Error(fields) => {
                        // An Error is a failure unless the following statements
                        // reference `error.*`, which commits the command to the
                        // error branch (matching validator semantics).
                        let expect = cmd.annotation.as_ref().and_then(|a| a.value.clone());
                        let was_expected = block_has_error_ref_after(block, idx);
                        scope.bind("error", Value::Record(fields.clone()));
                        if !was_expected {
                            statements.push(StatementReport::CommandFailure {
                                actor: actor_name.clone(),
                                command: cmd.name.name.clone(),
                                span: cmd.span,
                                error_fields: fields,
                                expect,
                            });
                            return true;
                        }
                    }
                    RunOutcome::NotImplemented { actor, cmd: c } => {
                        statements.push(StatementReport::CommandNotImplemented {
                            actor: actor.to_string(),
                            command: c.to_string(),
                            span: cmd.span,
                        });
                        return true;
                    }
                }
            }
            Statement::Let(let_stmt) => match run_let(let_stmt, &mut scope) {
                Ok(()) => {}
                Err(e) => {
                    statements.push(StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: let_stmt.span,
                        message: e.to_string(),
                    });
                    return true;
                }
            },
            Statement::Assignment(_) => {
                // Not supported in Phase 5. Validator accepts it; runtime
                // flags it so a test can't silently no-op.
                statements.push(StatementReport::EvalError {
                    actor: actor_name.clone(),
                    span: block.span,
                    message: "assignment statements are not yet supported in runtime".into(),
                });
                return true;
            }
            Statement::Assert(a) => match eval_assert(a, &scope) {
                Ok(r) if r.passed => {}
                Ok(r) => {
                    let expect = a.annotation.as_ref().and_then(|an| an.value.clone());
                    statements.push(StatementReport::AssertFailure {
                        actor: actor_name.clone(),
                        span: a.span,
                        left: r.left,
                        right: r.right,
                        op: r.op,
                        expect,
                    });
                    return true;
                }
                Err(e) => {
                    statements.push(StatementReport::EvalError {
                        actor: actor_name.clone(),
                        span: a.span,
                        message: e.to_string(),
                    });
                    return true;
                }
            },
        }
    }

    false
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

fn eval_keyword_args(
    args: &[KeywordArg],
    scope: &Scope,
) -> Result<BTreeMap<String, Value>, RuntimeError> {
    let mut out = BTreeMap::new();
    for kw in args {
        let v = match &kw.value {
            KeywordValue::Expr(e) => eval(e, scope)?,
            KeywordValue::Map(pairs) => {
                let mut rec = BTreeMap::new();
                for (k_expr, v_expr) in pairs {
                    let key = match eval(k_expr, scope)? {
                        Value::String(s) => s,
                        Value::Atom(a) => a,
                        other => {
                            return Err(RuntimeError::Eval(format!(
                                "map key must be string or atom, got {}",
                                other.type_name()
                            )));
                        }
                    };
                    let value = eval(v_expr, scope)?;
                    rec.insert(key, value);
                }
                Value::Record(rec)
            }
        };
        out.insert(kw.key.name.clone(), v);
    }
    Ok(out)
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

// ── Teardown ──────────────────────────────────────────────────────────────────
//
// Holds spawned instances in spawn order. `teardown_all` tears down in
// reverse. `Drop` is the last-resort safety net for panics — in the normal
// path `teardown_all` is called explicitly so results can be recorded.

struct TeardownGuard {
    /// (actor name, instance). Ordered by spawn.
    entries: Vec<(String, Box<dyn ActorInstance>)>,
}

impl TeardownGuard {
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

    fn get_mut(&mut self, name: &str) -> Option<&mut dyn ActorInstance> {
        for (n, inst) in self.entries.iter_mut() {
            if n == name {
                return Some(&mut **inst);
            }
        }
        None
    }

    fn teardown_all(&mut self) -> Vec<TeardownReport> {
        let mut reports = Vec::with_capacity(self.entries.len());
        while let Some((name, mut inst)) = self.entries.pop() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inst.teardown()))
                    .unwrap_or_else(|_| TeardownOutcome::failed("teardown panicked"));
            reports.push(TeardownReport {
                actor: name,
                outcome,
            });
        }
        reports.reverse(); // Report in spawn order for readability.
        reports
    }
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // If teardown_all already ran, entries is empty. This only fires
        // on panic.
        if self.entries.is_empty() {
            return;
        }
        let _ = self.teardown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_failure_reports_fail() {
        let report = run_test_file(Path::new("bogus.ill"), "actor !!!!");
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::ParseFailure(_))
        ));
    }

    #[test]
    fn validation_failure_reports_fail() {
        let report = run_test_file(Path::new("bogus.ill"), "actor bob = nope_actor\n");
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::ValidationFailure(_))
        ));
    }

    #[test]
    fn exec_basic_spawn_and_teardown() {
        // Uses `sleep 60` so the spawn has time to be observed and torn down.
        let src = "\
actor server = exec,
  command: \"sleep 60\"

as server:
  run
";
        let report = run_test_file(Path::new("t.ill"), src);
        assert!(report.passed, "statements: {}", report.statements.len());
        assert_eq!(report.teardown.len(), 1);
        assert!(report.teardown[0].outcome.ok);
    }

    #[test]
    fn assert_failure_is_recorded() {
        // Hand-roll an assertion that must fail: `assert 1 == 2`.
        // We need an actor declaration for the `as` block to be valid.
        let src = "\
actor server = exec,
  command: \"sleep 60\"

as server:
  run
  assert 1 == 2
";
        let report = run_test_file(Path::new("t.ill"), src);
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::AssertFailure { .. })
        ));
        // Teardown should still have run.
        assert_eq!(report.teardown.len(), 1);
    }

    #[test]
    fn unknown_program_is_command_failure() {
        let src = "\
actor server = exec,
  command: \"definitely_not_a_real_program_xyz\"

as server:
  run
";
        let report = run_test_file(Path::new("t.ill"), src);
        assert!(!report.passed);
        assert!(matches!(
            report.statements.first(),
            Some(StatementReport::CommandFailure { .. })
        ));
    }
}
