// TestReport — the structured result of running a single .ill file.

use std::path::PathBuf;

use crate::ast::{ComparisonOp, Span};
use crate::diagnostic::Diagnostic;

use super::{Record, Value};

pub struct TestReport {
    pub path: PathBuf,
    pub passed: bool,
    pub statements: Vec<StatementReport>,
    pub teardown: Vec<TeardownReport>,
}

/// Per-statement result. Only failures carry detail — success is implicit in
/// the test passing.
pub enum StatementReport {
    ValidationFailure(Vec<Diagnostic>),
    ParseFailure(Vec<String>),
    ConstructFailure {
        actor: String,
        message: String,
        span: Span,
    },
    CommandFailure {
        actor: String,
        command: String,
        span: Span,
        error_fields: Record,
        expect: Option<String>,
    },
    CommandNotImplemented {
        actor: String,
        command: String,
        span: Span,
    },
    AssertFailure {
        actor: String,
        span: Span,
        left: Value,
        right: Option<Value>,
        op: Option<ComparisonOp>,
        expect: Option<String>,
    },
    EvalError {
        actor: String,
        span: Span,
        message: String,
    },
}

pub struct TeardownReport {
    pub actor: String,
    pub outcome: super::TeardownOutcome,
}
