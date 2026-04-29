// Diagnostic renderer. One source of truth for how `Diagnostic`s look on the
// terminal — both `ill check` and `ill test` route through here so the two
// commands can't drift.

use std::path::Path;

use codespan_reporting::diagnostic::{Diagnostic as CsrDiagnostic, Label};
use codespan_reporting::files::SimpleFile;
use codespan_reporting::term::{self, termcolor::WriteColor, Config};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};

/// Render a batch of diagnostics for a single source file. The path is used for
/// the snippet header; the source string is needed to resolve byte offsets to
/// lines/columns.
pub fn render(
    path: &Path,
    source: &str,
    diags: &[Diagnostic],
    writer: &mut dyn WriteColor,
) -> Result<(), codespan_reporting::files::Error> {
    let file = SimpleFile::new(path.display().to_string(), source);
    let config = Config::default();
    for d in diags {
        let csr = to_csr(d);
        term::emit_to_write_style(writer, &config, &file, &csr)?;
    }
    Ok(())
}

fn to_csr(d: &Diagnostic) -> CsrDiagnostic<()> {
    let severity = match d.severity {
        Severity::Error => codespan_reporting::diagnostic::Severity::Error,
        Severity::Warning => codespan_reporting::diagnostic::Severity::Warning,
        Severity::Info => codespan_reporting::diagnostic::Severity::Note,
        Severity::Hint => codespan_reporting::diagnostic::Severity::Help,
    };

    // codespan-reporting refuses to render an empty range, so widen any
    // zero-length span (typical for MISSING tokens) by one byte.
    let range = if d.span.start == d.span.end {
        d.span.start..d.span.end + 1
    } else {
        d.span.start..d.span.end
    };

    CsrDiagnostic::new(severity)
        .with_message(&d.message)
        .with_code(code_str(d.code))
        .with_labels(vec![Label::primary((), range)])
        .with_notes(d.notes.clone())
}

fn code_str(code: DiagnosticCode) -> &'static str {
    match code {
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
    }
}
