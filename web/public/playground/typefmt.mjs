// Pretty-printing an inferred SurrealQL response kind.
//
// The analyzer hands back one flat string per statement — e.g.
// `array<{ id: record<follows>, in: record<user>, out: record<user>, since: datetime }>`.
// Read as a single line that is 83 characters of nested punctuation; read as
// four indented fields it is a table row you recognise at a glance. This module
// turns the string into a small tree and sets it the way a formatter would.
//
// Two rules decide whether a construct breaks, and they are deliberately
// different in kind:
//
//   1. STRUCTURE. An object literal with more than two fields always expands,
//      one field per line, however much room there is. A one- or two-field
//      object is a pair you take in at a glance; three or more is a list you
//      scan, and a list wants lines. This is the rule the horizontal-space rule
//      cannot express — on a wide screen everything "fits", and fitting is not
//      the same as being readable.
//   2. WIDTH. Everything else stays inline until it would run past the measured
//      character capacity of the container, then breaks. So `array<string>`
//      keeps its own line to itself instead of becoming three, and a long union
//      still breaks when it has to.
//
// Both rules only ever *add* line breaks: the printed text always contains
// exactly the characters of the input (modulo whitespace), which is what makes
// "never truncate" a property of this module rather than a hope about CSS.
// `parseKind` proves that per call — if the round-trip does not match the
// input character-for-character it returns a raw node and the string is printed
// verbatim.

/** Kind names that are part of the type language rather than a user's table. */
const BUILTIN = new Set(
  (
    "any none null bool bytes datetime decimal duration float int number object point " +
    "string uuid record geometry array set option range future regex closure function " +
    "file literal either"
  ).split(" ")
);

const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
const tag = (cls, text) => `<span class="${cls}">${esc(text)}</span>`;
const punc = (t) => tag("kp", t);
const nameHtml = (t) => tag(BUILTIN.has(t) ? "kt" : "kn", t);

// ---------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------

/**
 * Parses a kind string into a tree, or returns `{t:"raw"}` when anything at all
 * is unexpected. Never throws: this runs on whatever the engine printed, and a
 * kind the grammar here has not met yet must still reach the reader intact.
 *
 * @param {string} src
 * @returns {object}
 */
export function parseKind(src) {
  const s = String(src == null ? "" : src);
  const raw = { t: "raw", text: s };
  let i = 0;

  const ws = () => {
    while (i < s.length && /\s/.test(s[i])) i++;
  };

  function skipString(quote) {
    i++;
    while (i < s.length && s[i] !== quote) i += s[i] === "\\" ? 2 : 1;
    if (s[i] !== quote) return false;
    i++;
    return true;
  }

  function parseType() {
    const parts = [];
    for (;;) {
      const atom = parseAtom();
      if (!atom) return null;
      parts.push(atom);
      ws();
      if (s[i] === "|") {
        i++;
        continue;
      }
      break;
    }
    return parts.length === 1 ? parts[0] : { t: "union", parts };
  }

  function parseAtom() {
    ws();
    const c = s[i];
    if (c === undefined) return null;
    if (c === "{") return parseObject();
    if (c === "(") {
      i++;
      const inner = parseType();
      ws();
      if (!inner || s[i] !== ")") return null;
      i++;
      return { t: "paren", inner };
    }
    if (c === "'" || c === '"') {
      const start = i;
      if (!skipString(c)) return null;
      return { t: "lit", text: s.slice(start, i) };
    }
    const start = i;
    while (i < s.length && !"<>{}|,:()'\" \t\r\n".includes(s[i])) i++;
    if (i === start) return null;
    const text = s.slice(start, i);
    ws();
    if (s[i] !== "<") return { t: /^[-+.0-9]/.test(text) ? "lit" : "name", text };

    i++;
    const args = [];
    ws();
    if (s[i] === ">") {
      i++;
      return { t: "gen", name: text, args };
    }
    for (;;) {
      const arg = parseType();
      if (!arg) return null;
      args.push(arg);
      ws();
      if (s[i] === ",") {
        i++;
        continue;
      }
      if (s[i] === ">") {
        i++;
        break;
      }
      return null;
    }
    return { t: "gen", name: text, args };
  }

  function parseObject() {
    i++; // {
    ws();
    const fields = [];
    if (s[i] === "}") {
      i++;
      return { t: "obj", fields };
    }
    for (;;) {
      const key = scanKey();
      if (key === null) return null;
      i++; // :
      const value = parseType();
      if (!value) return null;
      fields.push({ key, value });
      ws();
      if (s[i] === ",") {
        i++;
        continue;
      }
      if (s[i] === "}") {
        i++;
        break;
      }
      return null;
    }
    return { t: "obj", fields };
  }

  /**
   * A field key is everything up to the `:` that closes it. It is scanned as
   * raw text rather than parsed, because the engine names a projected column
   * after the expression that produced it: `SELECT age > name FROM user` infers
   * `array<{ age > name: bool }>`, and `>` there is an operator, not a bracket.
   */
  function scanKey() {
    ws();
    const start = i;
    let depth = 0;
    while (i < s.length) {
      const c = s[i];
      if (c === "'" || c === '"') {
        if (!skipString(c)) return null;
        continue;
      }
      if (c === "<" || c === "(" || c === "{") depth++;
      else if (c === ">" || c === ")" || c === "}") {
        if (depth > 0) depth--;
        else if (c !== ">") return null; // a stray `)` or `}` is malformed
      } else if (c === ":" && depth === 0) {
        const key = s.slice(start, i).trim();
        return key.length ? key : null;
      }
      i++;
    }
    return null;
  }

  const node = parseType();
  ws();
  if (!node || i < s.length) return raw;
  // The safety net: printing the tree back must reproduce the input. Anything
  // this grammar silently dropped would be a *truncated type*, which is the one
  // failure this whole file exists to prevent.
  const bare = (x) => x.replace(/\s+/g, "");
  if (bare(plain(node)) !== bare(s)) return raw;
  return node;
}

