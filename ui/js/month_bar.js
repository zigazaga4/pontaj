// The "Luna" and "Anul" pickers of a month bar, shared by every tab that works on one month (Categoria B, Birou, Garaj, Oraș).
import { h } from "./dom.js";

export const MONTHS = ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];

/**
 * The two pickers for `{ year, month }`; `open(year, month)` is called when either changes.
 * The years offered are the last, this and the next one (and the one shown).
 */
export function monthFields({ year, month }, open, today = new Date()) {
  const years = [...new Set([today.getFullYear() - 1, today.getFullYear(), today.getFullYear() + 1, year])].sort();
  const monthSelect = h("select", { onchange: (e) => open(year, Number(e.target.value)) },
    MONTHS.map((name, i) => h("option", { value: i + 1, selected: i + 1 === month }, name)));
  const yearSelect = h("select", { onchange: (e) => open(Number(e.target.value), month) },
    years.map((y) => h("option", { value: y, selected: y === year }, y)));
  return [h("label", { class: "bar-field" }, "Luna", monthSelect), h("label", { class: "bar-field" }, "Anul", yearSelect)];
}
