// Building blocks for the editable tables: a bound text input, keyboard
// navigation and the cell syntax used by the hours cells.
import { h } from "./dom.js";
import { parseHours } from "./time.js";

export const KNOWN_CODES = ["CO", "CM", "CP", "BP", "AM", "CIM", "CC", "CFP", "OLP", "N", "ZLP"];

/** Turns what the user typed in an hours cell into an edit: { overrideHours, code }. */
export function parseCellInput(text) {
  const hours = parseHours(text);
  if (hours === null) return { overrideHours: null, code: null };
  if (hours !== undefined) return { overrideHours: hours, code: null };
  return { overrideHours: null, code: text.trim().toUpperCase() };
}

/**
 * The edit for an hours cell of a day: hours (or a code, or empty = automatic) and the times of the day follow them,
 * so the typed times that were there are dropped and the times are made again from the hours.
 */
export const hoursCellEdit = (text) => ({ ...parseCellInput(text), overrideIntervals: null });

/** A value an hours cell can take already, while it is being typed: hours or a known code ("C" waits for "CO"). */
export const isCompleteCell = (text) => {
  const hours = parseHours(text);
  return hours !== null && (hours !== undefined || KNOWN_CODES.includes(String(text).trim().toUpperCase()));
};

/** How long after the last key the sheet follows what is typed. */
const LIVE_DELAY = 350;

/**
 * Calls `apply` a moment after the typing stops, so the sheet follows while the user types and not only when the field
 * is left. An empty field or a half-typed value (`complete` says no) waits for Enter / leaving the field.
 */
export function whileTyping(el, apply, { complete = () => true, delay = LIVE_DELAY } = {}) {
  let timer;
  el.addEventListener("input", () => {
    clearTimeout(timer);
    if (el.value.trim() === "" || !complete(el.value)) return;
    timer = setTimeout(apply, delay);
  });
  el.addEventListener("change", () => clearTimeout(timer));
}

/**
 * A text input showing a value read from the current sheet.
 *  - `get()` returns the value, `format(value)` the text shown,
 *  - `parse(text)` returns the new value (`undefined` = invalid input),
 *  - `commit(value, { final })` is called when a valid change was typed (`final`: the field was left or Enter pressed),
 *  - `classes(value)` optionally returns extra CSS classes,
 *  - `live`: also commit while typing (`complete(text)` tells when what is typed is a whole value).
 * `update()` re-reads the sheet, but never overwrites a field being typed in.
 */
export function boundInput({ get, format = String, parse = (t) => t, commit, classes, className = "", title, live = false, complete, ...attrs }) {
  const el = h("input", { class: `cellin ${className}`, spellcheck: "false", autocomplete: "off", ...attrs });
  let last = ""; // text already committed (Enter fires `change` and so does leaving the field)
  const update = () => {
    const value = get();
    if (document.activeElement !== el) el.value = last = format(value);
    const invalid = el.classList.contains("invalid") ? "invalid" : "";
    el.className = `cellin ${className} ${classes?.(value) ?? ""} ${invalid}`.replace(/\s+/g, " ").trim();
    el.title = typeof title === "function" ? title(value) ?? "" : title ?? "";
  };
  const apply = (final) => {
    if (el.value === last) return;
    const value = parse(el.value);
    if (value === undefined) {
      if (final) el.classList.add("invalid");
      return;
    }
    el.classList.remove("invalid");
    last = el.value;
    commit(value, { final });
  };
  el.addEventListener("focus", () => el.select());
  el.addEventListener("change", () => apply(true));
  if (live) whileTyping(el, () => apply(false), { complete });
  return { el, update };
}

/** Enter / ↑ / ↓ move between rows of the same column (left/right: Tab). */
export function enableKeyNav(table) {
  table.addEventListener("keydown", (e) => {
    const step = { Enter: 1, ArrowDown: 1, ArrowUp: -1 }[e.key];
    if (!step || !(e.target instanceof HTMLInputElement) || e.target.type === "date") return;
    const cell = e.target.closest("td");
    const row = cell?.parentElement;
    if (!row) return;
    e.preventDefault();
    e.target.dispatchEvent(new Event("change"));
    let next = step > 0 ? row.nextElementSibling : row.previousElementSibling;
    while (next && !next.cells[cell.cellIndex]?.querySelector("input")) next = step > 0 ? next.nextElementSibling : next.previousElementSibling;
    next?.cells[cell.cellIndex]?.querySelector("input")?.focus();
  });
}
