use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("test") => {
            let paths: Vec<&str> = args[2..].iter().map(String::as_str).collect();
            run_test(&paths);
        }
        Some(cmd) => {
            eprintln!("ill: unknown command `{cmd}`");
            eprintln!("usage: ill test [paths...]");
            process::exit(1);
        }
        None => {
            eprintln!("usage: ill test [paths...]");
            process::exit(1);
        }
    }
}

fn run_test(paths: &[&str]) {
    let files = if paths.is_empty() {
        collect_ill_files(Path::new("."))
    } else {
        let mut all = Vec::new();
        for p in paths {
            let path = Path::new(p);
            if path.is_dir() {
                all.extend(collect_ill_files(path));
            } else {
                all.push(path.to_path_buf());
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
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ill_files_inner(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("ill") {
            out.push(path);
        }
    }
}
