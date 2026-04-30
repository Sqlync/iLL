// AST types for iLL — the integration logic language.
//
// These types represent the semantic structure of an iLL program after lowering
// from the tree-sitter CST. Commands are generic (name + args); actor-specific
// validation belongs in a later pass.

// ── Spans ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

// ── Identifiers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

// ── Top level ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub items: Vec<TopLevel>,
}

#[derive(Debug, Clone)]
pub enum TopLevel {
    ActorDeclaration(ActorDeclaration),
    AsBlock(AsBlock),
}

// ── Actors ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ActorDeclaration {
    pub name: Ident,
    pub actor_type: Ident,
    pub keyword_args: Vec<KeywordArg>,
    pub vars: Vec<VarDeclaration>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct VarDeclaration {
    pub annotations: Vec<Annotation>,
    pub name: Ident,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: Ident,
    pub value: Option<String>,
    pub span: Span,
}

// ── As blocks / statements ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AsBlock {
    pub actor: Ident,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Command(Command),
    Assert(Assert),
    Let(Let),
    Assignment(Assignment),
}

#[derive(Debug, Clone)]
pub struct Command {
    pub annotation: Option<Annotation>,
    pub name: Ident,
    pub positional_args: Vec<Expr>,
    pub keyword_args: Vec<KeywordArg>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub target: Expr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Assert {
    pub annotation: Option<Annotation>,
    pub left: Expr,
    pub op: Option<ComparisonOp>,
    pub right: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    Eq,
    NotEq,
    Gt,
    Gte,
    Lt,
    Lte,
    Contains,
    NotContains,
    Matches,
    NotMatches,
}

impl std::fmt::Display for ComparisonOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ComparisonOp::Eq => "==",
            ComparisonOp::NotEq => "!=",
            ComparisonOp::Gt => ">",
            ComparisonOp::Gte => ">=",
            ComparisonOp::Lt => "<",
            ComparisonOp::Lte => "<=",
            ComparisonOp::Contains => "contains",
            ComparisonOp::NotContains => "!contains",
            ComparisonOp::Matches => "matches",
            ComparisonOp::NotMatches => "!matches",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Let {
    pub name: Ident,
    pub value: LetValue,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum LetValue {
    Expr(Expr),
    Parse { source: Expr, format: Ident },
}

// ── Keyword args (shared by actors and commands) ───────────────────────────────

#[derive(Debug, Clone)]
pub struct KeywordArg {
    pub key: Ident,
    pub value: KeywordValue,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum KeywordValue {
    Expr(Expr),
    Map(Vec<(Expr, Expr)>),
}

// ── Expressions ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Ident(Ident),
    StringLit(StringLit),
    Number(i64),
    Bool(bool),
    Atom(Ident),
    Array(Vec<Expr>),
    Squiggle(Squiggle),
    MemberAccess {
        object: Box<Expr>,
        property: Ident,
        span: Span,
    },
    Index {
        object: Box<Expr>,
        indices: Vec<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct Squiggle {
    pub name: Ident,
    pub fragments: Vec<StringFragment>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StringLit {
    pub fragments: Vec<StringFragment>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StringFragment {
    Text(String),
    Interpolation(Expr),
}
