// Diagnostics produced by lowering and validation. Shaped so Phase 6's LSP can
// consume them directly without reshaping. Both the parser (lower.rs) and the
// validator emit `Diagnostic` so the CLI renderer has a single input type.

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
    // Parse / lower errors
    ParseError,
    MissingToken,
    UnexpectedNode,
    MissingField,
    InvalidLiteral,
    InvalidEscape,

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

    // Squiggles
    UnknownSquiggle,

    // Runtime (test execution failures rendered as diagnostics)
    RuntimeFailure,
}

impl DiagnosticCode {
    /// Stable string form for the code, used by the renderer and (eventually)
    /// LSP `codeDescription` links. Numeric ranges are namespaced by category:
    /// 000x parse/lower, 010x names, 020x commands, 030x types, 040x squiggles,
    /// 050x runtime.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticCode::ParseError => "E0001",
            DiagnosticCode::MissingToken => "E0002",
            DiagnosticCode::UnexpectedNode => "E0003",
            DiagnosticCode::MissingField => "E0004",
            DiagnosticCode::InvalidLiteral => "E0005",
            DiagnosticCode::InvalidEscape => "E0006",
            DiagnosticCode::UnknownActorType => "E0101",
            DiagnosticCode::UnknownActor => "E0102",
            DiagnosticCode::DuplicateActor => "E0103",
            DiagnosticCode::UnknownCommand => "E0201",
            DiagnosticCode::CommandNotValidInMode => "E0202",
            DiagnosticCode::MissingRequiredArg => "E0203",
            DiagnosticCode::UnknownKeywordArg => "E0204",
            DiagnosticCode::ConflictingOutcomeAsserts => "E0205",
            DiagnosticCode::TypeMismatch => "E0301",
            DiagnosticCode::UnknownSquiggle => "E0401",
            DiagnosticCode::RuntimeFailure => "E0501",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    /// Footer notes shown below the source snippet — hints, suggestions, related
    /// info. Renderers display each on its own line.
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(span: Span, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            code,
            message: message.into(),
            notes: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
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
