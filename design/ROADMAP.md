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

## Phase 3: AST Types
Define the Rust data structures that represent the language semantically. Pure data, no parsing logic.

- [ ] Core AST types (actors, statements, expressions)
- [ ] Actor command types per actor type
- [ ] Mode definitions per actor type

## Phase 4: CST → AST Lowering
Walk tree-sitter nodes, produce AST. This is where `actor db = postgres` becomes `ActorDecl { name: "db", ... }`.

- [ ] Tree-sitter → AST lowering pass
- [ ] All examples lower to valid ASTs

## Phase 5: Validation Pass
Mode checking, name resolution (does this actor exist?), type checking (is this expression valid here?). Takes raw AST, produces errors or a validated AST.

- [ ] Name resolution (actor declarations → as block references)
- [ ] Mode tracking and validation
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