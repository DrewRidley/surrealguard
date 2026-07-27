// The SurrealQL syntax highlighter the whole site draws with.
//
// It lived inline in `/index.html` and the standalone playground simply went
// without, which is why the two pages did not look like the same product: the
// landing demo coloured its editors and `/playground/` rendered plain grey
// text. One module, imported by both, so a keyword added here shows up in both
// places. The token classes (`k`, `s`, `t`, `fn`, `n`, `c`, `p`, `r`) are the
// ones `/styles.css` paints — see the shared palette under
// "`.sg-hl-pre .k, .guard-code .k`".
//
// It is deliberately a regex tokenizer and not the real grammar: the real
// parser is the 4.7 MB WASM analyzer, and the point of this is to colour text
// the user is still in the middle of typing, where nothing parses.

// Keywords are matched case-sensitively as UPPERCASE and types as lowercase,
// so lowercase identifiers (table/field names like `user`, `age`) are never
// mis-coloured even when they spell a keyword (USER…).
const SG_KW = new Set(
  (
    "SELECT VALUE OMIT FROM WHERE SPLIT GROUP BY ORDER ASC DESC COLLATE NUMERIC LIMIT START FETCH " +
    "TIMEOUT PARALLEL EXPLAIN WITH INDEX NOINDEX WITHINDEX UPDATE UPSERT CREATE INSERT INTO DELETE " +
    "RELATE CONTENT SET MERGE PATCH REPLACE RETURN BEFORE AFTER DIFF NONE DEFINE REMOVE ALTER TABLE " +
    "FIELD EVENT ANALYZER TOKENIZER FILTERS FUNCTION PARAM USER TOKEN SCOPE ACCESS NAMESPACE DATABASE " +
    "SCHEMAFULL SCHEMALESS PERMISSIONS FLEXIBLE READONLY DEFAULT ASSERT TYPE COMMENT OVERWRITE IF NOT " +
    "EXISTS DROP CHANGEFEED RELATION IN OUT ENFORCED UNIQUE SEARCH LET FOR END THEN ELSE BEGIN COMMIT " +
    "CANCEL TRANSACTION LIVE KILL SHOW CHANGES SINCE AND OR IS CONTAINS CONTAINSNOT CONTAINSALL " +
    "CONTAINSANY CONTAINSNONE INSIDE NOTINSIDE ALLINSIDE ANYINSIDE NONEINSIDE OUTSIDE INTERSECTS AS ON " +
    "TRUE FALSE NULL FULL"
  ).split(/\s+/)
);
const SG_TY = new Set(
  (
    "any bool bytes datetime decimal duration float int number object point string uuid record " +
    "geometry array set option range future regex closure"
  ).split(/\s+/)
);
const SG_RULES = [
  [/\/\*[\s\S]*?\*\//y, "c"],
  [/--[^\n]*/y, "c"],
  [/\/\/[^\n]*/y, "c"],
  [/'(?:[^'\\]|\\.)*'/y, "s"],
  [/"(?:[^"\\]|\\.)*"/y, "s"],
  [/`(?:[^`\\]|\\.)*`/y, "s"],
  [/\$[A-Za-z_]\w*/y, "p"],
  [/[A-Za-z_]\w*:[A-Za-z0-9_]+/y, "r"],
  [/\d+(?:\.\d+)?(?:ns|us|ms|s|m|h|d|w|y)?\b/y, "n"],
  [/[A-Za-z_]\w*(?=\s*\()/y, "fn"],
  [/[A-Za-z_]\w*/y, "word"],
];

/** HTML-escapes the three characters that can break out of text content. */
export function sgEsc(s) {
  return String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
}

/**
 * Renders SurrealQL to HTML with token spans. Escapes everything it emits, so
 * it is safe to hand straight to `innerHTML` — and it must, because the
 * squiggle overlay passes it user input.
 *
 * @param {string} src
 * @returns {string} HTML
 */
export function sgHighlight(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  outer: while (i < n) {
    for (const [re, cls] of SG_RULES) {
      re.lastIndex = i;
      const m = re.exec(src);
      if (m) {
        const text = m[0];
        if (!text.length) break;
        let c = cls;
        if (cls === "word") {
          if (SG_KW.has(text)) c = "k";
          else if (SG_TY.has(text)) c = "t";
          else c = null;
        }
        out += c ? `<span class="${c}">${sgEsc(text)}</span>` : sgEsc(text);
        i += text.length;
        continue outer;
      }
    }
    out += sgEsc(src[i]);
    i++;
  }
  return out;
}

/** A diagnostic message, with its backtick spans rendered as code. */
export function sgMsg(m) {
  return sgEsc(String(m)).replace(/`([^`]+)`/g, "<code>$1</code>");
}
