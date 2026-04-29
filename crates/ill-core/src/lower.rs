// Tree-sitter CST → AST lowering pass.
//
// Walks the concrete syntax tree produced by tree-sitter-ill and builds the
// typed AST defined in ast.rs. Collects errors rather than bailing on the first
// problem so we can report multiple issues at once.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, DiagnosticCode};

/// Process backslash escape sequences in a double-quoted string fragment.
/// Squiggles and single-quoted strings are raw and skip this pass.
fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('$') => out.push('$'),
            Some(c) => return Err(format!("unknown escape sequence `\\{c}`")),
            None => return Err("dangling backslash at end of string".to_string()),
        }
    }
    Ok(out)
}

// ── Input normalization ────────────────────────────────────────────────────────

/// Normalize source text before handing it to tree-sitter.
///
/// Handles common real-world messiness:
/// - Windows (`\r\n`) and old-Mac (`\r`) line endings → `\n`
/// - Tabs → two spaces (matching the scanner's `TAB_WIDTH = 2`)
/// - Trailing whitespace on every line (prevents spurious NEWLINE tokens at EOF)
/// - Ensures the file ends with exactly one newline
pub fn normalize(src: &str) -> String {
    // Normalise line endings first so split('\n') works uniformly.
    let s = src.replace("\r\n", "\n").replace('\r', "\n");

    let mut result = String::with_capacity(s.len() + 1);
    for line in s.split('\n') {
        // Expand tabs to two spaces (matching scanner TAB_WIDTH).
        let expanded = line.replace('\t', "  ");
        // Strip trailing spaces/tabs from each line.
        result.push_str(expanded.trim_end());
        result.push('\n');
    }

    // split('\n') on "a\nb\n" yields ["a", "b", ""], so the trailing empty
    // element produces an extra '\n'. Collapse any run of trailing newlines
    // down to exactly one.
    while result.ends_with("\n\n") {
        result.pop();
    }

    result
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn lower(source: &str) -> Result<SourceFile, Vec<Diagnostic>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ill::LANGUAGE.into())
        .expect("failed to load tree-sitter-ill grammar");

    let normalized = normalize(source);
    let tree = parser
        .parse(&normalized, None)
        .expect("tree-sitter parse failed");
    let root = tree.root_node();

    let mut ctx = LowerCtx {
        source: &normalized,
        errors: Vec::new(),
    };

    // Collect any tree-sitter ERROR nodes.
    ctx.collect_errors(root);

    let file = ctx.lower_source_file(root);

    if ctx.errors.is_empty() {
        Ok(file)
    } else {
        Err(ctx.errors)
    }
}

// ── Context ────────────────────────────────────────────────────────────────────

struct LowerCtx<'a> {
    source: &'a str,
    errors: Vec<Diagnostic>,
}

