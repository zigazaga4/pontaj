// Editable monthly grid: one hours row + one night row per driver.
import { h } from "./dom.js";
import { clock, duration } from "./time.js";
import { KNOWN_CODES, parseCellInput } from "./table.js";

// Total columns, pinned to the right edge of the grid.
const SUMS = [
  { key: "worked", label: "Lucrate" },
  { key: "night", label: "Noapte" },
  { key: "notWorked", label: "Nelucr." },
  { key: "absences", label: "Absențe" },
  { key: "overtime", label: "Supl." },
];
const SUM_WIDTH = 50;
const sumStyle = (i) => `right:${(SUMS.length - 1 - i) * SUM_WIDTH}px`;

function cellText(entry) {
  return entry.code ?? (entry.hours != null ? String(entry.hours) : "");
}

function cellClass(info, entry) {
  const auto = entry.workedMin > 0;
  return [
    "cell",
    info.weekend || info.holiday ? "off" : "",
    entry.code ? (KNOWN_CODES.includes(entry.code) ? "code" : "code unknown") : "",
    entry.overrideHours != null ? "manual" : "",
    !auto && entry.hours != null ? "manual" : "",
  ].filter(Boolean).join(" ");
}

/**
 * Renders the grid. `onEdit(empIndex, dayIndex, edit)` is called after a cell
 * changes, `onSelect(empIndex, dayIndex)` when a cell gets focus,
 * `onFunction(empIndex, text)` when the job title is edited.
 */
export function renderGrid(container, sheet, { onEdit, onSelect, onFunction }) {
  const days = sheet.days;
  const head = h("tr", {},
    h("th", { class: "sticky-1" }, "Nr"),
    h("th", { class: "sticky-2" }, "Șofer"),
    days.map((d) => h("th", { class: `day ${d.weekend || d.holiday ? "off" : ""} ${d.holiday ? "holiday" : ""}`, title: d.holiday ?? "" },
      h("div", {}, d.day), h("small", {}, d.weekday))),
    SUMS.map((s, i) => h("th", { class: "sum", style: sumStyle(i) }, s.label)),
  );

  const body = h("tbody");
  sheet.employees.forEach((emp, ei) => {
    const hoursRow = h("tr", { class: "hours-row" },
      h("td", { class: "sticky-1 nr", rowspan: 2 }, ei + 1),
      h("td", { class: "sticky-2 name", rowspan: 2 },
        h("div", { class: "emp-name" }, emp.name),
        h("input", { class: "emp-func", value: emp.function, title: "Funcția", onchange: (e) => onFunction(ei, e.target.value) })),
    );
    const nightRow = h("tr", { class: "night-row" });

    days.forEach((info, di) => {
      const entry = emp.days[di];
      const input = h("input", {
        class: cellClass(info, entry),
        value: cellText(entry),
        dataset: { e: ei, d: di },
        maxlength: 4,
        spellcheck: "false",
        onfocus: (e) => { e.target.select(); onSelect(ei, di); },
        onchange: (e) => onEdit(ei, di, parseCellInput(e.target.value)),
        onkeydown: (e) => navigate(e, container, ei, di),
      });
      hoursRow.append(h("td", { class: info.weekend || info.holiday ? "off" : "" }, input));
      nightRow.append(h("td", { class: `night ${info.weekend || info.holiday ? "off" : ""}`, dataset: { ne: ei, nd: di } }, entry.nightHours ?? ""));
    });

    SUMS.forEach(({ key }, i) => {
      hoursRow.append(h("td", { class: `sum ${key}`, rowspan: 2, style: sumStyle(i), dataset: { sum: key, se: ei } }, totalValue(emp, key)));
    });
    body.append(hoursRow, nightRow);
  });

  container.replaceChildren(h("table", { class: "grid" }, h("thead", {}, head), body));
}

function totalValue(emp, key) {
  const t = emp.totals;
  if (key === "absences") {
    // OLP is the computed remainder, not an absence typed in the grid.
    return t.codeHours.reduce((sum, v, i) => (KNOWN_CODES[i] === "OLP" ? sum : sum + v), 0) || "";
  }
  return t[key] || "";
}

/** Refreshes values/classes in place (keeps focus and scroll position). */
export function refreshGrid(container, sheet) {
  container.querySelectorAll("input.cell").forEach((input) => {
    const entry = sheet.employees[input.dataset.e].days[input.dataset.d];
    const info = sheet.days[input.dataset.d];
    if (document.activeElement !== input) input.value = cellText(entry);
    input.className = cellClass(info, entry);
  });
  container.querySelectorAll("td.night").forEach((td) => {
    td.textContent = sheet.employees[td.dataset.ne].days[td.dataset.nd].nightHours ?? "";
  });
  container.querySelectorAll("td.sum[data-sum]").forEach((td) => {
    td.textContent = totalValue(sheet.employees[td.dataset.se], td.dataset.sum);
  });
}

function navigate(e, container, ei, di) {
  const moves = { ArrowUp: [-1, 0], ArrowDown: [1, 0], Enter: [0, 1] };
  if (e.key === "ArrowLeft" && e.target.selectionStart === 0) moves.ArrowLeft = [0, -1];
  if (e.key === "ArrowRight" && e.target.selectionEnd === e.target.value.length) moves.ArrowRight = [0, 1];
  const move = moves[e.key];
  if (!move) return;
  e.preventDefault();
  e.target.dispatchEvent(new Event("change"));
  container.querySelector(`input.cell[data-e="${ei + move[0]}"][data-d="${di + move[1]}"]`)?.focus();
}

/** Explains how a day was computed. */
export function renderDetails(target, sheet, ei, di) {
  const emp = sheet.employees[ei];
  const entry = emp.days[di];
  const info = sheet.days[di];
  const date = `${info.weekday} ${String(info.day).padStart(2, "0")}.${String(sheet.month).padStart(2, "0")}.${sheet.year}`;
  const parts = [h("b", {}, `${emp.name} · ${date}`)];
  if (info.holiday) parts.push(h("span", { class: "pill holiday" }, info.holiday));

  if (entry.workedMin > 0) {
    const blocks = entry.cardIntervals.map((s) => `${clock(s.start)}–${clock(s.end)}`).join(", ");
    parts.push(h("span", {}, `Tahograf: ${duration(entry.workedMin)} lucrat (${blocks})`));
  } else {
    parts.push(h("span", { class: "muted" }, "Fără activitate în PDF"));
  }
  if (entry.code) parts.push(h("span", { class: "pill code" }, `Absență: ${entry.code}`));
  if (entry.hours != null) {
    const ivs = entry.intervals.map((i) => `${clock(i.start)}–${clock(i.end)}`).join(" · ");
    parts.push(h("span", {}, `Pontaj: ${entry.hours} ore${entry.overrideHours != null ? " (manual)" : ""} → ${ivs}`));
    parts.push(h("span", { class: "muted" },
      entry.nightHours ? `Noapte: ${entry.nightHours} ore` : entry.nightMin > 0 ? `Noapte: ${duration(entry.nightMin)} (sub prag, nu se trece)` : "Fără ore de noapte"));
  }
  target.replaceChildren(...parts);
}
