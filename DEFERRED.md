# Deferred implementation work

Known gaps and implementation followups. Picked up when there's concrete need.
For architecture-level roadmap items see `design/ROADMAP.md`.

- [ ] Process supervision / orphan prevention — exec teardown today is layered (explicit SIGTERM→grace→SIGKILL in `Running::stop`, `KillOnDrop` guard, `catch_unwind`, `InstantiatedActors` Drop), but all of it runs inside the runner's own process. SIGKILL or abort of the runner bypasses every destructor and leaks child processes. Needed once actors hold real resources (DB servers, HTTP listeners, anything with a port or file lock). Likely shape: `setpgid` in `pre_exec` on all unix, `PR_SET_PDEATHSIG` on Linux, re-exec-self supervisor subcommand on macOS watching the parent via pipe-EOF or kqueue.
- [ ] Actor-declared var runtime binding — the AST supports `var name` / `var name = default` in actor declarations, and the validator tracks their types, but the runtime never evaluates defaults or populates them into the `as` block scope. No example uses vars yet; when the first one does, `self.myvar` / bare `myvar` will fail with "undefined name". Shape TBD (fresh vars per block vs. shared, assignment mutation, default-expr scope).
- [ ] Exec stdout/stderr capture — current `exec` actor inherits stdout/stderr to the terminal. A future command will read them (e.g. `assert stdout contains "ready"`). Needs a bounded-buffer strategy (ring buffer, truncation marker) to avoid OOM on chatty children.