impl<'a> LowerCtx<'a> {
    fn text(&self, node: tree_sitter::Node) -> &'a str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    fn span(&self, node: tree_sitter::Node) -> Span {
        Span {
            start: node.start_byte(),
            end: node.end_byte(),
        }
    }

    fn collect_errors(&mut self, node: tree_sitter::Node) {
        if node.is_missing() {
            // MISSING nodes are tree-sitter's recovery hint for "expected X here
            // but didn't see it". `node.kind()` carries the expected token type.
            let kind = node.kind();
            let msg = if kind.is_empty() {
                "missing token here".to_string()
            } else {
                format!("missing `{kind}` here")
            };
            self.errors.push(Diagnostic::error(
                self.span(node),
                DiagnosticCode::MissingToken,
                msg,
            ));
            // MISSING nodes don't have meaningful children; stop descending.
            return;
        }

        if node.is_error() {
            // ERROR nodes wrap whatever the parser couldn't make sense of.
            // We emit one diagnostic per ERROR and don't walk into it — its
            // children are byproducts of recovery, not real errors the user
            // should see.
            self.errors.push(self.error_diagnostic(node));
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_errors(child);
        }
    }

    /// Build a diagnostic for an ERROR node. Looks at the surrounding context
    /// to produce something more useful than "parse error at 676..702".
    fn error_diagnostic(&self, node: tree_sitter::Node) -> Diagnostic {
        let span = self.span(node);
        let snippet = self.text(node);
        let trimmed = snippet.trim();
        let mut chars = trimmed.chars();
        let preview: String = chars.by_ref().take(40).collect();
        let preview_suffix = if chars.next().is_some() { "…" } else { "" };

        let parent_kind = node.parent().map(|p| p.kind()).unwrap_or("");

        let message = if trimmed.is_empty() {
            match parent_kind {
                "" | "source_file" => "unexpected input here".to_string(),
                kind => format!("unexpected input inside `{kind}`"),
            }
        } else {
            format!("unexpected `{preview}{preview_suffix}`")
        };

        let mut diag = Diagnostic::error(span, DiagnosticCode::ParseError, message);
        if !parent_kind.is_empty() && parent_kind != "source_file" {
            diag = diag.with_note(format!("while parsing a `{parent_kind}`"));
        }
        diag
    }

    // ── Source file ────────────────────────────────────────────────────────

    fn lower_source_file(&mut self, node: tree_sitter::Node) -> SourceFile {
        let mut items = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "actor_declaration" => {
                    if let Some(decl) = self.lower_actor_declaration(child) {
                        items.push(TopLevel::ActorDeclaration(decl));
                    }
                }
                "as_block" => {
                    if let Some(block) = self.lower_as_block(child) {
                        items.push(TopLevel::AsBlock(block));
                    }
                }
                "NEWLINE" | "comment" => {}
                // ERROR/MISSING were already reported by collect_errors; don't
                // double-flag them here as "unexpected node".
                _ if child.is_error() || child.is_missing() => {}
                _ => {
                    self.errors.push(Diagnostic::error(
                        self.span(child),
                        DiagnosticCode::UnexpectedNode,
                        format!("unexpected `{}` at top level", child.kind()),
                    ));
                }
            }
        }
        SourceFile { items }
    }

    // ── Actor declarations ─────────────────────────────────────────────────

    fn lower_actor_declaration(&mut self, node: tree_sitter::Node) -> Option<ActorDeclaration> {
        let name = self.lower_ident_field(node, "name")?;
        let type_node = node.child_by_field_name("type")?;
        // actor_type wraps an identifier
        let actor_type = self.lower_ident_from_first_child(type_node)?;

        let mut keyword_args = Vec::new();
        let mut vars = Vec::new();

        // Walk actor_body if present.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "actor_body" {
                self.lower_actor_body(child, &mut keyword_args, &mut vars);
            }
        }

        Some(ActorDeclaration {
            name,
            actor_type,
            keyword_args,
            vars,
            span: self.span(node),
        })
    }

    fn lower_actor_body(
        &mut self,
        node: tree_sitter::Node,
        keyword_args: &mut Vec<KeywordArg>,
        vars: &mut Vec<VarDeclaration>,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "actor_property" {
                self.lower_actor_property(child, keyword_args, vars);
            }
        }
    }

    fn lower_actor_property(
        &mut self,
        node: tree_sitter::Node,
        keyword_args: &mut Vec<KeywordArg>,
        vars: &mut Vec<VarDeclaration>,
    ) {
        // actor_property is either key: value OR a vars_block
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "vars_block" {
                self.lower_vars_block(child, vars);
                return;
            }
        }

        // It's a key: value property → KeywordArg
        let key = match self.lower_ident_field(node, "key") {
            Some(k) => k,
            None => return,
        };
        let value_node = match node.child_by_field_name("value") {
            Some(n) => n,
            None => return,
        };
        let value = match self.lower_expression(value_node) {
            Some(e) => e,
            None => return,
        };

        keyword_args.push(KeywordArg {
            key,
            value: KeywordValue::Expr(value),
            span: self.span(node),
        });
    }

    fn lower_vars_block(&mut self, node: tree_sitter::Node, vars: &mut Vec<VarDeclaration>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "var_declaration" {
                if let Some(v) = self.lower_var_declaration(child) {
                    vars.push(v);
                }
            }
        }
    }

    fn lower_var_declaration(&mut self, node: tree_sitter::Node) -> Option<VarDeclaration> {
        let name = self.lower_ident_field(node, "name")?;

        let mut annotations = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "annotation" {
                if let Some(ann) = self.lower_annotation(child) {
                    annotations.push(ann);
                }
            }
        }

        let default = node
            .child_by_field_name("default")
            .and_then(|n| self.lower_expression(n));

        Some(VarDeclaration {
            annotations,
            name,
            default,
            span: self.span(node),
        })
    }

    fn lower_annotation(&mut self, node: tree_sitter::Node) -> Option<Annotation> {
        let mut name = None;
        let mut value = None;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "annotation_name" => {
                    name = Some(self.ident_from_node(child));
                }
                "annotation_value" => {
                    // annotation_value wraps either an identifier or a string
                    let mut inner_cursor = child.walk();
                    for inner in child.named_children(&mut inner_cursor) {
                        match inner.kind() {
                            "identifier" => {
                                value = Some(self.text(inner).to_string());
                            }
                            "string" => {
                                value = Some(self.extract_string_text(inner));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Some(Annotation {
            name: name?,
            value,
            span: self.span(node),
        })
    }

    // ── As blocks ──────────────────────────────────────────────────────────

    fn lower_as_block(&mut self, node: tree_sitter::Node) -> Option<AsBlock> {
        let actor = self.lower_ident_field(node, "actor")?;
        let mut body = Vec::new();

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "block" {
                self.lower_block(child, &mut body);
            }
        }

        Some(AsBlock {
            actor,
            body,
            span: self.span(node),
        })
    }

    fn lower_block(&mut self, node: tree_sitter::Node, stmts: &mut Vec<Statement>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "command" => {
                    if let Some(cmd) = self.lower_command(child) {
                        stmts.push(Statement::Command(cmd));
                    }
                }
                "assert_statement" => {
                    if let Some(a) = self.lower_assert(child) {
                        stmts.push(Statement::Assert(a));
                    }
                }
                "let_statement" => {
                    if let Some(l) = self.lower_let(child) {
                        stmts.push(Statement::Let(l));
                    }
                }
                "assignment_statement" => {
                    if let Some(a) = self.lower_assignment(child) {
                        stmts.push(Statement::Assignment(a));
                    }
                }
                "INDENT" | "DEDENT" | "NEWLINE" | "comment" => {}
                _ if child.is_error() || child.is_missing() => {}
                _ => {
                    self.errors.push(Diagnostic::error(
                        self.span(child),
                        DiagnosticCode::UnexpectedNode,
                        format!("unexpected `{}` inside `as` block", child.kind()),
                    ));
                }
            }
        }
    }

    // ── Commands ───────────────────────────────────────────────────────────

    fn lower_command(&mut self, node: tree_sitter::Node) -> Option<Command> {
        let name = self.lower_ident_field(node, "name")?;
        let mut annotation = None;
        let mut positional_args = Vec::new();
        let mut keyword_args = Vec::new();

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "annotation" => {
                    annotation = self.lower_annotation(child);
                }
                "keyword_block" | "inline_keyword_args" => {
                    self.collect_keyword_args(child, &mut keyword_args);
                }
                "primary_expression" | "member_expression" | "index_expression" => {
                    if let Some(expr) = self.lower_expression(child) {
                        positional_args.push(expr);
                    }
                }
                _ => {}
            }
        }

        Some(Command {
            annotation,
            name,
            positional_args,
            keyword_args,
            span: self.span(node),
        })
    }

    // ── Keyword args ───────────────────────────────────────────────────────

    /// Collect `keyword_arg` children from either a `keyword_block` (indented)
    /// or `inline_keyword_args` (same-line) node — both have identical shape.
    fn collect_keyword_args(&mut self, node: tree_sitter::Node, args: &mut Vec<KeywordArg>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "keyword_arg" {
                if let Some(kw) = self.lower_keyword_arg(child) {
                    args.push(kw);
                }
            }
        }
    }

    fn lower_keyword_arg(&mut self, node: tree_sitter::Node) -> Option<KeywordArg> {
        let key = self.lower_ident_field(node, "key")?;

        // The value field can be either a simple expression or a nested block
        // of keyword_pair nodes (INDENT keyword_pair_list DEDENT).
        // Check for keyword_pair children to distinguish.
        let mut pairs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "keyword_pair" {
                if let Some(pair) = self.lower_keyword_pair(child) {
                    pairs.push(pair);
                }
            }
        }

        let value = if !pairs.is_empty() {
            KeywordValue::Map(pairs)
        } else {
            let value_node = node.child_by_field_name("value")?;
            KeywordValue::Expr(self.lower_expression(value_node)?)
        };

        Some(KeywordArg {
            key,
            value,
            span: self.span(node),
        })
    }

    fn lower_keyword_pair(&mut self, node: tree_sitter::Node) -> Option<(Expr, Expr)> {
        let key_node = node.child_by_field_name("key")?;
        let value_node = node.child_by_field_name("value")?;
        let key = self.lower_expression(key_node)?;
        let value = self.lower_expression(value_node)?;
        Some((key, value))
    }

    // ── Assert ─────────────────────────────────────────────────────────────

    fn lower_assert(&mut self, node: tree_sitter::Node) -> Option<Assert> {
        let left_node = node.child_by_field_name("left")?;
        let left = self.lower_expression(left_node)?;

        let op = node
            .child_by_field_name("operator")
            .and_then(|n| self.lower_comparison_op(n));

        let right = node
            .child_by_field_name("right")
            .and_then(|n| self.lower_expression(n));

        let mut annotation = None;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "annotation" {
                annotation = self.lower_annotation(child);
                break;
            }
        }

        Some(Assert {
            annotation,
            left,
            op,
            right,
            span: self.span(node),
        })
    }

    fn lower_comparison_op(&mut self, node: tree_sitter::Node) -> Option<ComparisonOp> {
        let text = self.text(node);
        let op = match text {
            "==" => ComparisonOp::Eq,
            "!=" => ComparisonOp::NotEq,
            ">" => ComparisonOp::Gt,
            ">=" => ComparisonOp::Gte,
            "<" => ComparisonOp::Lt,
            "<=" => ComparisonOp::Lte,
            "contains" => ComparisonOp::Contains,
            "!contains" => ComparisonOp::NotContains,
            "matches" => ComparisonOp::Matches,
            "!matches" => ComparisonOp::NotMatches,
            _ => {
                self.errors.push(Diagnostic::error(
                    self.span(node),
                    DiagnosticCode::InvalidLiteral,
                    format!("`{text}` is not a valid comparison operator"),
                ));
                return None;
            }
        };
        Some(op)
    }

    // ── Let ────────────────────────────────────────────────────────────────

    fn lower_let(&mut self, node: tree_sitter::Node) -> Option<Let> {
        let name = self.lower_ident_field(node, "name")?;
        let value_node = node.child_by_field_name("value")?;

        let value = if value_node.kind() == "parse_expression" {
            self.lower_parse_expression(value_node)?
        } else {
            LetValue::Expr(self.lower_expression(value_node)?)
        };

        Some(Let {
            name,
            value,
            span: self.span(node),
        })
    }

    fn lower_parse_expression(&mut self, node: tree_sitter::Node) -> Option<LetValue> {
        let source_node = node.child_by_field_name("source")?;
        let format = self.lower_ident_field(node, "format")?;
        let source = self.lower_expression(source_node)?;
        Some(LetValue::Parse { source, format })
    }

    // ── Assignment ─────────────────────────────────────────────────────────

    fn lower_assignment(&mut self, node: tree_sitter::Node) -> Option<Assignment> {
        let target_node = node.child_by_field_name("target")?;
        let target = self.lower_expression(target_node)?;
        let value_node = node.child_by_field_name("value")?;
        let value = self.lower_expression(value_node)?;
        Some(Assignment {
            target,
            value,
            span: self.span(node),
        })
    }

    // ── Expressions ────────────────────────────────────────────────────────

    fn lower_expression(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        match node.kind() {
            "primary_expression" => {
                // primary_expression wraps one child
                let child = node.named_child(0)?;
                self.lower_primary(child)
            }
            "member_expression" => self.lower_member_expression(node),
            "index_expression" => self.lower_index_expression(node),
            // Sometimes tree-sitter gives us the inner node directly
            "identifier" | "string" | "number" | "boolean" | "atom" | "array" | "squiggle" => {
                self.lower_primary(node)
            }
            _ => {
                if !node.is_error() && !node.is_missing() {
                    self.errors.push(Diagnostic::error(
                        self.span(node),
                        DiagnosticCode::UnexpectedNode,
                        format!("expected an expression, found `{}`", node.kind()),
                    ));
                }
                None
            }
        }
    }

    fn lower_primary(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        match node.kind() {
            "identifier" => Some(Expr::Ident(self.ident_from_node(node))),
            "number" => {
                let text = self.text(node);
                match text.parse::<i64>() {
                    Ok(n) => Some(Expr::Number(n)),
                    Err(_) => {
                        self.errors.push(Diagnostic::error(
                            self.span(node),
                            DiagnosticCode::InvalidLiteral,
                            format!("`{text}` is not a valid integer"),
                        ));
                        None
                    }
                }
            }
            "boolean" => {
                let val = self.text(node) == "true";
                Some(Expr::Bool(val))
            }
            "atom" => {
                // atom is `:` + identifier
                Some(Expr::Atom(self.ident_from_node(node.named_child(0)?)))
            }
            "string" => self.lower_string(node),
            "squiggle" => self.lower_squiggle(node),
            "array" => self.lower_array(node),
            _ => {
                if !node.is_error() && !node.is_missing() {
                    self.errors.push(Diagnostic::error(
                        self.span(node),
                        DiagnosticCode::UnexpectedNode,
                        format!("expected a value, found `{}`", node.kind()),
                    ));
                }
                None
            }
        }
    }

    fn lower_member_expression(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let object_node = node.child_by_field_name("object")?;
        let property = self.lower_ident_field(node, "property")?;
        let object = self.lower_expression(object_node)?;
        Some(Expr::MemberAccess {
            object: Box::new(object),
            property,
            span: self.span(node),
        })
    }

    fn lower_index_expression(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let object_node = node.child_by_field_name("object")?;
        let object = self.lower_expression(object_node)?;

        let mut indices = Vec::new();
        let mut cursor = node.walk();
        for child in node.children_by_field_name("index", &mut cursor) {
            if let Some(expr) = self.lower_expression(child) {
                indices.push(expr);
            }
        }

        Some(Expr::Index {
            object: Box::new(object),
            indices,
            span: self.span(node),
        })
    }

    fn lower_array(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let mut elements = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "primary_expression" | "member_expression" | "index_expression" => {
                    if let Some(expr) = self.lower_expression(child) {
                        elements.push(expr);
                    }
                }
                _ => {}
            }
        }
        Some(Expr::Array(elements))
    }

    // ── Strings ────────────────────────────────────────────────────────────

    fn lower_string(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        // string wraps double_quoted_string or single_quoted_string
        let inner = node.named_child(0)?;
        match inner.kind() {
            "double_quoted_string" => {
                let fragments = self.lower_string_fragments(inner);
                Some(Expr::StringLit(StringLit {
                    fragments,
                    span: self.span(node),
                }))
            }
            "single_quoted_string" => {
                // Single-quoted strings are tokens with no children.
                // Strip the surrounding quotes.
                let raw = self.text(inner);
                let content = &raw[1..raw.len() - 1];
                Some(Expr::StringLit(StringLit {
                    fragments: vec![StringFragment::Text(content.to_string())],
                    span: self.span(node),
                }))
            }
            _ => None,
        }
    }

    fn lower_string_fragments(&mut self, node: tree_sitter::Node) -> Vec<StringFragment> {
        let mut fragments = self.collect_fragments(node, "string_content");
        for frag in fragments.iter_mut() {
            if let StringFragment::Text(t) = frag {
                match unescape(t) {
                    Ok(s) => *t = s,
                    Err(message) => self.errors.push(Diagnostic::error(
                        self.span(node),
                        DiagnosticCode::InvalidEscape,
                        message,
                    )),
                }
            }
        }
        fragments
    }

    /// Collect `{text_kind | interpolation}*` children into a fragment list.
    /// Shared between strings (`string_content`) and squiggles (`squiggle_content`).
    fn collect_fragments(
        &mut self,
        node: tree_sitter::Node,
        text_kind: &str,
    ) -> Vec<StringFragment> {
        let mut fragments = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == text_kind {
                fragments.push(StringFragment::Text(self.text(child).to_string()));
            } else if child.kind() == "interpolation" {
                // interpolation contains one expression child
                if let Some(expr) = child.named_child(0).and_then(|n| self.lower_expression(n)) {
                    fragments.push(StringFragment::Interpolation(expr));
                }
            }
        }
        fragments
    }

    // ── Squiggles ──────────────────────────────────────────────────────────

    fn lower_squiggle(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let name = self.ident_from_node(node.child_by_field_name("name")?);
        let fragments = self.collect_fragments(node, "squiggle_content");

        Some(Expr::Squiggle(Squiggle {
            name,
            fragments,
            span: self.span(node),
        }))
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// Build an `Ident` from a node, using its text as the name.
    fn ident_from_node(&self, node: tree_sitter::Node) -> Ident {
        Ident {
            name: self.text(node).to_string(),
            span: self.span(node),
        }
    }

    fn lower_ident_field(&mut self, node: tree_sitter::Node, field: &str) -> Option<Ident> {
        let child = node.child_by_field_name(field).or_else(|| {
            self.errors.push(Diagnostic::error(
                self.span(node),
                DiagnosticCode::MissingField,
                format!("`{}` is missing required `{field}`", node.kind()),
            ));
            None
        })?;
        Some(self.ident_from_node(child))
    }

    fn lower_ident_from_first_child(&mut self, node: tree_sitter::Node) -> Option<Ident> {
        Some(self.ident_from_node(node.named_child(0)?))
    }

    /// Extract plain text from a string node (strips quotes, ignores interpolation).
    /// Used for annotation values where we just want the text content.
    fn extract_string_text(&self, node: tree_sitter::Node) -> String {
        let inner = match node.named_child(0) {
            Some(n) => n,
            None => return String::new(),
        };
        match inner.kind() {
            "double_quoted_string" => {
                let mut text = String::new();
                let mut cursor = inner.walk();
                for child in inner.named_children(&mut cursor) {
                    if child.kind() == "string_content" {
                        text.push_str(self.text(child));
                    }
                }
                text
            }
            "single_quoted_string" => {
                let raw = self.text(inner);
                raw[1..raw.len() - 1].to_string()
            }
            _ => String::new(),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_minimal_actor() {
        let source = "actor db = container\n";
        let file = lower(source).expect("should lower");
        assert_eq!(file.items.len(), 1);
        match &file.items[0] {
            TopLevel::ActorDeclaration(decl) => {
                assert_eq!(decl.name.name, "db");
                assert_eq!(decl.actor_type.name, "container");
            }
            _ => panic!("expected actor declaration"),
        }
    }

    #[test]
    fn lower_squiggle_with_arbitrary_name() {
        // Squiggle names are an open set at the grammar level — any identifier
        // works. Whether it's a *known* squiggle is a validate-time concern.
        let source = "actor a = container\nas a:\n  cmd ~yaml`hello`\n";
        let file = lower(source).expect("arbitrary squiggle name should lower");
        let as_block = file
            .items
            .iter()
            .find_map(|i| match i {
                TopLevel::AsBlock(b) => Some(b),
                _ => None,
            })
            .expect("expected as-block");
        let cmd = match &as_block.body[0] {
            Statement::Command(c) => c,
            _ => panic!("expected command"),
        };
        let sq = match &cmd.positional_args[0] {
            Expr::Squiggle(s) => s,
            _ => panic!("expected squiggle expression"),
        };
        assert_eq!(sq.name.name, "yaml");
        assert_eq!(sq.fragments.len(), 1);
        match &sq.fragments[0] {
            StringFragment::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected text fragment"),
        }
    }

    // ── normalize() ────────────────────────────────────────────────────────

    #[test]
    fn normalize_crlf() {
        assert_eq!(normalize("a\r\nb\r\nc\r\n"), "a\nb\nc\n");
    }

    #[test]
    fn normalize_bare_cr() {
        assert_eq!(normalize("a\rb\rc\r"), "a\nb\nc\n");
    }

    #[test]
    fn normalize_trailing_spaces() {
        assert_eq!(normalize("a  \nb  \n"), "a\nb\n");
    }

    #[test]
    fn normalize_no_trailing_newline() {
        assert_eq!(normalize("a\nb"), "a\nb\n");
    }

    #[test]
    fn normalize_multiple_trailing_newlines() {
        assert_eq!(normalize("a\n\n\n"), "a\n");
    }

    #[test]
    fn normalize_tabs() {
        // Two-space expansion matching scanner TAB_WIDTH = 2
        assert_eq!(normalize("\tcmd"), "  cmd\n");
    }

    #[test]
    fn normalize_preserves_internal_blank_lines() {
        assert_eq!(normalize("a\n\nb\n"), "a\n\nb\n");
    }

    // ── Messy input round-trips through lower() ────────────────────────────

    #[test]
    fn lower_crlf_source() {
        let source = "actor db = container\r\n";
        lower(source).expect("CRLF source should lower cleanly");
    }

    #[test]
    fn lower_no_trailing_newline() {
        let source = "actor db = container";
        lower(source).expect("source without trailing newline should lower cleanly");
    }

    #[test]
    fn lower_trailing_whitespace_line() {
        // A file that ends with a whitespace-only line — the bug that broke
        // readme.ill's `as bob:` block before the scanner fix.
        let source = "actor db = container\n  ";
        lower(source).expect("trailing whitespace line should lower cleanly");
    }

    // ── Trailing commas in vars: ────────────────────────────────────────────

    #[test]
    fn lower_vars_trailing_commas() {
        let source = "\
actor args = args_actor,
  vars:
    required,
    optional_a: \"foo\",
    optional_b: \"bar\",
";
        let file = lower(source).expect("trailing commas in vars should lower cleanly");
        match &file.items[0] {
            TopLevel::ActorDeclaration(decl) => {
                assert_eq!(decl.vars.len(), 3);
                assert_eq!(decl.vars[0].name.name, "required");
                assert_eq!(decl.vars[1].name.name, "optional_a");
                assert_eq!(decl.vars[2].name.name, "optional_b");
            }
            _ => panic!("expected actor declaration"),
        }
    }

    // ── Negative numbers ────────────────────────────────────────────────────

    #[test]
    fn lower_negative_number_in_array() {
        let source = "\
actor db = container
as db:
  assert ok.row[1] == [\"root\", -4, \"root\", \"0000000000\"]
";
        let file = lower(source).expect("negative numbers in arrays should lower cleanly");
        let as_block = file
            .items
            .iter()
            .find_map(|i| match i {
                TopLevel::AsBlock(b) => Some(b),
                _ => None,
            })
            .expect("expected as-block");
        let assert_stmt = match &as_block.body[0] {
            Statement::Assert(a) => a,
            _ => panic!("expected assert statement"),
        };
        let array = match assert_stmt.right.as_ref() {
            Some(Expr::Array(elems)) => elems,
            other => panic!("expected array on rhs, got {other:?}"),
        };
        assert!(matches!(array[1], Expr::Number(-4)), "expected -4, got {:?}", array[1]);
    }

    #[test]
    fn lower_negative_number_standalone() {
        let source = "\
actor db = container
as db:
  assert ok.code == -1
";
        let file = lower(source).expect("standalone negative number should lower cleanly");
        let as_block = file
            .items
            .iter()
            .find_map(|i| match i {
                TopLevel::AsBlock(b) => Some(b),
                _ => None,
            })
            .expect("expected as-block");
        let assert_stmt = match &as_block.body[0] {
            Statement::Assert(a) => a,
            _ => panic!("expected assert statement"),
        };
        match assert_stmt.right.as_ref() {
            Some(Expr::Number(-1)) => {}
            other => panic!("expected -1, got {other:?}"),
        }
    }

    // ── Corpus smoke test ───────────────────────────────────────────────────

    /// Every `.ill` file under examples/ must parse cleanly and lower to an
    /// error-free AST. Catches grammar/scanner/lowering regressions across
    /// the whole corpus in one shot.
    #[test]
    fn all_examples_lower_cleanly() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let paths = crate::test_util::collect_ill_files(&examples_dir);
        assert!(
            !paths.is_empty(),
            "found no examples under {}",
            examples_dir.display()
        );

        let mut failures = Vec::new();
        for p in &paths {
            let src = std::fs::read_to_string(p).expect("read example");
            if let Err(errs) = lower(&src) {
                failures.push((p.clone(), errs));
            }
        }

        if !failures.is_empty() {
            for (p, errs) in &failures {
                eprintln!("FAIL {} ({} errors)", p.display(), errs.len());
                for e in errs.iter().take(3) {
                    eprintln!("  {}", e);
                }
            }
            panic!(
                "{}/{} examples failed to lower",
                failures.len(),
                paths.len()
            );
        }
    }

    // ── unescape() ────────────────────────────────────────────────────────

    #[test]
    fn unescape_backslash() {
        assert_eq!(unescape(r"a\\b").unwrap(), r"a\b");
    }

    #[test]
    fn unescape_regex_dot() {
        assert_eq!(unescape(r"^x\\.org$").unwrap(), r"^x\.org$");
    }

    #[test]
    fn unescape_quote() {
        assert_eq!(unescape(r#"say \"hi\""#).unwrap(), r#"say "hi""#);
    }

    #[test]
    fn unescape_newline_tab_cr_null() {
        assert_eq!(unescape(r"a\nb\tc\rd\0e").unwrap(), "a\nb\tc\rd\0e");
    }

    #[test]
    fn unescape_dollar() {
        assert_eq!(unescape(r"price \$5").unwrap(), "price $5");
    }

    #[test]
    fn unescape_unknown_escape_errors() {
        assert!(unescape(r"\q").is_err());
    }

    #[test]
    fn unescape_dangling_backslash_errors() {
        assert!(unescape("trailing\\").is_err());
    }

    #[test]
    fn unescape_no_escapes_passthrough() {
        assert_eq!(unescape("plain text").unwrap(), "plain text");
    }

    // ── string-literal lowering integrates unescape ────────────────────────

    #[test]
    fn double_quoted_string_unescapes() {
        // End-to-end check that lower_string_fragments runs unescape on
        // double-quoted strings — the var default `"a\\b"` should arrive as
        // the two-char string `a\b`.
        let source = "\
actor a = args_actor,
  vars:
    name: \"a\\\\b\",
";
        let file = lower(source).expect("should lower");
        match &file.items[0] {
            TopLevel::ActorDeclaration(decl) => {
                let default = decl.vars[0].default.as_ref().expect("default expr");
                let Expr::StringLit(lit) = default else {
                    panic!("expected string literal, got {default:?}");
                };
                // Grammar splits text on escape boundaries; concatenate to
                // see the post-unescape value the runtime would build.
                let combined: String = lit
                    .fragments
                    .iter()
                    .map(|f| match f {
                        StringFragment::Text(t) => t.as_str(),
                        _ => "",
                    })
                    .collect();
                assert_eq!(combined, r"a\b");
            }
            _ => panic!("expected actor declaration"),
        }
    }

    // ── Parse-error dedupe (regression: each ERROR used to emit two diagnostics) ──

    #[test]
    fn parse_error_emits_one_diagnostic_per_error_node() {
        // `@@@` mid-block forces tree-sitter into ERROR recovery. The old
        // collect_errors+fall-through path emitted both a TreeSitterError AND
        // an UnexpectedNode for the same span. New code must not.
        let source = "\
actor a = container
as a:
  @@@
";
        let errs = lower(source).expect_err("should fail to lower");

        let mut spans: Vec<(usize, usize)> =
            errs.iter().map(|d| (d.span.start, d.span.end)).collect();
        spans.sort();
        let dedup_len = {
            let mut s = spans.clone();
            s.dedup();
            s.len()
        };
        assert_eq!(
            spans.len(),
            dedup_len,
            "two diagnostics share a span — dedupe regression: {errs:?}"
        );

        // No diagnostic for an ERROR site should leak through as UnexpectedNode
        // (that's the lower_block fall-through case the dedupe fix targets).
        assert!(
            !errs
                .iter()
                .any(|d| d.code == DiagnosticCode::UnexpectedNode
                    && d.message.contains("ERROR")),
            "leaked tree-sitter ERROR terminology as UnexpectedNode: {errs:?}"
        );
    }

    #[test]
    fn parse_error_carries_parent_context_note() {
        // Garbage inside a known-good command position parks the ERROR under a
        // named parent (`command` / `block`), so the "while parsing a `X`"
        // footer should fire on at least one diagnostic.
        let source = "\
actor a = container
as a:
  receive @@@bogus@@@
";
        let errs = lower(source).expect_err("should fail to lower");
        let any_with_note = errs.iter().any(|d| {
            d.code == DiagnosticCode::ParseError
                && d.notes.iter().any(|n| n.starts_with("while parsing a"))
        });
        assert!(
            any_with_note,
            "expected at least one ParseError with a `while parsing a ...` note, got: {errs:?}"
        );
    }

    #[test]
    fn invalid_escape_surfaces_as_diagnostic() {
        // Confirms the bad-escape error reaches the user with a readable
        // message — no "invalid literal `unknown escape...`" double-wrapping.
        let source = "\
actor a = args_actor,
  vars:
    name: \"oops \\q here\",
";
        let errs = lower(source).expect_err("should fail to lower");
        let rendered: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        assert!(
            rendered.iter().any(|s| s.contains("unknown escape sequence")
                && s.contains("\\q")
                && !s.contains("invalid literal")),
            "expected a clean InvalidEscape diagnostic, got: {rendered:?}"
        );
    }
}
