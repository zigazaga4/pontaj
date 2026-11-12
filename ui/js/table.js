// Building blocks for the editable tables: a bound text input, keyboard
// navigation and the cell syntax used by the hours cells.
import { h } from "./dom.js";

export const KNOWN_CODES = ["CO", "CM", "CP", "BP", "AM", "CIM", "CC", "CFP", "OLP", "N", "ZLP"];

/** Turns what the user typed in an hours cell into an edit: { overrideHours, code }. */
export function parseCellInput(text) {
  const value = text.trim().toUpperCase();
  if (value === "") return { overrideHours: null, code: null };
  if (/^\d{1,2}$/.test(value)) return { overrideHours: Number(value), code: null };
  return { overrideHours: null, code: value };
}

/**
 * A text input showing a value read from the current sheet.
 *  - `get()` returns the value, `format(value)` the text shown,
 *  - `parse(text)` returns the new value (`undefined` = invalid input),
 *  - `commit(value)` is called when a valid change was typed,
 *  - `classes(value)` optionally returns extra CSS classes.
 * `update()` re-reads the sheet, but never overwrites a field being typed in.
 */
export function boundInput({ get, format = String, parse = (t) => t, commit, classes, className = "", title, ...attrs }) {
  const el = h("input", { class: `cellin ${className}`, spellcheck: "false", autocomplete: "off", ...attrs });
  let last = ""; // text already committed (Enter fires `change` and so does leaving the field)
  const update = () => {
    const value = get();
    if (document.activeElement !== el) el.value = last = format(value);
    const invalid = el.classList.contains("invalid") ? "invalid" : "";
    el.className = `cellin ${className} ${classes?.(value) ?? ""} ${invalid}`.replace(/\s+/g, " ").trim();
    el.title = typeof title === "function" ? title(value) ?? "" : title ?? "";
  };
  el.addEventListener("focus", () => el.select());
  el.addEventListener("change", () => {
    if (el.value === last) return;
    const value = parse(el.value);
    if (value === undefined) {
      el.classList.add("invalid");
      return;
    }
    el.classList.remove("invalid");
    last = el.value;
    commit(value);
  });
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
