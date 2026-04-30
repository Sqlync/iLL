// Render a `TestReport` as a single human-readable string suitable for
// `panic!` in test harnesses.
//
// Parse and validation diagnostics flow through `render::render` directly.
// Runtime failures (assertion mismatches, command errors, etc.) are turned
// into synthetic `Diagnostic`s so they get the same source-snippet treatment
// — the underlying `DiagnosticCode` is unused by the renderer.
//
// Caller is responsible for guarding with `report.passed` — this always
// emits a `FAIL <path>` header.

use codespan_reporting::term::termcolor::NoColor;

use crate::ast::Span;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::render;

use super::report::{StatementReport, TestReport};

pub fn format_failure(report: &TestReport, src: &str) -> String {
    let mut out = format!("FAIL {}\n", report.path.display());
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
                    notes.push(format!("@expect {e:?}"));
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
                    notes.push(format!("op:    {op:?}"));
                    notes.push(format!("right: {right}"));
                } else {
                    notes.push(format!("value: {left} (not truthy)"));
                }
                if let Some(e) = expect {
                    notes.push(format!("@expect {e:?}"));
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
            Ok(()) => out.push_str(&String::from_utf8_lossy(&buf.into_inner())),
            Err(e) => {
                out.push_str(&format!("  failed to render diagnostics: {e}\n"));
                for d in &diags {
                    out.push_str(&format!(
                        "  [{}..{}] {}\n",
                        d.span.start, d.span.end, d.message
                    ));
                }
            }
        }
    }

    for t in &report.teardown {
        if !t.outcome.ok {
            out.push_str(&format!(
                "  teardown {}: {}\n",
                t.actor,
                t.outcome.message.as_deref().unwrap_or("failed")
            ));
        }
    }
    out
}

fn synth_diag(span: Span, message: String, notes: Vec<String>) -> Diagnostic {
    let mut d = Diagnostic::error(span, DiagnosticCode::ParseError, message);
    for note in notes {
        d = d.with_note(note);
    }
    d
}
