// Integration tests for the `ill test` validation gate.
//
// Runs the compiled binary against fixture files in a temp directory and
// asserts on exit code + key substrings in stdout/stderr. Substring matches
// (rather than full-output equality) keep the tests resilient to renderer
// formatting tweaks while still pinning the contract callers care about:
// what header format prints, when the suite is refused, what's silent on
// the happy path.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ill");

/// Minimal scoped temp dir. Removed on drop. Avoids pulling in `tempfile`
/// just for these smoke tests.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ill-cli-gate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, contents).expect("write fixture");
        p
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RunResult {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_ill(args: &[&str]) -> RunResult {
    let output = Command::new(BIN)
        .args(args)
        .output()
        .expect("spawn ill binary");
    RunResult {
        code: output.status.code().expect("ill exited via signal"),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

const GOOD_ILL: &str = "actor a = args_actor\n";
const BAD_ILL: &str = "actor x = nope_actor\n\nas x:\n  run\n";

#[test]
fn happy_path_emits_no_validation_output() {
    let tmp = TempDir::new("happy");
    let path = tmp.write("good.ill", GOOD_ILL);

    let r = run_ill(&["test", path.to_str().unwrap()]);

    assert_eq!(r.code, 0, "stderr: {}", r.stderr);
    assert!(r.stdout.contains("PASS"), "stdout: {}", r.stdout);
    assert!(
        !r.stderr.contains("failed validation"),
        "gate header should be silent on the happy path; stderr: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("not running tests"),
        "refusal line should not print on success; stderr: {}",
        r.stderr
    );
}

#[test]
fn single_failing_file_uses_singular_header() {
    let tmp = TempDir::new("single-fail");
    let path = tmp.write("bad.ill", BAD_ILL);

    let r = run_ill(&["test", path.to_str().unwrap()]);

    assert_eq!(r.code, 1);
    let header = format!("ill: {} failed validation", path.display());
    assert!(
        r.stderr.contains(&header),
        "expected singular header `{header}`; got stderr: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("files failed validation"),
        "should not use plural header for one file; stderr: {}",
        r.stderr
    );
    assert!(r.stderr.contains("ill: not running tests"));
    // Diagnostic body should appear too — the validator emits the unknown-type
    // message; pin on the actor-type substring rather than exact wording.
    assert!(
        r.stderr.contains("nope_actor"),
        "diagnostic body missing; stderr: {}",
        r.stderr
    );
}

#[test]
fn multi_failing_files_list_every_path() {
    let tmp = TempDir::new("multi-fail");
    let p1 = tmp.write("bad1.ill", BAD_ILL);
    let p2 = tmp.write("bad2.ill", "actor y = also_nope\n\nas y:\n  run\n");

    let r = run_ill(&["test", tmp.path().to_str().unwrap()]);

    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("ill: 2 files failed validation:"),
        "expected plural header; stderr: {}",
        r.stderr
    );
    for p in [&p1, &p2] {
        assert!(
            r.stderr.contains(&p.display().to_string()),
            "expected path `{}` in listing; stderr: {}",
            p.display(),
            r.stderr
        );
    }
    assert!(r.stderr.contains("ill: not running tests"));
}

#[test]
fn one_bad_file_blocks_the_clean_ones() {
    let tmp = TempDir::new("mixed");
    tmp.write("good.ill", GOOD_ILL);
    let bad = tmp.write("bad.ill", BAD_ILL);

    let r = run_ill(&["test", tmp.path().to_str().unwrap()]);

    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains(&format!("ill: {} failed validation", bad.display())),
        "stderr: {}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("PASS"),
        "good file ran despite gate failure; stdout: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("passed,") && !r.stdout.contains("failed"),
        "run summary printed despite gate failure; stdout: {}",
        r.stdout
    );
}
