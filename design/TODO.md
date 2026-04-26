# TODO

## Current


## Next

sigils
- consider renaming to "tagged literal"
- update tree-sitter grammar to allow arbitrary sigil types
consider implementing a ValueType instead of using stringlytyped types
split actors into separate crates, this will allow users to easily create their own actors
bug: regex escapting in assertions.ill is not working - string literal escaping issue
bug: container actor's `run` silently overwrites a member var named `port` with the host-mapped port. The runtime is overloading the name `port` to mean two distinct things — the kwarg/result of `run` (an integer the runtime computes) and a user-declared member variable that happens to share the name. Any container actor that declares `port:` for any reason gets it stomped on. Need an explicit binding mechanism (e.g. `self.exposed_port = ok.port` once user-writes are supported, or a declaration-site `@bind ok.port` annotation) instead of name-based magic. See container/runtime.rs ~line 418.