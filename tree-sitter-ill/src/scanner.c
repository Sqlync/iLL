#include "tree_sitter/parser.h"
#include <stdbool.h>
#include <stdint.h>

// Token types — must match the order in grammar.js externals
enum TokenType {
  NEWLINE,
  INDENT,
  DEDENT,
};

#define MAX_INDENT_LEVELS 64
#define TAB_WIDTH 2

typedef struct {
  uint16_t indent_stack[MAX_INDENT_LEVELS];
  uint8_t stack_size;
  uint8_t pending_dedents;
} Scanner;

void *tree_sitter_ill_external_scanner_create(void) {
  Scanner *scanner = calloc(1, sizeof(Scanner));
  scanner->indent_stack[0] = 0;
  scanner->stack_size = 1;
  scanner->pending_dedents = 0;
  return scanner;
}

void tree_sitter_ill_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_ill_external_scanner_serialize(void *payload,
                                                     char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  unsigned size = 0;

  buffer[size++] = (char)scanner->stack_size;
  buffer[size++] = (char)scanner->pending_dedents;

  for (uint8_t i = 0; i < scanner->stack_size; i++) {
    buffer[size++] = (char)(scanner->indent_stack[i] & 0xFF);
    buffer[size++] = (char)((scanner->indent_stack[i] >> 8) & 0xFF);
  }

  return size;
}

void tree_sitter_ill_external_scanner_deserialize(void *payload,
                                                    const char *buffer,
                                                    unsigned length) {
  Scanner *scanner = (Scanner *)payload;

  if (length == 0) {
    scanner->indent_stack[0] = 0;
    scanner->stack_size = 1;
    scanner->pending_dedents = 0;
    return;
  }

  unsigned pos = 0;
  scanner->stack_size = (uint8_t)buffer[pos++];
  if (scanner->stack_size > MAX_INDENT_LEVELS) {
    scanner->stack_size = MAX_INDENT_LEVELS;
  }
  scanner->pending_dedents = (uint8_t)buffer[pos++];

  for (uint8_t i = 0; i < scanner->stack_size && pos + 1 < length; i++) {
    scanner->indent_stack[i] =
        (uint16_t)((uint8_t)buffer[pos]) |
        ((uint16_t)((uint8_t)buffer[pos + 1]) << 8);
    pos += 2;
  }
}

static uint16_t current_indent(Scanner *scanner) {
  return scanner->indent_stack[scanner->stack_size - 1];
}

bool tree_sitter_ill_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;

  // Emit pending DEDENTs first
  if (scanner->pending_dedents > 0 && valid_symbols[DEDENT]) {
    scanner->pending_dedents--;
    lexer->result_symbol = DEDENT;
    return true;
  }

  // Nothing to do if no external tokens are valid
  if (!valid_symbols[NEWLINE] && !valid_symbols[INDENT] &&
      !valid_symbols[DEDENT]) {
    return false;
  }

  // At a newline, handle indentation
  if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
    // Consume newline(s) and skip blank lines
    while (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
      lexer->advance(lexer, true);
      uint16_t indent = 0;
      while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        indent += (lexer->lookahead == '\t') ? TAB_WIDTH : 1;
        lexer->advance(lexer, true);
      }
      if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
        continue;
      }

      // We have the indent of the next meaningful line
      uint16_t cur = current_indent(scanner);

      if (indent > cur && valid_symbols[INDENT]) {
        if (scanner->stack_size < MAX_INDENT_LEVELS) {
          scanner->indent_stack[scanner->stack_size++] = indent;
        }
        lexer->result_symbol = INDENT;
        return true;
      } else if (indent < cur && valid_symbols[DEDENT]) {
        while (scanner->stack_size > 1 &&
               scanner->indent_stack[scanner->stack_size - 1] > indent) {
          scanner->stack_size--;
          scanner->pending_dedents++;
        }
        scanner->pending_dedents--;
        lexer->result_symbol = DEDENT;
        return true;
      } else if (indent == cur && valid_symbols[NEWLINE]) {
        lexer->result_symbol = NEWLINE;
        return true;
      }

      return false;
    }
  }

  // At EOF, emit any remaining DEDENTs
  if (lexer->eof(lexer)) {
    if (valid_symbols[DEDENT] && scanner->stack_size > 1) {
      scanner->stack_size--;
      lexer->result_symbol = DEDENT;
      return true;
    }
    if (valid_symbols[NEWLINE]) {
      lexer->result_symbol = NEWLINE;
      return true;
    }
  }

  return false;
}
