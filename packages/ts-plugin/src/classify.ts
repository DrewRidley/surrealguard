/**
 * SurrealQL token kinds in TypeScript's two classification vocabularies.
 *
 * This is the flicker fix, and it is worth being precise about what it can and
 * cannot do.
 *
 * The standalone LSP publishes semantic tokens for the bytes inside a query
 * string. So does TypeScript, for the same bytes — it calls them one string
 * literal. Two servers answering about one range is a race the editor resolves
 * differently on every keystroke, which is what the flicker *is*. A language
 * service plugin removes the race by construction: our classifications come
 * back from TypeScript's own method, so there is nothing to merge.
 *
 * What survives the trip depends on which format the client asked for.
 *
 * - **Original** ({@link ts.ClassificationType}) has keyword, comment, string,
 *   number, operator and regexp. Everything SurrealQL has, TypeScript can say.
 * - **2020** ({@link TokenType2020}) — what VS Code's TypeScript extension asks
 *   for — has twelve *semantic* kinds and no lexical ones at all: no keyword,
 *   no string, no comment, no number, no operator. There is no encoding for
 *   "this word is a keyword", so `SELECT` inside a query keeps whatever colour
 *   the string literal had.
 *
 * The identifier half maps cleanly in both, and that is the half that carries
 * the information: table names, fields, `$params` and `fn::` calls stop being
 * undifferentiated string. Mapping a keyword onto some unrelated 2020 kind to
 * force a colour would be worse than leaving it — it would paint SurrealQL
 * keywords with whatever theme rule the user chose for TypeScript namespaces.
 * So in the 2020 format the lexical kinds are omitted, deliberately.
 */

/**
 * The shared legend index a token carries, matching
 * `surrealql_analyzer_syntax::highlight::TokenKind::index`. Renumbering this
 * miscolours every token at once, so it is pinned by a test on both sides.
 */
export enum TokenKind {
  Keyword = 0,
  Comment = 1,
  String = 2,
  Number = 3,
  Regexp = 4,
  Operator = 5,
  Type = 6,
  Function = 7,
  Variable = 8,
  Parameter = 9,
  Property = 10,
  EnumMember = 11,
}

/** `ts.ClassificationType`, inlined so this module needs no `typescript` import. */
const enum Original {
  comment = 1,
  identifier = 2,
  keyword = 3,
  numericLiteral = 4,
  operator = 5,
  stringLiteral = 6,
  regularExpressionLiteral = 7,
  className = 11,
  typeAliasName = 16,
  parameterName = 17,
}

/** `ts.classifier.v2020.TokenType`, likewise inlined. */
const enum TokenType2020 {
  type = 5,
  parameter = 6,
  variable = 7,
  enumMember = 8,
  property = 9,
  function = 10,
}

/** The bit shift the 2020 encoding puts the token type at. */
const TYPE_OFFSET = 8;

/**
 * The Original-format classification for a token kind. Every kind has one.
 */
export function toOriginal(kind: TokenKind): number {
  switch (kind) {
    case TokenKind.Keyword:
      return Original.keyword;
    case TokenKind.Comment:
      return Original.comment;
    case TokenKind.String:
      return Original.stringLiteral;
    case TokenKind.Number:
      return Original.numericLiteral;
    case TokenKind.Regexp:
      return Original.regularExpressionLiteral;
    case TokenKind.Operator:
      return Original.operator;
    case TokenKind.Type:
      return Original.typeAliasName;
    case TokenKind.Function:
      return Original.identifier;
    case TokenKind.Parameter:
      return Original.parameterName;
    case TokenKind.EnumMember:
      return Original.className;
    case TokenKind.Variable:
    case TokenKind.Property:
      return Original.identifier;
  }
}

/**
 * The 2020-format classification for a token kind, or `undefined` when the
 * format cannot express it — see the module comment. `undefined` means "leave
 * these bytes alone", which is the only non-lying option.
 *
 * The encoding is TypeScript's own: `(type + 1) << 8 | modifiers`, with no
 * modifiers ever set.
 */
export function toTwentyTwenty(kind: TokenKind): number | undefined {
  const type = ((): TokenType2020 | undefined => {
    switch (kind) {
      case TokenKind.Type:
        return TokenType2020.type;
      case TokenKind.Function:
        return TokenType2020.function;
      case TokenKind.Variable:
        return TokenType2020.variable;
      case TokenKind.Parameter:
        return TokenType2020.parameter;
      case TokenKind.Property:
        return TokenType2020.property;
      case TokenKind.EnumMember:
        return TokenType2020.enumMember;
      default:
        return undefined;
    }
  })();
  return type === undefined ? undefined : ((type + 1) << TYPE_OFFSET) | 0;
}

/**
 * A SurrealQL Analyzer finding code (`E1002`) as a TypeScript diagnostic code.
 *
 * TypeScript's own codes live below 100 000 — the highest it has ever shipped
 * is five digits — so the millions are free, and one million plus the finding's
 * number keeps the mapping legible in both directions: `E1002` is `1001002`,
 * `L7014` is `1007014`. The number alone identifies a finding (the letter is
 * derived from the thousand-block), so nothing collides, and a user filtering
 * on a code in their editor sees their own catalog number inside ours.
 */
export function toDiagnosticCode(code: string): number {
  const digits = code.match(/\d+/);
  return DIAGNOSTIC_CODE_BASE + (digits ? Number(digits[0]) : 0);
}

/** Where SurrealQL Analyzer's diagnostic codes start. */
export const DIAGNOSTIC_CODE_BASE = 1_000_000;

/** The `source` every diagnostic the plugin adds carries. */
export const DIAGNOSTIC_SOURCE = "surrealql-analyzer";
