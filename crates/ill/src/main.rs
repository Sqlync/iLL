use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

const ILL_EXTENSION: &str = "ill";

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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { paths } => run_test(&paths),
    }
}

fn run_test(paths: &[PathBuf]) {
    let files = if paths.is_empty() {
        collect_ill_files(Path::new("."))
    } else {
        let mut all = Vec::new();
        for p in paths {
            if p.is_dir() {
                all.extend(collect_ill_files(p));
            } else {
                all.push(p.clone());
            }
        }
        all
    };

    if files.is_empty() {
        eprintln!("ill: no .ill files found");
        process::exit(1);
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

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
                println!("=== {} ===", path.display());
                println!("{ast:#?}");
                println!();
                passed += 1;
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
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ill_files_inner(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some(ILL_EXTENSION) {
            out.push(path);
        }
    }
}
