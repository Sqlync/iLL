use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

use ill_core::diagnostic::{Diagnostic, Severity};
use ill_core::render;
use ill_core::runtime::report::{StatementReport, TestReport};

const ILL_EXTENSION: &str = "ill";

fn is_ill_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some(ILL_EXTENSION)
}

#[derive(Parser)]
#[command(name = "ill", about = "iLL — integration Logic Language")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run .ill test files.
    ///
    /// Accepts any number of files and/or directories. Directories are searched
    /// recursively for .ill files. With no arguments, searches the current
    /// directory recursively.
    Test {
        /// Files or directories to test. Defaults to the current directory.
        paths: Vec<PathBuf>,
        /// Supply a command-line argument to the `args_actor` in the form
        /// `KEY=VALUE`. Repeat for multiple values.
        #[arg(long = "arg", value_name = "KEY=VALUE")]
        args: Vec<String>,
    },
    /// Check .ill files for errors without running them.
    ///
    /// Accepts any number of files and/or directories. Directories are searched
    /// recursively for .ill files. With no arguments, searches the current
    /// directory recursively.
    Check {
        /// Files or directories to check. Defaults to the current directory.
        paths: Vec<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { paths, args } => {
            let cli_args = match parse_cli_args(&args) {
                Ok(m) => m,
                Err(msg) => {
                    eprintln!("ill: {msg}");
                    process::exit(1);
                }
            };
            ill_core::actor_type::args_actor::set_cli_args(cli_args);
            run_test(&paths).await;
        }
        Commands::Check { paths } => run_check(&paths),
    }
}

/// Parse `--arg KEY=VALUE` entries into a map. Duplicate keys error out —
/// silently picking one would make test runs unpredictable.
fn parse_cli_args(raw: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for item in raw {
        let Some((k, v)) = item.split_once('=') else {
            return Err(format!("--arg `{item}` must be in the form KEY=VALUE"));
        };
        let key = k.trim();
        if key.is_empty() {
            return Err(format!("--arg `{item}` has an empty key"));
        }
        if out.insert(key.to_string(), v.to_string()).is_some() {
            return Err(format!("--arg `{key}` specified more than once"));
        }
    }
    Ok(out)
}

/// Resolve a list of user-supplied paths into a sorted list of .ill files.
/// Directories are searched recursively. Non-.ill files are skipped with a warning.
/// Returns an empty Vec if nothing was found (callers decide how to handle that).
fn resolve_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    if paths.is_empty() {
        return collect_ill_files(Path::new("."));
    }

    let mut all = Vec::new();
    for p in paths {
        if p.is_dir() {
            all.extend(collect_ill_files(p));
        } else if !is_ill_file(p) {
            eprintln!("ill: skipping {}: not a .ill file", p.display());
        } else {
            all.push(p.clone());
        }
    }
    all
}

async fn run_test(paths: &[PathBuf]) {
    let files = resolve_files(paths);

    if files.is_empty() {
        eprintln!("ill: no .ill files found");
        process::exit(1);
    }

    let mut passed = 0;
    let mut failed = 0;

    for path in &files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error reading {}: {e}", path.display());
                failed += 1;
                continue;
            }
        };

        let report = ill_core::runtime::harness::run_test_file(path, &src).await;
        if report.passed {
            println!("PASS {}", path.display());
            passed += 1;
        } else {
            print_failed_report(&report, &src);
            failed += 1;
        }
    }

    println!("{passed} passed, {failed} failed");

    if failed > 0 {
        process::exit(1);
    }
}

/// Render diagnostics with a plain-text fallback if the rich renderer fails
/// (e.g. EPIPE on stderr). Better to lose color than to silently drop the
/// diagnostic body.
fn render_or_fallback(path: &Path, source: &str, diags: &[Diagnostic]) {
    let mut stderr = render::stderr_writer();
    if let Err(e) = render::render(path, source, diags, &mut stderr) {
        eprintln!("ill: failed to render diagnostics: {e}");
        for d in diags {
            eprintln!("  {d}");
        }
    }
}

