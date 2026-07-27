// The inferred-response-type region that sits *below* the query editor.
//
// It replaces the end-of-line chips. A chip could only ever be as wide as the
// line it hung off, so the types that matter most — the graph-edge row is 83
// characters — were exactly the ones it ellipsised. A region under the editor
// has the full column width, as many lines as it needs, and no reason to
// truncate anything.
//
// Attribution is the one thing chips got for free and this has to earn. Three
// signals do it, cheapest first:
//
//   * every statement gets a row, numbered by its position in the query, and
//     the editor draws the same number as a small badge at the end of the
//     statement (see `setMarkers` in editor.mjs) — so the mapping is visible
//     without reading anything;
//   * each row echoes the leading text of its statement, so `SELECT name,
//     email FROM user…` identifies itself even among three SELECTs;
//   * the caret and the pointer link the two directions live — the row for the
//     statement under the caret is marked active, hovering a row lights its
//     badge, and clicking a row selects the statement in the textarea.
//
// Statements that return nothing (DEFINE, LET, BEGIN) get a row too, dimmed.
// Skipping them would make the numbering lie about which statement is which,
// and "this statement has no response value" is worth saying once.

import { byteMapper } from "./editor.mjs";
import { formatKind } from "./typefmt.mjs";

const escapeHtml = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** `define_table` -> `DEFINE TABLE`; the way it is written in the query. */
const kindLabel = (kind) =>
  typeof kind === "string" && kind && kind !== "unknown" ? kind.replace(/_/g, " ").toUpperCase() : "";

/** One line of the statement's own text, whitespace collapsed. */
function excerpt(text, start, end) {
  return text.slice(start, end).replace(/\s+/g, " ").trim();
}

/**
 * Splits the excerpt into the leading statement keyword and the rest, so the
 * keyword can carry the weight and the rest can recede.
 *
 * The analyzer's `kind` is derived from that keyword, so printing it as a
 * separate tag *and* quoting the statement gives you `SELECT` twice on the
 * same line. Emphasising it in place says the same thing once.
 */
function splitKeyword(text, label) {
  if (label && text.slice(0, label.length).toUpperCase() === label) {
    return [text.slice(0, label.length), text.slice(label.length)];
  }
  return ["", text];
}

/**
 * Builds the region and returns a handle for it.
 *
 * @param {HTMLElement} host an empty element under the query editor
 * @param {{onPick?: (statement: object) => void,
 *          onHover?: (index: number|null) => void,
 *          highlight?: (text: string) => string}} [options]
 */
