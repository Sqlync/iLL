// Diagnostics produced by the validator. Shaped so Phase 6's LSP can consume
// them directly without reshaping.

use std::fmt;

use crate::ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Machine-readable diagnostic code. Used by the LSP to attach fixes and by
/// tests to assert on specific errors without matching on message strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    // Name resolution
    UnknownActorType,
    UnknownActor,
    DuplicateActor,

    // Commands
    UnknownCommand,
    CommandNotValidInMode,
    MissingRequiredArg,
    UnknownKeywordArg,
    ConflictingOutcomeAsserts,

    // Types (narrow scope for Phase 4)
    TypeMismatch,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
}

impl Diagnostic {
    pub fn error(span: Span, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        })
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at [{}..{}]: {}",
            self.severity, self.span.start, self.span.end, self.message
        )
    }
}
