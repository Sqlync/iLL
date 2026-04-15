// Validator — a symbolic interpreter over the AST.
//
// Walks `SourceFile` in source order, tracking each actor's mode and var
// types as it goes. Phase 5's runtime will have the same structure with real
// values and real I/O. Keeping the traversal shape parallel is the defense
// against drift between validation and execution.
//
// Phase 4 scope:
//   - name resolution   (actor decls, actor_type lookup, `as` refs, duplicates)
//   - command validation (name, required args, unknown kwargs)
//   - mode tracking      (command valid in current mode, apply transitions)
//   - expression types   (narrow — enough to catch obvious mismatches)

use std::collections::HashMap;

use crate::actor_type::{ActorType, KeywordArgDef, Mode, ValueType};
use crate::ast::{self, AsBlock, Expr, KeywordArg, SourceFile, Statement, TopLevel};
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::registry::Registry;

/// Per-actor state threaded through the symbolic walk.
struct ActorState {
    type_def: &'static dyn ActorType,
    mode: &'static dyn Mode,
    /// Types of declared vars + `let` bindings introduced inside this actor's
    /// `as` blocks.
    vars: HashMap<String, ValueType>,
}

struct Validator<'r> {
    registry: &'r Registry,
    actors: HashMap<String, ActorState>,
    diagnostics: Vec<Diagnostic>,
}

pub fn validate(source: &SourceFile) -> Vec<Diagnostic> {
    let mut v = Validator {
        registry: Registry::global(),
        actors: HashMap::new(),
        diagnostics: Vec::new(),
    };
    v.run(source);
    v.diagnostics
}

impl<'r> Validator<'r> {
    fn run(&mut self, source: &SourceFile) {
        // First pass: collect actor declarations so `as` blocks can reference
        // actors in any order.
        for item in &source.items {
            if let TopLevel::ActorDeclaration(decl) = item {
                self.register_actor(decl);
            }
        }

        // Second pass: validate `as` blocks.
        for item in &source.items {
            if let TopLevel::AsBlock(block) = item {
                self.check_as_block(block);
            }
        }
    }

    // ── Actor declarations ────────────────────────────────────────────────────

    fn register_actor(&mut self, decl: &ast::ActorDeclaration) {
        if self.actors.contains_key(&decl.name.name) {
            self.diagnostics.push(Diagnostic::error(
                decl.name.span,
                DiagnosticCode::DuplicateActor,
                format!("actor `{}` is already declared", decl.name.name),
            ));
            return;
        }

        let Some(type_def) = self.registry.get(&decl.actor_type.name) else {
            self.diagnostics.push(Diagnostic::error(
                decl.actor_type.span,
                DiagnosticCode::UnknownActorType,
                format!("unknown actor type `{}`", decl.actor_type.name),
            ));
            return;
        };

        self.check_keyword_args_against(
            &decl.keyword_args,
            type_def.constructor_keyword(),
            decl.span,
            None,
        );

        let mut vars = HashMap::new();
        for var in &decl.vars {
            let ty = var
                .default
                .as_ref()
                .map(expr_type)
                .unwrap_or(ValueType::Unknown);
            vars.insert(var.name.name.clone(), ty);
        }

        self.actors.insert(
            decl.name.name.clone(),
            ActorState {
                type_def,
                mode: type_def.initial_mode(),
                vars,
            },
        );
    }

    // ── `as` blocks ───────────────────────────────────────────────────────────

