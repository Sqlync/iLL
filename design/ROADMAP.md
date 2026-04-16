# iLL Project Roadmap

## Phase 1: Language Design via Examples
Validate and iterate on the language through concrete examples before building anything.

- [x] README examples (SQL overview)
- [x] SQL examples
- [x] MQTT examples
- [x] REST examples
- [x] Container examples
- [x] revisit MQTT examples

## Phase 2: Tree-sitter Grammar
Write `grammar.js`. Get it parsing all the examples. Test with `tree-sitter parse examples/**/*.ill` and verify clean CSTs. No Rust code yet.

- [x] tree-sitter grammar.js
- [x] All examples parse cleanly

## Phase 3: AST
Define Rust data structures for the language and lower the tree-sitter CST into them. Commands are generic (name + args) — actor-specific knowledge lives in the validation pass, not the AST.

- [x] Core AST types (actors, statements, expressions)
- [x] Tree-sitter → AST lowering pass
- [x] All examples lower to valid ASTs

## Phase 4: Validation Pass
Mode checking, name resolution (does this actor exist?), type checking (is this expression valid here?). Actor-specific command and mode definitions live here, making actor types pluggable.

- [x] Name resolution (actor declarations → as block references)
- [x] Per-actor command validation (valid commands, required args, argument types)
- [x] Per-actor mode tracking (e.g. must connect before query)
- [x] Expression type checking
- [x] `ill check` command

## Phase 5: First Actor — Exec Runtime
End-to-end vertical slice with the simplest actor: run a command on the host, capture output, check assertions. Establishes the harness, lifecycle, and assertion machinery that later actors reuse. Build only what exec needs; resist generalizing.

- [ ] Exec runtime (command, args, env, timeout, stdout/stderr capture, exit code)
- [ ] Enough expression/binding/assertion support to run exec examples
- [ ] Test harness reporting (pass/fail, exit codes) proven against exec tests
- [ ] Teardown semantics, including failure paths

## Actors
Additional actor types are tracked here rather than as sequential phases — they can be built in any order once Phase 5 lands, and can happen in parallel with later phases. Container should come next since postgres and MQTT will typically run inside containers. Extract shared pieces from Phase 5 as patterns repeat; refactor when the second actor forces it, not before.

- [ ] Container (image/dockerfile, run, lifecycle, shell)
- [ ] Postgres (start/stop, client, queries)
- [ ] MQTT (broker, client, pub/sub)
- [ ] REST (HTTP client)
- [ ] Built-in actors (assert, env, etc.)

## Phase 6: LSP
Wire tree-sitter + validation into the language server for diagnostics, completions, hover.

- [ ] Diagnostics from validation pass
- [ ] Completions (keywords, actor names, mode-aware commands)
- [ ] Hover information

## Phase 7: Cleanup

- [ ] remove all reference to phases and any other roadmap details

## Deferred
Cross-cutting concerns that apply to multiple actors or require broader design. Picked up when there's concrete need and clearer context.

- [ ] Unexpected actor death during a test — how long-running actors signal and surface failure when they crash mid-test (affects exec, container, postgres, mqtt, any persistent service)