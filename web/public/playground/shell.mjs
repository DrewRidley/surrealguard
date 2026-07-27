// The app shell: draggable dividers and the examples menu.
//
// This is the part of /playground/ that is a window manager rather than an
// analyzer. It is separate from index.html so the page's own script stays
// about what the page is *for* — schema in, diagnostics and types out — and
// because both things here are the sort of small interaction that is only
// correct once you have handled pointer capture, the keyboard, and the escape
// key, and is then never worth writing twice.
//
// Sizes live in CSS custom properties on a container element, so the layout
// itself stays declarative: the divider writes `--pg-col: 640px`, the grid
// reads it, and a browser without JS (or a failed module) still gets the
// stylesheet's default proportions rather than a collapsed page.

const STORE = "sg-playground-layout";

/** localStorage is unavailable in more configurations than it looks. */
function readStore() {
  try {
    return JSON.parse(localStorage.getItem(STORE) || "{}") || {};
  } catch {
    return {};
  }
}

function writeStore(patch) {
  try {
    localStorage.setItem(STORE, JSON.stringify({ ...readStore(), ...patch }));
  } catch {
    /* private mode, or a full quota — the layout simply stops persisting */
  }
}

/**
 * Makes `handle` drag one edge of `container`, writing the result to a CSS
 * custom property in pixels.
 *
 * @param {HTMLElement} handle    the divider, `role="separator"`
 * @param {{container: HTMLElement, prop: string, axis: "x"|"y",
 *          min?: number, minOther?: number, store?: string}} options
 */
export function mountDivider(handle, options) {
  const { container, prop, axis, min = 90, minOther = 130, store } = options;
  const horizontal = axis === "x";
  handle.setAttribute("role", "separator");
  handle.setAttribute("aria-orientation", horizontal ? "vertical" : "horizontal");
  handle.tabIndex = 0;

  const extent = () => {
    const box = container.getBoundingClientRect();
    return horizontal ? box.width : box.height;
  };
  // Where the divider actually is, measured rather than read back from the
  // custom property: the property starts out as a percentage from the
  // stylesheet, and the first keyboard nudge must move from where the reader
  // can see the divider, not from `parseFloat("46%")`.
  const current = () => {
    const box = container.getBoundingClientRect();
    const bar = handle.getBoundingClientRect();
    return horizontal ? bar.left - box.left : bar.top - box.top;
  };

  function apply(size) {
    const room = extent();
    const clamped = Math.max(min, Math.min(size, room - minOther));
    container.style.setProperty(prop, `${Math.round(clamped)}px`);
    handle.setAttribute("aria-valuenow", String(Math.round((clamped / room) * 100)));
    if (store) writeStore({ [store]: Math.round(clamped) });
  }

  // A stored size is in pixels and the window may since have been resized to
  // something smaller; `apply` clamps, so a layout saved on a desktop cannot
  // arrive on a laptop with the output column pushed off-screen.
  const saved = store ? readStore()[store] : null;
  if (typeof saved === "number") requestAnimationFrame(() => apply(saved));

  handle.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add("is-dragging");
    document.body.classList.add(horizontal ? "pg-resizing-x" : "pg-resizing-y");
    const box = container.getBoundingClientRect();
    const bar = handle.getBoundingClientRect();
    // Where inside the divider the drag started, so it does not jump under the
    // pointer on the first pixel of movement.
    const grab = horizontal ? event.clientX - bar.left : event.clientY - bar.top;
    const move = (e) =>
      apply(horizontal ? e.clientX - box.left - grab : e.clientY - box.top - grab);
    const stop = () => {
      handle.releasePointerCapture(event.pointerId);
      handle.classList.remove("is-dragging");
      document.body.classList.remove("pg-resizing-x", "pg-resizing-y");
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", stop);
      handle.removeEventListener("pointercancel", stop);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", stop);
    handle.addEventListener("pointercancel", stop);
  });

  // A separator you cannot move from the keyboard is a separator half the
  // people who need it most cannot move at all.
  handle.addEventListener("keydown", (event) => {
    const step = event.shiftKey ? 64 : 16;
    const less = horizontal ? "ArrowLeft" : "ArrowUp";
    const more = horizontal ? "ArrowRight" : "ArrowDown";
    if (event.key === less) apply(current() - step);
    else if (event.key === more) apply(current() + step);
    else if (event.key === "Home") apply(min);
    else if (event.key === "End") apply(extent() - minOther);
    else return;
    event.preventDefault();
  });

  return { apply, reset: () => container.style.removeProperty(prop) };
}

/**
 * A dropdown of examples, hung off `button`.
 *
 * The presets used to be a strip of ten buttons above the editor, which is the
 * one place a playground cannot afford to spend: the tool is what the visitor
 * came for, and the examples are how they get started *once*. A menu keeps
 * them one click away and zero pixels tall.
 *
 * @param {HTMLButtonElement} button
 * @param {HTMLElement} list
 * @param {{label: string, note?: string}[]} items
 * @param {(index: number) => void} onPick
 */
export function mountMenu(button, list, items, onPick) {
  list.innerHTML = items
    .map(
      (item, i) =>
        `<button type="button" role="menuitem" data-i="${i}" class="pg-menu-item">` +
        `<span class="pg-menu-label">${escapeHtml(item.label)}</span>` +
        (item.note ? `<span class="pg-menu-note">${escapeHtml(item.note)}</span>` : "") +
        "</button>"
    )
    .join("");

  const entries = () => [...list.querySelectorAll(".pg-menu-item")];
  let open = false;

  function setOpen(next, focusFirst = false) {
    open = next;
    list.hidden = !next;
    button.setAttribute("aria-expanded", String(next));
    button.classList.toggle("is-open", next);
    if (next && focusFirst) entries()[0]?.focus();
  }

  button.addEventListener("click", (event) => {
    event.stopPropagation();
    setOpen(!open);
  });
  button.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setOpen(true, true);
    }
  });

  list.addEventListener("click", (event) => {
    const item = event.target.closest(".pg-menu-item");
    if (!item) return;
    setOpen(false);
    button.focus();
    onPick(Number(item.dataset.i));
  });

  list.addEventListener("keydown", (event) => {
    const all = entries();
    const at = all.indexOf(document.activeElement);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      all[(at + step + all.length) % all.length]?.focus();
    } else if (event.key === "Escape") {
      setOpen(false);
      button.focus();
    }
  });

  document.addEventListener("click", (event) => {
    if (open && !list.contains(event.target) && event.target !== button) setOpen(false);
  });
  document.addEventListener("keydown", (event) => {
    if (open && event.key === "Escape") {
      setOpen(false);
      button.focus();
    }
  });

  return { close: () => setOpen(false) };
}

/**
 * The schema pane's fold. Collapsed, it keeps its header — the schema is the
 * context every diagnostic on the page is relative to, so it may recede but it
 * may not disappear.
 */
export function mountFold(button, pane, column, store = "schemaFolded") {
  const set = (folded) => {
    pane.classList.toggle("is-folded", folded);
    column.classList.toggle("has-folded", folded);
    button.setAttribute("aria-expanded", String(!folded));
    button.title = folded ? "Show the schema" : "Hide the schema";
    writeStore({ [store]: folded });
  };
  set(readStore()[store] === true);
  button.addEventListener("click", () => set(!pane.classList.contains("is-folded")));
  return { isFolded: () => pane.classList.contains("is-folded"), set };
}

const escapeHtml = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
