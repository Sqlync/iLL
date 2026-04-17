use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

use ill_core::diagnostic::Severity;

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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { paths } => run_test(&paths),
        Commands::Check { paths } => run_check(&paths),
    }
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

fn run_test(paths: &[PathBuf]) {
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

        match ill_core::lower::lower(&src) {
            Ok(ast) => {
                let diags = ill_core::validate::validate(&ast);
                let errors: Vec<_> = diags
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .collect();
                if errors.is_empty() {
                    passed += 1;
                } else {
                    eprintln!("FAIL {}", path.display());
                    for d in &errors {
                        eprintln!("  {d}");
                    }
                    failed += 1;
                }
            }
            Err(errors) => {
                eprintln!("FAIL {}", path.display());
                for e in &errors {
                    eprintln!("  {e}");
                }
                failed += 1;
            }
        }
    }

    println!("{passed} passed, {failed} failed");

    if failed > 0 {
        process::exit(1);
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
                for d in &diags {
                    eprintln!("{}: {d}", path.display());
                    match d.severity {
                        Severity::Error => error_count += 1,
                        Severity::Warning => warning_count += 1,
                        Severity::Info => info_count += 1,
                        Severity::Hint => hint_count += 1,
                    }
                }
            }
            Err(errors) => {
                for e in &errors {
                    eprintln!("{}: {e}", path.display());
                }
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
