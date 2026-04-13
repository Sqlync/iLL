; Inject the sigil's language into its content
; e.g. ~sql`SELECT ...` gets SQL highlighting, ~json`{...}` gets JSON highlighting
; injection.combined merges all sigil_content fragments into one injection so the
; SQL/JSON parser sees the full content even when interpolations (${...}) split it.
((sigil
  (sigil_name) @injection.language
  (sigil_content) @injection.content)
 (#set! injection.combined))
