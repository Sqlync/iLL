// Render a `TestReport` as a human-readable failure message — used by both the
// `ill` CLI and external embedders that bundle the harness via cargo and need
// a `String` for `panic!`.
//
// Runtime failures are turned into synthetic `Diagnostic`s tagged
// `RuntimeFailure` so they share `render::render`'s source-snippet treatment.
//
// Caller is responsible for guarding with `report.passed` — these always emit
// a `FAIL <path>` header.

use std::io;

use codespan_reporting::term::termcolor::{NoColor, WriteColor};

use crate::ast::Span;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::render;
use crate::runtime::report::{StatementReport, TestReport};

/// Stream a failure report to `w`. Takes `WriteColor` so terminal output keeps
/// the same color treatment as `render::render`.
pub fn write_failure(
    report: &TestReport,
    src: &str,
    w: &mut dyn WriteColor,
) -> io::Result<()> {
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
            } => diags.push(synth_diag(
                *span,
                format!("construction failed for `{actor}`: {message}"),
                Vec::new(),
            )),
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
            } => diags.push(synth_diag(
                *span,
                format!("{actor}: `{command}` has no runtime implementation"),
                Vec::new(),
            )),
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
            } => diags.push(synth_diag(*span, format!("{actor}: {message}"), Vec::new())),
        }
    }

    if !diags.is_empty() {
        render::render_with_fallback(&report.path, src, &diags, w);
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
/// `codespan-reporting` writes UTF-8 because it indexes a `&str`, so the
/// final `from_utf8` is sound.
pub fn format_failure(report: &TestReport, src: &str) -> String {
    let mut buf = NoColor::new(Vec::new());
    write_failure(report, src, &mut buf).expect("Vec write is infallible");
    String::from_utf8(buf.into_inner()).expect("renderer emits utf-8")
}

fn synth_diag(span: Span, message: String, notes: Vec<String>) -> Diagnostic {
    notes.into_iter().fold(
        Diagnostic::error(span, DiagnosticCode::RuntimeFailure, message),
        Diagnostic::with_note,
    )
}