    fn check_as_block(&mut self, block: &AsBlock) {
        if !self.actors.contains_key(&block.actor.name) {
            self.diagnostics.push(Diagnostic::error(
                block.actor.span,
                DiagnosticCode::UnknownActor,
                format!("unknown actor `{}`", block.actor.name),
            ));
            return;
        }

        // Track the last command's ok result shape for `let x = ok.*` resolution.
        // `error.*` binding support isn't wired yet; add it when a real case needs it.
        let mut last_ok: ValueType = ValueType::Unknown;

        for (idx, stmt) in block.body.iter().enumerate() {
            match stmt {
                Statement::Command(cmd) => {
                    // A command followed (before the next command) by an assert
                    // on `error.*` is on the failure branch — don't apply the
                    // success-path mode transition.
                    let on_error_branch = asserts_error_before_next_command(&block.body, idx);
                    let (ok, _) = self.check_command(&block.actor.name, cmd, !on_error_branch);
                    last_ok = ok;
                }
                Statement::Let(let_stmt) => {
                    let ty = match &let_stmt.value {
                        ast::LetValue::Expr(expr) => {
                            self.expr_type_in_actor(&block.actor.name, expr, last_ok)
                        }
                        ast::LetValue::Parse { format, .. } => match format.name.as_str() {
                            "json" => ValueType::Json,
                            _ => ValueType::Unknown,
                        },
                    };
                    if let Some(state) = self.actors.get_mut(&block.actor.name) {
                        state.vars.insert(let_stmt.name.name.clone(), ty);
                    }
                }
                Statement::Assignment(_) => {
                    // TODO: check target var exists, check mutability annotation,
                    // check type. Deferred — needs annotation semantics nailed down.
                }
                Statement::Assert(_) => {
                    // TODO: check both sides type-check and are comparable.
                    // Deferred — low-value for Phase 4, high-noise potential.
                }
            }
        }
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    /// Returns `(ok_type, error_type)` for the command, used by following
    /// `let`/`assert` statements that reference `ok.*` / `error.*`.
    ///
    /// `apply_transition` is false when the caller has determined this command
    /// is on the error branch (asserted via `error.*`), in which case the
    /// success-path mode transition must not happen.
    fn check_command(
        &mut self,
        actor_name: &str,
        cmd: &ast::Command,
        apply_transition: bool,
    ) -> (ValueType, ValueType) {
        let Some(state) = self.actors.get(actor_name) else {
            return (ValueType::Unknown, ValueType::Unknown);
        };
        let type_def = state.type_def;
        let current_mode = state.mode;

        let Some(cmd_def) = type_def.command(&cmd.name.name) else {
            self.diagnostics.push(Diagnostic::error(
                cmd.name.span,
                DiagnosticCode::UnknownCommand,
                format!(
                    "unknown command `{}` for actor type `{}`",
                    cmd.name.name,
                    type_def.name()
                ),
            ));
            return (ValueType::Unknown, ValueType::Unknown);
        };

        // Mode check
        let valid_modes = cmd_def.valid_in_modes();
        let in_valid_mode = valid_modes
            .iter()
            .any(|m| crate::actor_type::same_mode(*m, current_mode));
        if !in_valid_mode {
            let expected: Vec<&str> = valid_modes.iter().map(|m| m.name()).collect();
            self.diagnostics.push(Diagnostic::error(
                cmd.name.span,
                DiagnosticCode::CommandNotValidInMode,
                format!(
                    "command `{}` is not valid in mode `{}` (valid: {})",
                    cmd.name.name,
                    current_mode.name(),
                    expected.join(", "),
                ),
            ));
            return (ValueType::Unknown, ValueType::Unknown);
        }

        // Required positional args (presence only for now).
        let expected_positional = cmd_def.positional().len();
        let actual_positional = cmd.positional_args.len();
        if actual_positional < expected_positional {
            let missing = cmd_def.positional()[actual_positional].name;
            self.diagnostics.push(Diagnostic::error(
                cmd.name.span,
                DiagnosticCode::MissingRequiredArg,
                format!(
                    "command `{}` missing required positional arg `{}`",
                    cmd.name.name, missing
                ),
            ));
        }

        // Keyword args: required presence + unknown names.
        self.check_keyword_args_against(
            &cmd.keyword_args,
            cmd_def.keyword(),
            cmd.span,
            Some(&cmd.name.name),
        );

        // Apply mode transition only if: (1) the command was valid in the
        // current mode (otherwise we'd compound errors on bogus state), and
        // (2) the caller didn't mark this as the error branch.
        if in_valid_mode && apply_transition {
            if let Some(next) = cmd_def.transitions_to() {
                if let Some(state) = self.actors.get_mut(actor_name) {
                    state.mode = next;
                }
            }
        }

        (cmd_def.result_type(), cmd_def.error_type())
    }

    fn check_keyword_args_against(
        &mut self,
        provided: &[KeywordArg],
        expected: &[KeywordArgDef],
        fallback_span: ast::Span,
        command_context: Option<&str>,
    ) {
        // Unknown kwargs
        for kw in provided {
            if !expected.iter().any(|d| d.name == kw.key.name) {
                let msg = match command_context {
                    Some(cmd) => {
                        format!(
                            "unknown keyword arg `{}` for command `{}`",
                            kw.key.name, cmd
                        )
                    }
                    None => format!("unknown keyword arg `{}`", kw.key.name),
                };
                self.diagnostics.push(Diagnostic::error(
                    kw.key.span,
                    DiagnosticCode::UnknownKeywordArg,
                    msg,
                ));
            }
        }

        // Missing required kwargs
        for def in expected {
            if def.required && !provided.iter().any(|kw| kw.key.name == def.name) {
                let span = provided.first().map(|kw| kw.span).unwrap_or(fallback_span);
                let msg = match command_context {
                    Some(cmd) => format!(
                        "command `{}` missing required keyword arg `{}`",
                        cmd, def.name
                    ),
                    None => format!("missing required keyword arg `{}`", def.name),
                };
                self.diagnostics.push(Diagnostic::error(
                    span,
                    DiagnosticCode::MissingRequiredArg,
                    msg,
                ));
            }
        }
    }

    // ── Expression typing (narrow) ────────────────────────────────────────────

    fn expr_type_in_actor(&self, actor_name: &str, expr: &Expr, last_ok: ValueType) -> ValueType {
        match expr {
            Expr::Ident(ident) => {
                // `ok` is a keyword-ish binding set by the last command.
                if ident.name == "ok" {
                    return last_ok;
                }
                if let Some(state) = self.actors.get(actor_name) {
                    if let Some(ty) = state.vars.get(&ident.name) {
                        return *ty;
                    }
                }
                ValueType::Unknown
            }
            _ => expr_type(expr),
        }
    }
}

/// True if the command at `cmd_idx` is followed (before any later command)
/// by an assert whose left-hand side starts with the `error` identifier.
///
/// This lets the validator stay on the failure branch rather than naively
/// advancing the mode as if the command had succeeded.
fn asserts_error_before_next_command(body: &[Statement], cmd_idx: usize) -> bool {
    for stmt in &body[cmd_idx + 1..] {
        match stmt {
            Statement::Command(_) => return false,
            Statement::Assert(a) if expr_starts_with_ident(&a.left, "error") => return true,
            _ => {}
        }
    }
    false
}

fn expr_starts_with_ident(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(ident) => ident.name == name,
        Expr::MemberAccess { object, .. } => expr_starts_with_ident(object, name),
        Expr::Index { object, .. } => expr_starts_with_ident(object, name),
        _ => false,
    }
}

