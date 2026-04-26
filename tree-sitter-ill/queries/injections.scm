; Inject the squiggle's language into its content
; e.g. ~sql`SELECT ...` gets SQL highlighting, ~json`{...}` gets JSON highlighting
; injection.combined merges all squiggle_content fragments into one injection so the
; SQL/JSON parser sees the full content even when interpolations (${...}) split it.
((squiggle
  (squiggle_name) @injection.language
  (squiggle_content) @injection.content)
 (#set! injection.combined))
