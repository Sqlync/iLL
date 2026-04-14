/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// iLL — integration Logic Language
// tree-sitter grammar
//
// iLL is indentation-sensitive. An external scanner emits NEWLINE, INDENT,
// and DEDENT tokens so the grammar can express block structure.

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
      seq($.INDENT, $._actor_property_list, $.DEDENT),

    _actor_property_list: ($) => newline_list($, $.actor_property),

    actor_property: ($) =>
      choice(
        seq(field("key", $.identifier), ":", field("value", $._expression)),
        $.vars_block,
      ),

    vars_block: ($) =>
      seq("vars", ":", $.INDENT, $._var_list, $.DEDENT),

    _var_list: ($) => newline_list($, $.var_declaration),

    var_declaration: ($) =>
      seq(
        repeat($.annotation),
        field("name", $.identifier),
        optional(seq(":", field("default", $._expression))),
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
      seq($.INDENT, $._statement_list, $.DEDENT),

    _statement_list: ($) => newline_list($, $._statement),

    // ─── Statements ────────────────────────────────────────────────────
    _statement: ($) =>
      choice($.command, $.assert_statement, $.let_statement),

    // ─── Commands ──────────────────────────────────────────────────────
    // Any identifier in statement position is a command.
    // tree-sitter's `word` property ensures literal keywords (assert, let,
    // parse, as, actor, vars) take priority over $.identifier here.
    command: ($) =>
      seq(field("name", $.identifier), optional($._command_tail)),

    // Command argument patterns:
    //   cmd                                   → no tail
    //   cmd,\n  port: 8080                    → comma + keyword block
    //   cmd ~<name>`...`                      → positional only
    //   cmd "t", "p"                          → multiple positional
    //   cmd x, timeout: 2                     → positional + inline keyword
    //   cmd ~<name>`...`,\n  timeout: 5000    → positional + keyword block
    //   cmd,\n  host: "localhost"             → comma + keyword block
    _command_tail: ($) =>
      choice(
        // comma + indented keyword block (no positional args)
        seq(",", $.keyword_block),
        // positional args only
        $._positional_arg_list,
        // positional args + comma + keyword section
        seq($._positional_arg_list, ",", choice($.keyword_block, $.inline_keyword_args)),
      ),

    _positional_arg_list: ($) =>
      prec.left(seq($._expression, repeat(seq(",", $._expression)))),

    // Indented keyword block
    keyword_block: ($) =>
      seq($.INDENT, $._keyword_list, $.DEDENT),

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
          seq($.INDENT, $._keyword_pair_list, $.DEDENT),
          // simple value: port: 8080
          $._expression,
        )),
      ),

    _keyword_pair_list: ($) => newline_list($, $.keyword_pair),

    keyword_pair: ($) =>
      seq(field("key", $._expression), ":", field("value", $._expression)),

    // ─── Assert ────────────────────────────────────────────────────────
    assert_statement: ($) =>
      seq(
        optional($.annotation),
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
        $.hex_number,
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

    string_content: (_) => token.immediate(prec(1, /[^"\\$]+|\\./)),

    single_quoted_string: (_) => token(seq("'", /[^']*/, "'")),

    interpolation: ($) => seq(token.immediate("${"), $._expression, "}"),

    // ─── Sigils ────────────────────────────────────────────────────────
    sigil: ($) =>
      seq(
        "~",
        field("name", $.sigil_name),
        "`",
        repeat(choice($.interpolation, $.sigil_content)),
        "`",
      ),

    sigil_name: (_) => choice("sql", "json"),

    sigil_content: (_) => token.immediate(prec(1, /[^`\\$]+|\\./)),

    // ─── Primitives ────────────────────────────────────────────────────
    number: (_) => /\d+/,

    hex_number: (_) => /0x[0-9a-fA-F]+/,

    boolean: (_) => choice("true", "false"),

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_-]*/,
  },
});
