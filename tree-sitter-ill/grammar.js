/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// iLL — integration Logic Language
// tree-sitter grammar
//
// iLL is indentation-sensitive. An external scanner emits NEWLINE, INDENT,
// and DEDENT tokens so the grammar can express block structure.

// Between items, at least one NEWLINE is required (the scanner also emits a
// pending NEWLINE after a DEDENT that returns to the enclosing block's indent
// level). Optional leading NEWLINEs are handled per-block after INDENT.
const newline_list = ($, rule) =>
  seq(rule, repeat(seq(repeat1($.NEWLINE), rule)));

module.exports = grammar({
  name: "ill",

  externals: ($) => [$.NEWLINE, $.INDENT, $.DEDENT],

  extras: ($) => [/[ \t]/, $.comment],

  word: ($) => $.identifier,


  rules: {
    source_file: ($) => repeat(choice($._top_level, $.NEWLINE)),

    _top_level: ($) => choice($.actor_declaration, $.as_block),

    // ─── Comments ──────────────────────────────────────────────────────
    comment: (_) => token(seq("#", /.*/)),

    // ─── Actor declarations ────────────────────────────────────────────
    actor_declaration: ($) =>
      seq(
        "actor",
        field("name", $.identifier),
        "=",
        field("type", $.actor_type),
        optional(seq(",", $.actor_body)),
      ),

    actor_type: ($) => $.identifier,

    actor_body: ($) =>
      seq($.INDENT, repeat($.NEWLINE), $._actor_property_list, $.DEDENT),

    _actor_property_list: ($) => newline_list($, $.actor_property),

    actor_property: ($) =>
      choice(
        seq(field("key", $.identifier), ":", field("value", $._expression)),
        $.vars_block,
      ),

    vars_block: ($) =>
      seq("vars", ":", $.INDENT, repeat($.NEWLINE), $._var_list, $.DEDENT),

    _var_list: ($) => newline_list($, $.var_declaration),

    var_declaration: ($) =>
      seq(
        repeat($.annotation),
        field("name", $.identifier),
        optional(seq(":", field("default", $._expression))),
        optional(","),
      ),

    // ─── Annotations ───────────────────────────────────────────────────
    annotation: ($) =>
      prec.left(seq("@", $.annotation_name, optional($.annotation_value))),

    annotation_name: (_) => choice("access", "mut", "expect"),

    annotation_value: ($) => choice($.identifier, $.string),

    // ─── As blocks ─────────────────────────────────────────────────────
    as_block: ($) =>
      seq("as", field("actor", $.identifier), ":", $.block),

    block: ($) =>
      seq($.INDENT, repeat($.NEWLINE), $._statement_list, $.DEDENT),

    _statement_list: ($) => newline_list($, $._statement),

    // ─── Statements ────────────────────────────────────────────────────
    _statement: ($) =>
      choice($.command, $.assert_statement, $.let_statement, $.assignment_statement),

    // ─── Commands ──────────────────────────────────────────────────────
    // Any identifier in statement position is a command.
    // tree-sitter's `word` property ensures literal keywords (assert, let,
    // parse, as, actor, vars) take priority over $.identifier here.
    command: ($) =>
      seq(
        // Annotation may appear on its own line before the command name;
        // repeat(NEWLINE) handles the line break between them.
        optional(seq($.annotation, repeat($.NEWLINE))),
        field("name", $.identifier),
        optional($._command_tail),
      ),

    // Command argument patterns:
    //   cmd                                   → no tail
    //   cmd,\n  port: 8080                    → comma + keyword block
    //   cmd ~<name>`...`                      → positional only
    //   cmd "t", "p"                          → multiple positional
    //   cmd x, timeout: 2                     → positional + inline keyword
    //   cmd ~<name>`...`,\n  timeout: 5000    → positional + keyword block
    //   cmd,\n  host: "localhost"             → comma + keyword block
    //
    // Grammar structure: a tail is either:
    //   a) "," keyword_section              (no positional args at all)
    //   b) expr ("," expr)* ["," keyword_section]
    //                                        (one or more positional args, optional keyword section)
    //
    // Putting all positional args in one flat repetition with a single optional
    // keyword_section suffix eliminates the shift-reduce ambiguity that arose
    // from having two separate _positional_arg_list choices in _command_tail.
    _command_tail: ($) =>
      choice(
        seq(",", $._keyword_section),
        seq(
          $._expression,
          repeat(seq(",", $._expression)),
          optional(seq(",", $._keyword_section)),
        ),
      ),

    _keyword_section: ($) => choice($.keyword_block, $.inline_keyword_args),

    // Indented keyword block
    keyword_block: ($) =>
      seq($.INDENT, repeat($.NEWLINE), $._keyword_list, $.DEDENT),

    _keyword_list: ($) => newline_list($, $.keyword_arg),

    // Inline keyword args (on same line as command)
    inline_keyword_args: ($) =>
      seq($.keyword_arg, repeat(seq(",", $.keyword_arg))),

    keyword_arg: ($) =>
      seq(
        field("key", $.identifier),
        ":",
        field("value", choice(
          // nested block: env:\n  KEY: "val"
          seq($.INDENT, repeat($.NEWLINE), $._keyword_pair_list, $.DEDENT),
          // simple value: port: 8080
          $._expression,
        )),
      ),

    _keyword_pair_list: ($) => newline_list($, $.keyword_pair),

    keyword_pair: ($) =>
      seq(field("key", $._expression), ":", field("value", $._expression)),

    // ─── Assignment ────────────────────────────────────────────────────
    // Sets a member variable: self.user_id = resp["id"]
    assignment_statement: ($) =>
      seq(
        field("target", choice($.member_expression, $.identifier)),
        "=",
        field("value", $._expression),
      ),

    // ─── Assert ────────────────────────────────────────────────────────
    assert_statement: ($) =>
      seq(
        optional(seq($.annotation, repeat($.NEWLINE))),
        "assert",
        field("left", $._expression),
        optional(
          seq(
            field("operator", $.comparison_operator),
            field("right", $._expression),
          ),
        ),
      ),

    comparison_operator: (_) =>
      choice(
        "==", "!=", ">", ">=", "<", "<=",
        "contains", "!contains", "matches", "!matches",
      ),

    // ─── Let ───────────────────────────────────────────────────────────
    let_statement: ($) =>
      seq(
        "let",
        field("name", $.identifier),
        "=",
        field("value", choice($.parse_expression, $._expression)),
      ),

    parse_expression: ($) =>
      seq(
        "parse",
        field("source", $._expression),
        "as",
        field("format", $.identifier),
      ),

    // ─── Expressions ───────────────────────────────────────────────────
    _expression: ($) =>
      choice($.member_expression, $.index_expression, $.primary_expression),

    member_expression: ($) =>
      prec.left(2, seq(
        field("object", choice($.member_expression, $.primary_expression)),
        ".",
        field("property", $.identifier),
      )),

    index_expression: ($) =>
      prec.left(3, seq(
        field("object", choice($.index_expression, $.member_expression, $.primary_expression)),
        "[",
        field("index", $._expression),
        repeat(seq(",", field("index", $._expression))),
        "]",
      )),

    primary_expression: ($) =>
      choice(
        $.identifier,
        $.string,
        $.sigil,
        $.number,
        $.boolean,
        $.atom,
        $.array,
      ),

    // :syntax_error, :timeout, etc.
    atom: ($) => seq(":", $.identifier),

    // [1, "alice"]
    array: ($) =>
      seq("[", $._expression, repeat(seq(",", $._expression)), "]"),

    // ─── Strings ───────────────────────────────────────────────────────
    string: ($) => choice($.double_quoted_string, $.single_quoted_string),

    double_quoted_string: ($) =>
      seq('"', repeat(choice($.interpolation, $.string_content)), '"'),

    // Allow lone $ (not followed by {) so patterns like "^foo$" work.
    // interpolation uses prec(2) to win over string_content when ${ appears.
    string_content: (_) => token.immediate(prec(1, /[^"\\$]+|\\.|[$]/)),

    single_quoted_string: (_) => token(seq("'", /[^']*/, "'")),

    interpolation: ($) => seq(token.immediate(prec(2, "${")), $._expression, "}"),

    // ─── Sigils ────────────────────────────────────────────────────────
    sigil: ($) =>
      seq(
        "~",
        field("name", $.sigil_name),
        "`",
        repeat(choice($.interpolation, $.sigil_content)),
        "`",
      ),

    sigil_name: (_) => choice("sql", "json", "hex", "re"),

    // Same lone-$ fix as string_content.
    sigil_content: (_) => token.immediate(prec(1, /[^`\\$]+|\\.|[$]/)),

    // ─── Primitives ────────────────────────────────────────────────────
    number: (_) => /\d+/,

    boolean: (_) => choice("true", "false"),

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_-]*/,
  },
});
