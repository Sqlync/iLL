// Render a `TestReport` as a human-readable failure message — used by both the
// `ill` CLI (writing to stderr) and external embedders that bundle the harness
// via cargo and need a `String` for `panic!`.
//
// Parse and validation diagnostics flow through `render::render` directly.
// Runtime failures (assertion mismatches, command errors, etc.) are turned
// into synthetic `Diagnostic`s tagged `DiagnosticCode::RuntimeFailure` so they
// share the source-snippet treatment.
//
// Caller is responsible for guarding with `report.passed` — these always emit
// a `FAIL <path>` header.

use std::io;

use codespan_reporting::term::termcolor::NoColor;

use crate::ast::Span;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::render;

use super::report::{StatementReport, TestReport};

/// Stream a failure report to `w`. Mirrors `render::render`'s shape so the two
/// renderers feel like siblings.
pub fn write_failure(report: &TestReport, src: &str, w: &mut dyn io::Write) -> io::Result<()> {
    writeln!(w, "FAIL {}", report.path.display())?;

    let mut diags: Vec<Diagnostic> = Vec::new();
    for s in &report.statements {
        match s {
            StatementReport::ParseFailure(ds) | StatementReport::ValidationFailure(ds) => {
                diags.extend(ds.iter().cloned());
            }
            StatementReport::ConstructFailure {
                actor,
                message,
                span,
            } => {
                diags.push(synth_diag(
                    *span,
                    format!("construction failed for `{actor}`: {message}"),
                    Vec::new(),
                ));
            }
            StatementReport::CommandFailure {
                actor,
                command,
                span,
                error_fields,
                expect,
            } => {
                let mut notes: Vec<String> = error_fields
                    .iter()
                    .map(|(k, v)| format!("error.{k} = {v}"))
                    .collect();
                if let Some(e) = expect {
                    notes.push(format!("@expect {e}"));
                }
                diags.push(synth_diag(
                    *span,
                    format!("{actor}: `{command}` failed"),
                    notes,
                ));
            }
            StatementReport::CommandNotImplemented {
                actor,
                command,
                span,
            } => {
                diags.push(synth_diag(
                    *span,
                    format!("{actor}: `{command}` has no runtime implementation"),
                    Vec::new(),
                ));
            }
            StatementReport::AssertFailure {
                actor,
                span,
                left,
                right,
                op,
                expect,
            } => {
                let mut notes = Vec::new();
                if let (Some(op), Some(right)) = (op, right) {
                    notes.push(format!("left:  {left}"));
                    notes.push(format!("op:    {op}"));
                    notes.push(format!("right: {right}"));
                } else {
                    notes.push(format!("value: {left} (not truthy)"));
                }
                if let Some(e) = expect {
                    notes.push(format!("@expect {e}"));
                }
                diags.push(synth_diag(
                    *span,
                    format!("{actor}: assertion failed"),
                    notes,
                ));
            }
            StatementReport::EvalError {
                actor,
                span,
                message,
            } => {
                diags.push(synth_diag(*span, format!("{actor}: {message}"), Vec::new()));
            }
        }
    }

    if !diags.is_empty() {
        let mut buf = NoColor::new(Vec::new());
        match render::render(&report.path, src, &diags, &mut buf) {
            Ok(()) => w.write_all(&buf.into_inner())?,
            Err(e) => {
                writeln!(w, "  failed to render diagnostics: {e}")?;
                for d in &diags {
                    writeln!(w, "  [{}..{}] {}", d.span.start, d.span.end, d.message)?;
                    for note in &d.notes {
                        writeln!(w, "    {note}")?;
                    }
                }
            }
        }
    }

    for t in &report.teardown {
        if !t.outcome.ok {
            writeln!(
                w,
                "  teardown {}: {}",
                t.actor,
                t.outcome.message.as_deref().unwrap_or("failed")
            )?;
        }
    }
    Ok(())
}

/// Convenience wrapper for callers that want a `String` (e.g. `panic!`).
pub fn format_failure(report: &TestReport, src: &str) -> String {
    let mut buf: Vec<u8> = Vec::new();
    write_failure(report, src, &mut buf).expect("Vec write is infallible");
    String::from_utf8(buf).expect("renderer emits utf-8")
}

fn synth_diag(span: Span, message: String, notes: Vec<String>) -> Diagnostic {
    let mut d = Diagnostic::error(span, DiagnosticCode::RuntimeFailure, message);
    for note in notes {
        d = d.with_note(note);
    }
    d
}