/// Context-free expression type — good enough for literals and simple forms.
fn expr_type(expr: &Expr) -> ValueType {
    match expr {
        Expr::StringLit(_) => ValueType::String,
        Expr::Number(_) => ValueType::Number,
        Expr::Bool(_) => ValueType::Bool,
        Expr::Atom(_) => ValueType::Atom,
        Expr::Sigil(sigil) => match sigil.name.name.as_str() {
            "sql" => ValueType::String,
            "json" => ValueType::Json,
            "hex" => ValueType::Bytes,
            _ => ValueType::Unknown,
        },
        _ => ValueType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::lower;

    fn diags(src: &str) -> Vec<Diagnostic> {
        let ast = lower(src).expect("lower ok");
        validate(&ast)
    }

    fn codes(ds: &[Diagnostic]) -> Vec<DiagnosticCode> {
        ds.iter().map(|d| d.code).collect()
    }

    #[test]
    fn unknown_actor_type_is_flagged() {
        let src = "actor bob = nope_actor\n";
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::UnknownActorType]);
    }

    #[test]
    fn duplicate_actor_is_flagged() {
        let src = "\
actor alice = pg_client
actor alice = pg_client
";
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::DuplicateActor]);
    }

    #[test]
    fn unknown_actor_in_as_block_is_flagged() {
        let src = "\
actor alice = pg_client
as bob:
  connect,
    user: \"u\"
    database: \"d\"
";
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::UnknownActor]);
    }

    #[test]
    fn unknown_command_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  nope
";
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::UnknownCommand]);
    }

    #[test]
    fn query_before_connect_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  query \"SELECT 1\"
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::CommandNotValidInMode]
        );
    }

    #[test]
    fn missing_required_kwarg_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
";
        // missing `database`
        let ds = diags(src);
        assert!(ds
            .iter()
            .any(|d| d.code == DiagnosticCode::MissingRequiredArg));
    }

    #[test]
    fn unknown_kwarg_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
    bogus: 1
";
        let ds = diags(src);
        assert!(ds
            .iter()
            .any(|d| d.code == DiagnosticCode::UnknownKeywordArg));
    }

    #[test]
    fn all_examples_validate_cleanly() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let paths = crate::test_util::collect_ill_files(&examples_dir);
        assert!(!paths.is_empty(), "no examples found");

        let mut failures = Vec::new();
        for p in &paths {
            let src = std::fs::read_to_string(p).expect("read example");
            let ast = lower(&src).expect("lower example");
            let ds = validate(&ast);
            let errors: Vec<_> = ds
                .iter()
                .filter(|d| d.severity == crate::diagnostic::Severity::Error)
                .collect();
            if !errors.is_empty() {
                failures.push((p.clone(), errors.into_iter().cloned().collect::<Vec<_>>()));
            }
        }

        if !failures.is_empty() {
            for (p, errs) in &failures {
                eprintln!("{}", p.display());
                for e in errs {
                    eprintln!("  {e}");
                }
            }
            panic!("{} example(s) failed validation", failures.len());
        }
    }

    #[test]
    fn connect_then_query_validates_cleanly() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  query \"SELECT 1\"
";
        assert!(diags(src).is_empty(), "expected no diagnostics");
    }

    /// A command immediately followed by `assert error.*` is on the failure
    /// branch. The validator must NOT apply the mode transition, so subsequent
    /// commands that require the pre-command mode remain valid.
    #[test]
    fn error_branch_does_not_advance_mode() {
        // connect is followed by an error assert → stays disconnected.
        // A second connect attempt must therefore be valid (not flagged as
        // "connect not valid in connected mode").
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  assert error.code == 1
  connect,
    user: \"u\"
    database: \"d\"
";
        assert!(diags(src).is_empty(), "expected no diagnostics");
    }

    /// Conversely, a successful connect (no error assert) must advance the mode,
    /// making a second connect invalid.
    #[test]
    fn successful_command_advances_mode() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  connect,
    user: \"u\"
    database: \"d\"
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::CommandNotValidInMode]
        );
    }
}
