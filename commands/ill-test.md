---
description: Validate and run iLL tests at the given path (defaults to current directory).
argument-hint: "[path]"
---

Run iLL tests, validating first.

1. If `$ARGUMENTS` is empty, set `TARGET=.`; otherwise `TARGET=$ARGUMENTS`.
2. Run `ill check "$TARGET"`. If it exits non-zero, stop and report the diagnostics — do not run the tests.
3. If `check` passes, run `ill test "$TARGET"` and report the pass/fail summary.

If the `ill` binary isn't on PATH, ask the user how they invoke it in this project (common alternatives: a workspace-local shim, `cargo run -p ill --` from the iLL repo).
