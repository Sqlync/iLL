---
name: ill-tests
description: Author, edit, and debug iLL integration tests (.ill files). Use whenever the user is creating or modifying a `.ill` test, asks about the iLL language, declares an actor (`actor x = pg_client`, `exec`, `container`, `http_client`, `mqtt_client`, `args_actor`), references squiggles like `~sql`/`~json`/`~re`/`~hex`/`~b`, runs `ill test` or `ill check`, or sees an iLL diagnostic. SKIP for unrelated test frameworks (pytest, jest, cargo test, etc.) even when the word "test" appears.
---

# Authoring iLL tests

iLL is a scripting language for multi-actor, multi-system integration tests. Each `.ill` file is one test. You declare actors at the top, then drive them inside `as <actor>:` blocks. The runtime (`ill test`) brings actors up, executes the script, and tears them down. The validator (`ill check`) reports diagnostics without running anything.

This skill is for writing and fixing `.ill` files. If the user is debugging the iLL toolchain itself (Rust crates, tree-sitter grammar, LSP), defer to normal code-reading — this skill is about the language surface.

## Workflow

1. **Validate before claiming success.** Run `ill check <path>` after editing. It exits non-zero on errors and is fast — use it the way you'd use `tsc --noEmit` or `cargo check`. If `ill` is not on PATH, ask the user how to invoke it (often `cargo run -p ill -- check ...` from the iLL repo, or a shimmed script in their project).
2. **Run the test if appropriate.** `ill test <path>` runs it. Many actors need Docker (`container`, the `pg_client`/`mqtt_client` examples that pair with containers). Don't run tests that need infra you can't see is up; ask first.
3. **Don't generalize.** A `.ill` file is a single test. Don't introduce helpers, parameterization, or fixtures. If two tests share setup, that's two `.ill` files with similar prologues — that's fine.

## File anatomy

Every `.ill` file has the same shape:

```
# 1. actor declarations (top of file, all up-front)
actor <name> = <kind>,
  <kwargs>
  vars:
    <annotations>
    <var>: <default>

# 2. as-blocks that drive each actor
as <name>:
  <command> <positional>,
    <kwarg>: <value>
  assert <expression>
  let <binding> = <expression>
```

Rules to internalize:

- **Comments** start with `#`. Use them to explain *why*, the same way you would in Rust or Python.
- **`@expect "..."`** annotates the next statement with a human-readable expectation surfaced on failure. Use it on the statements that matter — typically `query`, `assert`, `run`, `check`. Don't sprinkle it on every line.
- **Indentation matters** for kwargs and `vars:` blocks. Two spaces, consistent.
- **`,` after a command name** introduces kwargs. `run` alone is fine; `run, env: ...` is the kwarg form.
- **`as <actor>:` blocks can repeat.** Returning to `as alice:` later is fine and common (e.g. setup, do work, teardown).
- **Cross-actor references** use `<actor>.<var>` (e.g. `db.port`). The variable must be declared with `@access read` on the source actor.

## Actors

| Kind          | When to use                                                 |
| ------------- | ----------------------------------------------------------- |
| `exec`        | Long-running host process: a server, daemon, broker.        |
| `container`   | Docker container by `image:` or `dockerfile:`.              |
| `pg_client`   | Postgres client — needs a running postgres (often a container). |
| `http_client` | REST client — stateless, no `connect`.                      |
| `mqtt_client` | MQTT 5 client — needs a broker (often a container).         |
| `args_actor`  | Built-in: read CLI args passed to `ill test`.               |

`exec` is for things that *stay running*. Use `container` for Docker; don't shell out to `docker run` via `exec`. For one-shot host commands (build, migration), prefer the actor that owns the system you're talking to — `exec` keeps the process alive and side effects are not contained.

### Actor member variables

```
actor db = container,
  image: "postgres:18"
  internal_port: 5432
  vars:
    @access read         # readable by other actors as `db.port`
    port: 5432           # default value (also makes it optional)
```

Annotations:

- `@access read` — other actors can read it (`<actor>.<var>`). Without this, the variable is private to the declaring actor.
- `@mut once` — the variable can be assigned exactly once (typical for IDs captured during the test).
- A `vars:` entry without a value is **required** (notably for `args_actor`). Entries with a value are **optional** with that default.

Inside an `as` block, the actor's own vars are reached as `self.<var>`.

## Commands and modes

Commands are actor-specific and mode-gated. The validator knows the mode graph: e.g. `pg_client` starts disconnected and `query` is invalid until `connect` runs. If you get a "command not valid in this mode" diagnostic, you usually skipped a setup step.

Per-actor command quick reference:

- `exec`: `run` (start), implicit stop on test end.
- `container`: `run` (start; takes `external_port:` and `env:`), `stop`.
- `pg_client`: `connect, user:, password:, port:, database:, application_name:` then `query <sql>`. Optional `timeout:` kwarg on `query`.
- `http_client`: `get`, `post`, `put`, `delete` (no `connect`). Kwargs: `headers:`, `body:`, `timeout:`.
- `mqtt_client`: `connect, host:, port:`, `subscribe_<qos>`, `publish_<qos>`, `receive publish` (kwarg `timeout:` in seconds), `disconnect, reason_code:, user_properties:`.
- `args_actor`: `check` (validates required vars are present).

## Squiggles

Squiggles attach a meaning to a literal: ``~sql`SELECT 1` `` is a string that the validator parses as SQL. They give you syntax highlighting, validation at interpretation time, and (for some) parsing.

Common squiggles:

