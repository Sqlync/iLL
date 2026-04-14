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

- [ ] Core AST types (actors, statements, expressions)
- [ ] Tree-sitter → AST lowering pass
- [ ] All examples lower to valid ASTs

## Phase 5: Validation Pass
Mode checking, name resolution (does this actor exist?), type checking (is this expression valid here?). Actor-specific command and mode definitions live here, making actor types pluggable.

- [ ] Name resolution (actor declarations → as block references)
- [ ] Per-actor command validation (valid commands, required args, argument types)
- [ ] Per-actor mode tracking (e.g. must connect before query)
- [ ] Expression type checking

## Phase 6: Interpreter / Runtime
Actually execute the validated AST. Start postgres, run queries, check assertions.

- [ ] Postgres runtime (start/stop, client, queries)
- [ ] MQTT runtime (broker, client, pub/sub)
- [ ] REST runtime (HTTP client)
- [ ] Bash runtime (run, start/stop daemons)

## Phase 7: LSP
Wire tree-sitter + validation into the language server for diagnostics, completions, hover.

- [ ] Diagnostics from validation pass
- [ ] Completions (keywords, actor names, mode-aware commands)
- [ ] Hover information