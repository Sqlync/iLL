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

use crate::actor_type::{ActorType, ErrorTypeDef, KeywordArgDef, Mode, OutcomeField, ValueType};
use crate::ast::{self, AsBlock, Expr, KeywordArg, SourceFile, Statement, StringFragment, TopLevel};
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::registry::Registry;
use crate::runtime::squiggle::Registry as SquiggleRegistry;

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
        for kw in &decl.keyword_args {
            self.check_squiggles_in_kwvalue(&kw.value);
        }

        // Type comes from the default expression; vars without a default
        // are `Unknown` to the validator. The runtime mirror of this lives
        // in `args_actor::runtime` — required-no-default vars become
        // `String` at construct time. See `DeclaredVar`.
        let mut vars = HashMap::new();
        for var in &decl.vars {
            let ty = var
                .default
                .as_ref()
                .map(expr_type)
                .unwrap_or(ValueType::Unknown);
            vars.insert(var.name.name.clone(), ty);
            if let Some(d) = &var.default {
                self.check_squiggles_in_expr(d);
            }
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

        // Schemas for `ok.*` / `error.*` after the last command.
        let mut last_ok_fields: &'static [OutcomeField] = &[];
        let mut last_error_types: &'static [ErrorTypeDef] = &[];

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

                    let (ok_fields, error_types, transition) =
                        self.check_command(&block.actor.name, cmd);
                    last_ok_fields = ok_fields;
                    last_error_types = error_types;
                    pending_transition = transition;
                }
                Statement::Let(let_stmt) => {
                    let ty = match &let_stmt.value {
                        ast::LetValue::Expr(expr) => {
                            // A `let` binding that references `ok.*` or `error.*`
                            // implies the command's outcome, same as an assert.
                            if expr_starts_with_ident(expr, "ok") {
                                outcome = advance_outcome(
                                    outcome,
                                    true,
                                    let_stmt.span,
                                    &mut self.diagnostics,
                                );
                            } else if expr_starts_with_ident(expr, "error") {
                                outcome = advance_outcome(
                                    outcome,
                                    false,
                                    let_stmt.span,
                                    &mut self.diagnostics,
                                );
                            }
                            self.check_squiggles_in_expr(expr);
                            self.expr_type_in_actor(
                                &block.actor.name,
                                expr,
                                last_ok_fields,
                                last_error_types,
                                outcome,
                            )
                        }
                        ast::LetValue::Parse { source, format } => {
                            self.check_squiggles_in_expr(source);
                            match format.name.as_str() {
                                "json" => ValueType::Dynamic,
                                _ => ValueType::Unknown,
                            }
                        }
                    };
                    if let Some(state) = self.actors.get_mut(&block.actor.name) {
                        state.vars.insert(let_stmt.name.name.clone(), ty);
                    }
                }
                Statement::Assignment(a) => {
                    // TODO: check target var exists, check mutability annotation,
                    // check type. Deferred — needs annotation semantics nailed down.
                    self.check_squiggles_in_expr(&a.target);
                    self.check_squiggles_in_expr(&a.value);
                }
                Statement::Assert(a) => {
                    let asserts_ok = expr_starts_with_ident(&a.left, "ok");
                    let asserts_error = expr_starts_with_ident(&a.left, "error");

                    if asserts_ok || asserts_error {
                        outcome =
                            advance_outcome(outcome, asserts_ok, a.span, &mut self.diagnostics);
                    }
                    self.check_squiggles_in_expr(&a.left);
                    if let Some(r) = &a.right {
                        self.check_squiggles_in_expr(r);
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

    /// Returns `(ok_fields, error_types, pending_transition)` for the command.
    /// The transition is returned rather than applied so the caller can defer
    /// it until the command's outcome is known from following asserts.
    fn check_command(
        &mut self,
        actor_name: &str,
        cmd: &ast::Command,
    ) -> (
        &'static [OutcomeField],
        &'static [ErrorTypeDef],
        Option<&'static dyn Mode>,
    ) {
        let Some(state) = self.actors.get(actor_name) else {
            return (&[], &[], None);
        };
        let type_def = state.type_def;
        let current_mode = state.mode;

        let Some((cmd_def, consumed)) =
            type_def.resolve_command(&cmd.name.name, &cmd.positional_args)
        else {
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

        // Required positional args (presence only for now). The first
        // `consumed` source positionals were absorbed by command-name
        // resolution (e.g. mqtt's `receive publish`); the schema only
        // describes the remaining args.
        let source_positional = &cmd.positional_args[consumed..];
        let expected_positional = cmd_def.positional().len();
        let actual_positional = source_positional.len();
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
        for arg in source_positional {
            self.check_squiggles_in_expr(arg);
        }

        // Keyword args: required presence + unknown names.
        self.check_keyword_args_against(
            &cmd.keyword_args,
            cmd_def.keyword(),
            cmd.span,
            Some(&cmd.name.name),
        );
        for kw in &cmd.keyword_args {
            self.check_squiggles_in_kwvalue(&kw.value);
        }

        (
            cmd_def.ok_fields(),
            cmd_def.error_types(),
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

    // ── Squiggle validation ───────────────────────────────────────────────────

    /// Recurse through `expr` and emit `UnknownSquiggle` for any squiggle
    /// whose name isn't registered. Squiggles are an open set at the grammar
    /// level — unknown names are caught here rather than at runtime.
    fn check_squiggles_in_expr(&mut self, expr: &Expr) {
        let diags = &mut self.diagnostics;
        for_each_expr(expr, &mut |e| {
            if let Expr::Squiggle(s) = e {
                if SquiggleRegistry::global().get(&s.name.name).is_none() {
                    diags.push(Diagnostic::error(
                        s.name.span,
                        DiagnosticCode::UnknownSquiggle,
                        format!("unknown squiggle `~{}`", s.name.name),
                    ));
                }
            }
        });
    }

    fn check_squiggles_in_kwvalue(&mut self, value: &ast::KeywordValue) {
        match value {
            ast::KeywordValue::Expr(e) => self.check_squiggles_in_expr(e),
            ast::KeywordValue::Map(pairs) => {
                for (k, v) in pairs {
                    self.check_squiggles_in_expr(k);
                    self.check_squiggles_in_expr(v);
                }
            }
        }
    }

    // ── Expression typing (narrow) ────────────────────────────────────────────

    fn expr_type_in_actor(
        &self,
        actor_name: &str,
        expr: &Expr,
        last_ok_fields: &'static [OutcomeField],
        last_error_types: &'static [ErrorTypeDef],
        outcome: CommandOutcome,
    ) -> ValueType {
        match expr {
            Expr::MemberAccess {
                object, property, ..
            } => {
                // `self.<field>` resolves against the enclosing actor's
                // declared vars. Unknown fields fall through to the
                // generic outcome-chain resolver below, which returns
                // None for non-ok/error roots → Unknown.
                if let Expr::Ident(root) = object.as_ref() {
                    if root.name == "self" {
                        if let Some(state) = self.actors.get(actor_name) {
                            if let Some(ty) = state.vars.get(&property.name) {
                                return *ty;
                            }
                        }
                        return ValueType::Unknown;
                    }
                }
                resolve_outcome_chain(expr, last_ok_fields, last_error_types)
                    .unwrap_or(ValueType::Unknown)
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

/// Visit `expr` and every nested sub-expression in pre-order. Used by
/// validator passes that need to look at every Expr position uniformly
/// (e.g. the squiggle-name check).
fn for_each_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Squiggle(s) => {
            for frag in &s.fragments {
                if let StringFragment::Interpolation(e) = frag {
                    for_each_expr(e, f);
                }
            }
        }
        Expr::StringLit(lit) => {
            for frag in &lit.fragments {
                if let StringFragment::Interpolation(e) = frag {
                    for_each_expr(e, f);
                }
            }
        }
        Expr::Array(items) => {
            for e in items {
                for_each_expr(e, f);
            }
        }
        Expr::MemberAccess { object, .. } => for_each_expr(object, f),
        Expr::Index {
            object, indices, ..
        } => {
            for_each_expr(object, f);
            for e in indices {
                for_each_expr(e, f);
            }
        }
        Expr::Ident(_) | Expr::Number(_) | Expr::Bool(_) | Expr::Atom(_) => {}
    }
}

fn resolve_outcome_chain(
    expr: &Expr,
    last_ok_fields: &'static [OutcomeField],
    last_error_types: &'static [ErrorTypeDef],
) -> Option<ValueType> {
    let Expr::MemberAccess {
        object, property, ..
    } = expr
    else {
        return None;
    };

    match object.as_ref() {
        Expr::Ident(root) => match root.name.as_str() {
            "ok" => last_ok_fields
                .iter()
                .find(|f| f.name == property.name)
                .map(|f| f.ty),
            "error" => match property.name.as_str() {
                "type" => Some(ValueType::Atom),
                // Bare `error.<variant>` — variant exists but isn't a leaf.
                _ => last_error_types
                    .iter()
                    .find(|v| v.name == property.name)
                    .map(|_| ValueType::Unknown),
            },
            _ => None,
        },
        Expr::MemberAccess {
            object: inner_object,
            property: variant_prop,
            ..
        } => {
            // Two-deep chain must be `error.<variant>.<field>`.
            let Expr::Ident(root) = inner_object.as_ref() else {
                return None;
            };
            if root.name != "error" {
                return None;
            }
            let variant = last_error_types
                .iter()
                .find(|v| v.name == variant_prop.name)?;
            variant
                .fields
                .iter()
                .find(|f| f.name == property.name)
                .map(|f| f.ty)
        }
        _ => None,
    }
}

/// Advance the `CommandOutcome` state when an `ok.*` or `error.*` reference is
/// encountered — whether in an `assert` or a `let` binding.
///
/// `is_ok` is true for `ok.*`, false for `error.*`. Emits
/// `ConflictingOutcomeAsserts` if both sides are referenced for the same
/// command.
fn advance_outcome(
    current: CommandOutcome,
    is_ok: bool,
    span: ast::Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> CommandOutcome {
    match (current, is_ok) {
        (CommandOutcome::ImpliedOk, true) => CommandOutcome::ExplicitOk,
        (CommandOutcome::ImpliedOk, false) => CommandOutcome::ExplicitError,
        (CommandOutcome::ExplicitOk, false) | (CommandOutcome::ExplicitError, true) => {
            diagnostics.push(Diagnostic::error(
                span,
                DiagnosticCode::ConflictingOutcomeAsserts,
                "cannot mix `ok` and `error` references for the same command",
            ));
            current
        }
        (o, _) => o,
    }
}

/// True if `expr` is `name`, `name.foo`, `name[…]`, or any nested chain rooted
/// at an ident named `name`. Used by both validation and runtime to detect
/// `ok.*` / `error.*` references.
pub(crate) fn expr_starts_with_ident(expr: &Expr, name: &str) -> bool {
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
        Expr::Squiggle(squiggle) => SquiggleRegistry::global()
            .get(&squiggle.name.name)
            .map(|s| s.output_type())
            .unwrap_or(ValueType::Unknown),
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
    fn unknown_squiggle_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  query ~yaml`SELECT 1`
";
        assert_eq!(codes(&diags(src)), vec![DiagnosticCode::UnknownSquiggle]);
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
  assert error.type == :auth
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
  assert error.type == :auth
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::ConflictingOutcomeAsserts]
        );
    }

    /// A `let` binding that references `ok.*` implicitly commits to the ok
    /// branch, same as `assert ok.*`. A following `assert error.*` must be
    /// flagged as conflicting.
    #[test]
    fn let_ok_then_assert_error_is_flagged() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  let x = ok.rows
  assert error.type == :auth
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::ConflictingOutcomeAsserts]
        );
    }

    /// A `let` binding on `ok.*` advances the mode (it implies success), so a
    /// second connect must be invalid.
    #[test]
    fn let_ok_advances_mode() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  let x = ok.rows
  connect,
    user: \"u\"
    database: \"d\"
";
        assert_eq!(
            codes(&diags(src)),
            vec![DiagnosticCode::CommandNotValidInMode]
        );
    }

    /// A `let` binding on `error.*` keeps the mode (it implies failure), so
    /// a second connect must remain valid.
    #[test]
    fn let_error_does_not_advance_mode() {
        let src = "\
actor alice = pg_client
as alice:
  connect,
    user: \"u\"
    database: \"d\"
  let e = error.type
  connect,
    user: \"u\"
    database: \"d\"
";
        assert!(diags(src).is_empty(), "expected no diagnostics");
    }
}