// ---------------------------------------------------------------------------
// printing
// ---------------------------------------------------------------------------

/** The node as one line of plain text — used for width and round-trip checks. */
function plain(n) {
  switch (n.t) {
    case "obj":
      return n.fields.length
        ? `{ ${n.fields.map((f) => `${f.key}: ${plain(f.value)}`).join(", ")} }`
        : "{}";
    case "gen":
      return `${n.name}<${n.args.map(plain).join(", ")}>`;
    case "union":
      return n.parts.map(plain).join(" | ");
    case "paren":
      return `(${plain(n.inner)})`;
    default:
      return n.text;
  }
}

/** The node as one line of highlighted HTML. */
function inline(n) {
  switch (n.t) {
    case "obj":
      return n.fields.length
        ? punc("{") +
            " " +
            n.fields
              .map((f) => tag("kf", f.key) + punc(":") + " " + inline(f.value))
              .join(punc(",") + " ") +
            " " +
            punc("}")
        : punc("{}");
    case "gen":
      return (
        nameHtml(n.name) + punc("<") + n.args.map(inline).join(punc(",") + " ") + punc(">")
      );
    case "union":
      return n.parts.map(inline).join(" " + punc("|") + " ");
    case "paren":
      return punc("(") + inline(n.inner) + punc(")");
    case "lit":
      return tag("kl", n.text);
    case "name":
      return nameHtml(n.text);
    default:
      return esc(n.text);
  }
}

/** Rule 1: an object of more than two fields is a list, and a list wants lines. */
const WIDE_OBJECT = 2;

function mustBreak(n) {
  switch (n.t) {
    case "obj":
      return n.fields.length > WIDE_OBJECT || n.fields.some((f) => mustBreak(f.value));
    case "gen":
      return n.args.some(mustBreak);
    case "union":
      return n.parts.some(mustBreak);
    case "paren":
      return mustBreak(n.inner);
    default:
      return false;
  }
}

/**
 * @param {object} n     the node
 * @param {number} col   the column this node starts at (for the fit test)
 * @param {number} indent indentation for its continuation and closing lines
 * @param {number} width  the container's capacity in characters
 */
function layout(n, col, indent, width) {
  if (!mustBreak(n) && col + plain(n).length <= width) return inline(n);
  const pad = " ".repeat(indent);
  const ipad = " ".repeat(indent + 2);

  switch (n.t) {
    case "obj": {
      if (!n.fields.length) return punc("{}");
      const body = n.fields
        .map((f, k) => {
          const value = layout(f.value, indent + 2 + f.key.length + 2, indent + 2, width);
          const comma = k < n.fields.length - 1 ? punc(",") : "";
          return ipad + tag("kf", f.key) + punc(":") + " " + value + comma;
        })
        .join("\n");
      return punc("{") + "\n" + body + "\n" + pad + punc("}");
    }
    case "gen": {
      // `array<{ … }>` hugs: the brackets ride on the inner construct's own
      // opening and closing lines rather than costing two lines of their own.
      const only = n.args.length === 1 ? n.args[0] : null;
      if (only && (only.t === "obj" || only.t === "gen" || only.t === "paren")) {
        return (
          nameHtml(n.name) +
          punc("<") +
          layout(only, col + n.name.length + 1, indent, width) +
          punc(">")
        );
      }
      const body = n.args
        .map(
          (a, k) =>
            ipad + layout(a, indent + 2, indent + 2, width) + (k < n.args.length - 1 ? punc(",") : "")
        )
        .join("\n");
      return nameHtml(n.name) + punc("<") + "\n" + body + "\n" + pad + punc(">");
    }
    case "union": {
      const [first, ...rest] = n.parts;
      return (
        layout(first, col, indent, width) +
        rest
          .map((p) => "\n" + ipad + punc("|") + " " + layout(p, indent + 4, indent + 2, width))
          .join("")
      );
    }
    case "paren":
      return punc("(") + layout(n.inner, col + 1, indent, width) + punc(")");
    default:
      // A single unbreakable token. Longer than the line is fine — the
      // container wraps it; it is never cut.
      return inline(n);
  }
}

/**
 * Formats a kind string as highlighted HTML, with newlines and two-space
 * indentation. Render it in a container with `white-space: pre-wrap`.
 *
 * @param {string} src   the kind, as the analyzer printed it
 * @param {number} width the container's capacity in characters
 * @returns {string} HTML
 */
export function formatKind(src, width = 72) {
  const node = parseKind(src);
  return layout(node, 0, 0, Math.max(24, Math.floor(width) || 0));
}

/**
 * The same layout as plain text. Only the harness needs this — it asserts that
 * formatting never loses a character.
 *
 * @param {string} src
 * @param {number} width
 * @returns {string}
 */
export function formatKindText(src, width = 72) {
  return formatKind(src, width)
    .replace(/<[^>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}
