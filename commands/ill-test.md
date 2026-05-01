---
description: Validate and run iLL tests at the given path (defaults to current directory).
argument-hint: "[path]"
---

Run iLL tests at `$ARGUMENTS` (or the current directory if no path was given), validating before running.

First, run `ill check` against the target. If it exits non-zero, stop — report the diagnostics to the user without running the tests, since `ill test` will hit the same errors and the `check` output is more focused.

If `check` passes, run `ill test` against the same target and report the pass/fail summary.

If the `ill` binary isn't on PATH, ask the user how they invoke it in this project — common alternatives are a workspace-local shim or `cargo run -p ill --` from the iLL repo.