fn print_failed_report(report: &TestReport, source: &str) {
    eprintln!("FAIL {}", report.path.display());
    for s in &report.statements {
        match s {
            StatementReport::ParseFailure(diags) | StatementReport::ValidationFailure(diags) => {
                render_or_fallback(&report.path, source, diags);
            }
            StatementReport::ConstructFailure {
                actor,
                message,
                span,
            } => {
                eprintln!(
                    "  [{}..{}] construction failed for `{actor}`: {message}",
                    span.start, span.end
                );
            }
            StatementReport::CommandFailure {
                actor,
                command,
                span,
                error_fields,
                expect,
            } => {
                eprintln!(
                    "  [{}..{}] {actor}: `{command}` failed",
                    span.start, span.end
                );
                for (k, v) in error_fields {
                    eprintln!("    error.{k} = {v}");
                }
                if let Some(e) = expect {
                    eprintln!("    @expect {e:?}");
                }
            }
            StatementReport::CommandNotImplemented {
                actor,
                command,
                span,
            } => {
                eprintln!(
                    "  [{}..{}] {actor}: `{command}` has no runtime implementation",
                    span.start, span.end
                );
            }
            StatementReport::AssertFailure {
                actor,
                span,
                left,
                right,
                op,
                expect,
            } => {
                eprintln!("  [{}..{}] {actor}: assertion failed", span.start, span.end);
                if let (Some(op), Some(right)) = (op, right) {
                    eprintln!("    left:  {left}");
                    eprintln!("    op:    {op:?}");
                    eprintln!("    right: {right}");
                } else {
                    eprintln!("    value: {left} (not truthy)");
                }
                if let Some(e) = expect {
                    eprintln!("    @expect {e:?}");
                }
            }
            StatementReport::EvalError {
                actor,
                span,
                message,
            } => {
                eprintln!("  [{}..{}] {actor}: {message}", span.start, span.end);
            }
        }
    }
    for t in &report.teardown {
        if !t.outcome.ok {
            eprintln!(
                "  teardown {}: {}",
                t.actor,
                t.outcome.message.as_deref().unwrap_or("failed")
            );
        }
    }
}

fn run_check(paths: &[PathBuf]) {
    let files = resolve_files(paths);

    if files.is_empty() {
        eprintln!("ill: no .ill files found");
        process::exit(1);
    }

    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;
    let mut hint_count = 0;

    for path in &files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: error reading file: {e}", path.display());
                error_count += 1;
                continue;
            }
        };

        match ill_core::lower::lower(&src) {
            Ok(ast) => {
                let diags = ill_core::validate::validate(&ast);
                render_or_fallback(path, &src, &diags);
                for d in &diags {
                    match d.severity {
                        Severity::Error => error_count += 1,
                        Severity::Warning => warning_count += 1,
                        Severity::Info => info_count += 1,
                        Severity::Hint => hint_count += 1,
                    }
                }
            }
            Err(errors) => {
                render_or_fallback(path, &src, &errors);
                error_count += errors.len();
            }
        }
    }

    let file_count = files.len();
    let file_word = if file_count == 1 { "file" } else { "files" };

    if error_count == 0 && warning_count == 0 && info_count == 0 && hint_count == 0 {
        println!("ok — {file_count} {file_word} checked");
    } else {
        let mut parts = Vec::new();
        if error_count > 0 {
            let word = if error_count == 1 { "error" } else { "errors" };
            parts.push(format!("{error_count} {word}"));
        }
        if warning_count > 0 {
            let word = if warning_count == 1 {
                "warning"
            } else {
                "warnings"
            };
            parts.push(format!("{warning_count} {word}"));
        }
        if info_count > 0 {
            parts.push(format!("{info_count} info"));
        }
        if hint_count > 0 {
            let word = if hint_count == 1 { "hint" } else { "hints" };
            parts.push(format!("{hint_count} {word}"));
        }
        println!("{} in {file_count} {file_word}", parts.join(", "));
    }

    if error_count > 0 {
        process::exit(1);
    }
}

fn collect_ill_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_ill_files_inner(dir, &mut out);
    out.sort();
    out
}

fn collect_ill_files_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ill: cannot read directory {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("ill: error reading directory entry: {e}");
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            collect_ill_files_inner(&path, out);
        } else if is_ill_file(&path) {
            out.push(path);
        }
    }
}
