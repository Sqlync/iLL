; iLL — syntax highlights
; Follows the standard tree-sitter capture name convention.

; ─── Comments ────────────────────────────────────────────────────────────────

(comment) @comment

; ─── Keywords ────────────────────────────────────────────────────────────────

"actor"  @keyword
"as"     @keyword
"vars"   @keyword
"let"    @keyword
"assert" @keyword
"parse"  @keyword

(parse_expression "as" @keyword)

; ─── Actor types ─────────────────────────────────────────────────────────────

(actor_type) @type.builtin

; ─── Command names ───────────────────────────────────────────────────────────

(command name: (identifier) @function.builtin)

; ─── Annotations ─────────────────────────────────────────────────────────────

(annotation "@" @punctuation.special)
(annotation_name) @attribute
(annotation_value) @attribute

; ─── Named identifiers — actor declarations and as-block references ───────────

(actor_declaration name: (identifier) @type)
(as_block actor: (identifier) @type)

; ─── Variable bindings ───────────────────────────────────────────────────────

(let_statement name: (identifier) @variable)

; ─── Built-in result identifiers: ok, error, self ───────────────────────────

((identifier) @variable.builtin
  (#any-of? @variable.builtin "ok" "error" "self"))

; ─── Member access properties ────────────────────────────────────────────────

(member_expression property: (identifier) @property)

; ─── Keyword argument keys ───────────────────────────────────────────────────
; keyword_arg.key is always a bare identifier (e.g. port:, host:, timeout:)

(keyword_arg key: (identifier) @property)

; actor property keys (image:, file:, port:)
(actor_property key: (identifier) @property)

; var declaration names inside vars: block
(var_declaration name: (identifier) @property)

; keyword_pair keys in nested dict blocks (env:, headers:, user_properties:)
; The key is _expression, so the immediate child is primary_expression.
; Identifier keys (e.g. POSTGRES_PASSWORD, APP_ENV):
(keyword_pair key: (primary_expression (identifier) @variable))
; String keys (e.g. "Content-Type", "app") fall through to @string below.

; ─── Parse expression format identifier ──────────────────────────────────────

(parse_expression format: (identifier) @type)

; ─── Operators ───────────────────────────────────────────────────────────────

(comparison_operator) @operator
(let_statement "=" @operator)
(actor_declaration "=" @operator)

; ─── Strings ─────────────────────────────────────────────────────────────────

(double_quoted_string) @string
(single_quoted_string) @string
(string_content) @string

; ─── String interpolation ────────────────────────────────────────────────────

(interpolation
  "${" @punctuation.special
  "}" @punctuation.special)

; ─── Sigils (~sql`...`, ~json`...`, ~hex`...`) ──────────────────────────────────────────

(sigil "~" @string.special)
(sigil_name) @string.special
(sigil "`" @string.special)
(sigil_content) @string

; ─── Atoms (:syntax_error, :timeout, etc.) ───────────────────────────────────

(atom ":" @punctuation.special)
(atom (identifier) @constant)

; ─── Numbers ─────────────────────────────────────────────────────────────────

(number) @number

; ─── Booleans ────────────────────────────────────────────────────────────────

(boolean) @constant.builtin

; ─── Brackets ────────────────────────────────────────────────────────────────

(array "[" @punctuation.bracket "]" @punctuation.bracket)
(index_expression "[" @punctuation.bracket "]" @punctuation.bracket)

; ─── Identifiers (fallback) ──────────────────────────────────────────────────

(identifier) @variable
