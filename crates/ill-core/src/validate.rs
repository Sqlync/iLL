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

use crate::actor_type::{ActorType, KeywordArgDef, Mode, OutcomeField, ValueType};
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

/// Outcome state for the command currently being processed in an `as` block.
/// Starts as `ImpliedOk` (no asserts seen), transitions on `assert ok.*` /
/// `assert error.*`, and flags a diagnostic if both are seen.
#[derive(Clone, Copy)]
enum CommandOutcome {
    ImpliedOk,
    ExplicitOk,
    ExplicitError,
}

impl CommandOutcome {
    fn is_ok(self) -> bool {
        matches!(self, CommandOutcome::ImpliedOk | CommandOutcome::ExplicitOk)
    }
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

        // Fields available on `ok.*` / `error.*` for the last command.
        let mut last_ok_fields: &'static [OutcomeField] = &[];
        let mut last_error_fields: &'static [OutcomeField] = &[];

        // Outcome state for the current command window. Reset on each command;
        // updated as ok/error asserts are encountered. Used to determine whether
        // to apply the mode transition and to flag ok/error mixing.
        let mut outcome = CommandOutcome::ImpliedOk;

        // Pending mode transition from the last command. Applied when the next
        // command is encountered (by which point we know the outcome), or at
        // end of block.
        let mut pending_transition: Option<&'static dyn Mode> = None;

        for stmt in &block.body {
            match stmt {
                Statement::Command(cmd) => {
                    // Apply the pending transition from the previous command,
                    // but only if that command's outcome was ok.
                    if outcome.is_ok() {
                        if let Some(next_mode) = pending_transition {
                            if let Some(state) = self.actors.get_mut(&block.actor.name) {
                                state.mode = next_mode;
                            }
                        }
                    }
                    outcome = CommandOutcome::ImpliedOk;

                    let (ok_fields, error_fields, transition) =
                        self.check_command(&block.actor.name, cmd);
                    last_ok_fields = ok_fields;
                    last_error_fields = error_fields;
                    pending_transition = transition;
                }
                Statement::Let(let_stmt) => {
                    let ty = match &let_stmt.value {
                        ast::LetValue::Expr(expr) => self.expr_type_in_actor(
                            &block.actor.name,
                            expr,
                            last_ok_fields,
                            last_error_fields,
                            outcome,
                        ),
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
                Statement::Assert(a) => {
                    let asserts_ok = expr_starts_with_ident(&a.left, "ok");
                    let asserts_error = expr_starts_with_ident(&a.left, "error");

                    if asserts_ok || asserts_error {
                        outcome = match (outcome, asserts_ok) {
                            (CommandOutcome::ImpliedOk, true) => CommandOutcome::ExplicitOk,
                            (CommandOutcome::ImpliedOk, false) => CommandOutcome::ExplicitError,
                            (CommandOutcome::ExplicitOk, false)
                            | (CommandOutcome::ExplicitError, true) => {
                                self.diagnostics.push(Diagnostic::error(
                                    a.span,
                                    DiagnosticCode::ConflictingOutcomeAsserts,
                                    "cannot assert both `ok` and `error` for the same command",
                                ));
                                outcome // leave unchanged after conflict
                            }
                            (o, _) => o, // already conflicted or same direction
                        };
                    }
                    // TODO: check both sides type-check and are comparable.
                    // Deferred — low-value for Phase 4, high-noise potential.
                }
            }
        }

        // Apply the pending transition for the final command in the block.
        if outcome.is_ok() {
            if let Some(next_mode) = pending_transition {
                if let Some(state) = self.actors.get_mut(&block.actor.name) {
                    state.mode = next_mode;
                }
            }
        }
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    /// Returns `(ok_fields, error_fields, pending_transition)` for the command.
    /// The transition is returned rather than applied so the caller can defer
    /// it until the command's outcome is known from following asserts.
    fn check_command(
        &mut self,
        actor_name: &str,
        cmd: &ast::Command,
    ) -> (
        &'static [OutcomeField],
        &'static [OutcomeField],
        Option<&'static dyn Mode>,
    ) {
        let Some(state) = self.actors.get(actor_name) else {
            return (&[], &[], None);
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
            return (&[], &[], None);
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
            return (&[], &[], None);
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

        (
            cmd_def.ok_fields(),
            cmd_def.error_fields(),
            cmd_def.transitions_to(),
        )
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
                let span = fallback_span;
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

    fn expr_type_in_actor(
        &self,
        actor_name: &str,
        expr: &Expr,
        last_ok_fields: &'static [OutcomeField],
        last_error_fields: &'static [OutcomeField],
        outcome: CommandOutcome,
    ) -> ValueType {
        match expr {
            Expr::MemberAccess {
                object, property, ..
            } => {
                // Resolve `ok.<field>` and `error.<field>` against the last
                // command's declared outcome fields.
                if let Expr::Ident(ident) = object.as_ref() {
                    let fields = match ident.name.as_str() {
                        "ok" => last_ok_fields,
                        "error" => last_error_fields,
                        _ => &[],
                    };
                    if !fields.is_empty() {
                        return fields
                            .iter()
                            .find(|f| f.name == property.name)
                            .map(|f| f.ty)
                            .unwrap_or(ValueType::Unknown);
                    }
                }
                ValueType::Unknown
            }
            Expr::Ident(ident) => {
                if let Some(state) = self.actors.get(actor_name) {
                    if let Some(ty) = state.vars.get(&ident.name) {
                        return *ty;
                    }
                }
                // `ok` / `error` bare — resolve to Unknown; field access is
                // the expected usage.
                let _ = outcome; // reserved for future bare-ok diagnostics
                ValueType::Unknown
            }
            _ => expr_type(expr),
        }
    }
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
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::MissingRequiredArg]);
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
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::UnknownKeywordArg]);
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

    #[test]
    fn conflicting_outcome_asserts_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  assert ok.rows == 1
  assert error.code == 2
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::ConflictingOutcomeAsserts]
        );
    }
}