export function createTypePanel(host, options = {}) {
  const highlight = options.highlight || escapeHtml;

  host.classList.add("sg-types");
  host.innerHTML =
    '<div class="sg-types-head"><span class="sg-types-title">Inferred response type</span>' +
    '<span class="sg-types-count"></span></div><ol class="sg-types-list"></ol>';
  const head = host.querySelector(".sg-types-title");
  const count = host.querySelector(".sg-types-count");
  const list = host.querySelector(".sg-types-list");

  // Ten monospace zeros, measured live: the layout width has to be in
  // characters, and the font size is whatever the page's CSS says it is.
  const probe = document.createElement("span");
  probe.className = "sg-types-probe";
  probe.textContent = "0000000000";
  list.appendChild(probe);

  let statements = [];
  let source = "";
  let lastWidth = 0;

  // The horizontal padding a row puts around its type block. Kept in step with
  // `.sg-type-row` in types-panel.css; two characters of slack either way is
  // the whole consequence of it drifting.
  const ROW_INSET = 20;

  /** The container's capacity in characters, or a sane default when hidden. */
  function measure() {
    const charWidth = probe.getBoundingClientRect().width / 10;
    const avail = list.clientWidth - ROW_INSET;
    if (!charWidth || avail <= 0) return 72;
    return Math.max(24, Math.floor(avail / charWidth));
  }

  function draw() {
    const width = measure();
    lastWidth = width;
    const rows = statements.map((s, i) => {
      const label = kindLabel(s.kind);
      const text = excerpt(source, s.charStart, s.charEnd);
      const [keyword, rest] = splitKeyword(text, label);
      const typed = typeof s.response === "string" && s.response.length;
      const body = typed
        ? `<pre class="sg-type"><code>${formatKind(s.response, width)}</code></pre>`
        : '<div class="sg-type sg-type-void">no response value</div>';
      return (
        `<li class="sg-type-row${typed ? "" : " is-void"}" data-i="${i}" data-kind="${escapeHtml(
          label || "?"
        )}" role="button" tabindex="0" title="Select this statement in the editor">` +
        `<div class="sg-type-meta">` +
        (statements.length > 1 ? `<span class="sg-type-idx">${i + 1}</span>` : "") +
        // A kind the excerpt does not already open with (`unknown`, or anything
        // the engine names differently) still gets said, as its own tag.
        (label && !keyword ? `<span class="sg-type-kind">${escapeHtml(label)}</span>` : "") +
        `<span class="sg-type-src" title="${escapeHtml(text)}">` +
        (keyword ? `<b class="sg-type-kw">${escapeHtml(keyword)}</b>` : "") +
        highlight(rest) +
        `</span></div>${body}</li>`
      );
    });

    if (!rows.length) {
      list.innerHTML = '<li class="sg-types-empty">Type a query to see its inferred response type.</li>';
    } else if (statements.every((s) => s.kind === "unknown" && !s.response)) {
      list.innerHTML =
        '<li class="sg-types-empty">The query does not parse yet — nothing to infer.</li>';
    } else {
      list.innerHTML = rows.join("");
    }
    list.appendChild(probe);

    const plural = statements.length === 1 ? "type" : "types";
    head.textContent = `Inferred response ${plural}`;
    count.textContent = statements.length
      ? `${statements.length} statement${statements.length === 1 ? "" : "s"}`
      : "";

    for (const row of list.querySelectorAll(".sg-type-row")) {
      const i = Number(row.dataset.i);
      const pick = () => options.onPick && options.onPick(statements[i]);
      row.addEventListener("click", pick);
      row.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          pick();
        }
      });
      row.addEventListener("mouseenter", () => options.onHover && options.onHover(i));
      row.addEventListener("mouseleave", () => options.onHover && options.onHover(null));
    }
  }

  // Re-set the types when the column changes width: the break decisions are
  // measured in characters, so a narrower column must be re-laid-out rather
  // than left to overflow.
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      if (statements.length && measure() !== lastWidth) draw();
    }).observe(list);
  }

  return {
    /**
     * @param {object[]} next the analyzer's `statements`
     * @param {string} query  the query pane's text, for the excerpts
     */
    update(next, query) {
      const toChar = byteMapper(query);
      source = query;
      statements = (Array.isArray(next) ? next : []).map((s) => ({
        ...s,
        charStart: toChar(s.start),
        charEnd: toChar(s.end),
      }));
      draw();
    },
    /** The statements, with char offsets — the editor draws badges from these. */
    statements() {
      return statements;
    },
    /**
     * Marks the row containing a caret at `charOffset`. This is how the type
     * reaches a touch device: there is no hover on a phone, but there is
     * always a caret.
     */
    setCaret(charOffset) {
      let active = -1;
      // Each statement owns everything from its start up to the next one's, so
      // a caret parked after a `;` — where typing and setting `.value` leave it
      // — still resolves to the statement it just finished.
      statements.forEach((s, i) => {
        if (charOffset >= s.charStart) active = i;
      });
      this.setActive(active);
    },
    /** Marks row `i` active, or none when `i` is out of range. */
    setActive(i) {
      list.querySelectorAll(".sg-type-row").forEach((row) => {
        row.classList.toggle("is-active", Number(row.dataset.i) === i);
      });
    },
    /** Lights row `i` in response to a pointer elsewhere (the editor badge). */
    setHot(i) {
      list.querySelectorAll(".sg-type-row").forEach((row) => {
        row.classList.toggle("is-hot", Number(row.dataset.i) === i);
      });
    },
  };
}