- ``~sql`...` `` — SQL statement (validated as SQL).
- ``~json`...` `` — JSON document.
- ``~re`...` `` — regex pattern (used with `matches` / `!matches`).
- ``~hex`DEADBEEF` `` — base-16 byte string.
- ``~b`hello` `` — raw bytes.

Backtick content is raw — `\.` is a literal backslash-dot, which is what you want for regex. Use `${expr}` for interpolation: ``~sql`SELECT * FROM users WHERE id = ${alice_id}` ``. Squiggles can span multiple lines; indent the inner content for readability.

## Assertions

```
assert <lhs> <op> <rhs>
```

Operators: `==`, `!=`, `>`, `>=`, `<`, `<=`, `contains`, `!contains`, `matches`, `!matches`. `matches` takes a `~re` regex on the right. `contains` works on strings (substring) and arrays (membership).

The result of the previous command is in `ok` (success) or `error` (failure). They're mutually exclusive. The shape depends on the command:

- `pg_client` query: `ok.row[i]`, `ok.row[i][j]`, `ok.col["name"]`, `ok.col[i]`, `ok.row_count`, `ok.col_count`. Errors: `error.query.reason == :syntax_error` (and similar atoms).
- `http_client`: `ok.status`, `ok.body`, `ok.headers["X-..."]`. Errors: `error.http.status`, `error.http.body`, `error.network.reason == :timeout`.
- `mqtt_client` receive: `ok.payload`, `ok.topic`. Errors: `error.mqtt.reason == :invalid_topic`.
- `exec` failure: `error.type == :exec`, `error.exec.reason == :command_not_found`.

Atoms (`:syntax_error`, `:timeout`) are leading-colon symbols — compare with `==`.

## Capturing values

```
let alice_id = ok.row[0][0]
```

Bindings are scoped to the file. Use them for IDs, tokens, trace headers — anything written by one statement and read by a later one. To persist across actors, use a member variable with `@mut once @access read` and assign with `self.<var> = ...` — see `examples/readme.ill` for the canonical pattern.

## Parsing responses

Bytes and strings come back raw — `ok.body` from `http_client` is a string, not a parsed object. Use `parse <expr> as <format>` to deserialize:

```
post "${api.base}/users",
  body: ~json`{"name": "bob"}`
let created = parse ok.body as json
assert created["name"] == "bob"
let new_id = created["id"]
```

`parse ... as json` returns a value you can index with `["key"]` (objects) or `[i]` (arrays); chain bracket access for nested fields (`user["address"]["city"]`). Use it on response bodies (`ok.body`, `error.http.body`) any time you need to assert on structure rather than the whole string.

## Errors and negative tests

Negative paths are first-class. Drive a command you expect to fail, then assert on `error`:

```
query "SELEC invalid"
assert error.query.reason == :syntax_error
```

When you *don't* want squiggle validation to reject your bad input, use a plain string literal (`"SELEC invalid"`), not `~sql`. The squiggle would refuse to parse it.

## Common diagnostics and fixes

- **"undeclared actor" / "unknown variable on actor"** — the actor wasn't declared at the top, or the var lacks `@access read`.
- **"command X not valid in mode Y"** — you're calling `query` before `connect`, or `publish` before `connect` on mqtt. Add the missing setup.
- **"required member variable not set"** — `args_actor` var declared without a default; pass it on the command line, or give it a default.
- **squiggle parse errors** — the embedded SQL/JSON/regex itself is invalid. Either fix it, or drop the squiggle if you intentionally want bad input.

## Reference examples

`examples/` (symlinked from the iLL repo's `examples/` so they're always current) — read these when you need a concrete shape:

- `examples/readme.ill` — full multi-actor bring-up: postgres container + django api + http signup + pg verification.
- `examples/pg_client/basic.ill` — postgres client core flow: connect, query, capture, assert.
- `examples/pg_client/assertions.ill` — every assertion operator (`==`, `contains`, `matches`, etc.) and multi-line `~sql`.
- `examples/pg_client/connection_failures.ill` — negative connect cases and error shapes.
- `examples/pg_client/query.ill` — query patterns beyond the basic case.
- `examples/pg_client/row_level_security.ill` — multiple clients, role-based access patterns.
- `examples/exec/basic.ill` — long-running host process.
- `examples/exec/with_cwd.ill` — pointing exec at a sibling project via `cwd:`.
- `examples/exec/failing.ill` — `:command_not_found` and other exec error shapes.
- `examples/container/basic.ill` — `image:` form, env, port mapping.
- `examples/container/dockerfile.ill` — building from a local `Dockerfile`.
- `examples/container/multi_container.ill` — two containers sharing config via `@access read` vars.
- `examples/rest/basic.ill` — http verbs, json parse, status assertions, transport-level errors.
- `examples/rest/headers.ill` — auth flows, header round-tripping, response header assertions.
- `examples/rest/multi_actor.ill` — multiple http clients against one api.
- `examples/rest/connection_failures.ill` — http error shapes (`error.http.*`, `error.network.*`).
- `examples/mqtt/basic.ill` — broker container + client, pub/sub, binary payloads via `~b` / `~hex`.
- `examples/mqtt/qos.ill` — QoS 0/1/2 publish and subscribe.
- `examples/mqtt/echo.ill`, `user_properties.ill`, `session_takeover.ill`, `connection_failures.ill` — narrower mqtt scenarios.
- `examples/built-in/args.ill` — `args_actor` for parameterizing tests from the CLI.

If you only have time to read one, read `examples/readme.ill` — it touches every actor kind in a realistic flow.

## Slash command

`/ill-test [path]` runs `ill check` then `ill test` on the given path (or the current directory). Use it after edits, or suggest it to the user when they're done writing.
