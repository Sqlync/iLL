// Diagnostic renderer. One source of truth for how `Diagnostic`s look on the
// terminal — both `ill check` and `ill test` route through here so the two
// commands can't drift.

use std::path::Path;

use codespan_reporting::diagnostic::{Diagnostic as CsrDiagnostic, Label};
use codespan_reporting::files::SimpleFile;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream, WriteColor};
use codespan_reporting::term::{self, Config};

use crate::diagnostic::{Diagnostic, Severity};

/// Build a color-aware stderr writer suitable for [`render`]. Centralised here
/// so callers (CLI, future LSP CLI mode, etc.) don't need a direct termcolor
/// dependency.
pub fn stderr_writer() -> StandardStream {
    StandardStream::stderr(ColorChoice::Auto)
}

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
        .with_code(d.code.as_str())
        .with_labels(vec![Label::primary((), range)])
        .with_notes(d.notes.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codespan_reporting::term::termcolor::NoColor;

    fn render_to_string(path: &str, source: &str, diags: &[Diagnostic]) -> String {
        let mut buf = NoColor::new(Vec::new());
        render(Path::new(path), source, diags, &mut buf).expect("render must succeed");
        String::from_utf8(buf.into_inner()).expect("rendered output must be utf-8")
    }

    #[test]
    fn snapshot_parse_error() {
        // Hand-written diagnostic mimicking what collect_errors produces for
        // a garbage `@@@` mid-block — frozen so renderer changes are visible
        // in review.
        let source = "actor a = container\nas a:\n  @@@\n";
        let span_start = source.find("@@@").unwrap();
        let span_end = span_start + 3;
        let diag = Diagnostic::error(
            crate::ast::Span {
                start: span_start,
                end: span_end,
            },
            crate::diagnostic::DiagnosticCode::ParseError,
            "unexpected `@@@`",
        )
        .with_note("while parsing a `block`");
        insta::assert_snapshot!(render_to_string("demo.ill", source, &[diag]));
    }

    #[test]
    fn snapshot_validation_error() {
        let source = "actor bob = unknownthing\n";
        let diag = Diagnostic::error(
            crate::ast::Span { start: 12, end: 24 },
            crate::diagnostic::DiagnosticCode::UnknownActorType,
            "unknown actor type `unknownthing`",
        );
        insta::assert_snapshot!(render_to_string("demo.ill", source, &[diag]));
    }

    #[test]
    fn snapshot_zero_length_span_widens() {
        // MISSING tokens come through with start == end. Renderer must widen
        // by one to satisfy codespan-reporting and still produce a usable caret.
        let source = "actor a = container\n";
        let diag = Diagnostic::error(
            crate::ast::Span { start: 5, end: 5 },
            crate::diagnostic::DiagnosticCode::MissingToken,
            "missing `=` here",
        );
        insta::assert_snapshot!(render_to_string("demo.ill", source, &[diag]));
    }
}
