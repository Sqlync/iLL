// Tree-sitter CST → AST lowering pass.
//
// Walks the concrete syntax tree produced by tree-sitter-ill and builds the
// typed AST defined in ast.rs. Collects errors rather than bailing on the first
// problem so we can report multiple issues at once.

use crate::ast::*;

// ── Errors ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum LowerError {
    UnexpectedNode {
        kind: String,
        span: Span,
    },
    MissingField {
        parent: String,
        field: String,
        span: Span,
    },
    InvalidLiteral {
        text: String,
        span: Span,
    },
    TreeSitterError {
        span: Span,
    },
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::UnexpectedNode { kind, span } => {
                write!(
                    f,
                    "unexpected node `{}` at {}..{}",
                    kind, span.start, span.end
                )
            }
            LowerError::MissingField {
                parent,
                field,
                span,
            } => {
                write!(
                    f,
                    "missing field `{}` on `{}` at {}..{}",
                    field, parent, span.start, span.end
                )
            }
            LowerError::InvalidLiteral { text, span } => {
                write!(
                    f,
                    "invalid literal `{}` at {}..{}",
                    text, span.start, span.end
                )
            }
            LowerError::TreeSitterError { span } => {
                write!(f, "parse error at {}..{}", span.start, span.end)
            }
        }
    }
}

impl std::error::Error for LowerError {}

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

pub fn lower(source: &str) -> Result<SourceFile, Vec<LowerError>> {
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
    errors: Vec<LowerError>,
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
        if node.is_error() || node.is_missing() {
            self.errors.push(LowerError::TreeSitterError {
                span: self.span(node),
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_errors(child);
        }
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
                _ => {
                    self.errors.push(LowerError::UnexpectedNode {
                        kind: child.kind().to_string(),
                        span: self.span(child),
                    });
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
                _ => {
                    self.errors.push(LowerError::UnexpectedNode {
                        kind: child.kind().to_string(),
                        span: self.span(child),
                    });
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
                self.errors.push(LowerError::InvalidLiteral {
                    text: text.to_string(),
                    span: self.span(node),
                });
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
            "identifier" | "string" | "number" | "boolean" | "atom" | "array" | "sigil" => {
                self.lower_primary(node)
            }
            _ => {
                self.errors.push(LowerError::UnexpectedNode {
                    kind: node.kind().to_string(),
                    span: self.span(node),
                });
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
                        self.errors.push(LowerError::InvalidLiteral {
                            text: text.to_string(),
                            span: self.span(node),
                        });
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
            "sigil" => self.lower_sigil(node),
            "array" => self.lower_array(node),
            _ => {
                self.errors.push(LowerError::UnexpectedNode {
                    kind: node.kind().to_string(),
                    span: self.span(node),
                });
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
        self.collect_fragments(node, "string_content")
    }

    /// Collect `{text_kind | interpolation}*` children into a fragment list.
    /// Shared between strings (`string_content`) and sigils (`sigil_content`).
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

    // ── Sigils ─────────────────────────────────────────────────────────────

    fn lower_sigil(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let name = self.ident_from_node(node.child_by_field_name("name")?);
        let fragments = self.collect_fragments(node, "sigil_content");

        Some(Expr::Sigil(Sigil {
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
            self.errors.push(LowerError::MissingField {
                parent: node.kind().to_string(),
                field: field.to_string(),
                span: self.span(node),
            });
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

    // ── Corpus smoke test ───────────────────────────────────────────────────

    /// Every `.ill` file under examples/ must parse cleanly and lower to an
    /// error-free AST. Catches grammar/scanner/lowering regressions across
    /// the whole corpus in one shot.
    #[test]
    fn all_examples_lower_cleanly() {
        fn visit(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    visit(&p, out);
                } else if p.extension().and_then(|s| s.to_str()) == Some("ill") {
                    out.push(p);
                }
            }
        }

        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut paths = Vec::new();
        visit(&examples_dir, &mut paths);
        paths.sort();
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
}
