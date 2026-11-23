// Editable monthly grid: one hours row + one night row per driver.
import { h } from "./dom.js";
import { clock, duration, hoursText, parseHours } from "./time.js";
import { KNOWN_CODES, hoursCellEdit, isCompleteCell, whileTyping } from "./table.js";
import { tirToggle } from "./tir.js";

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
  return entry.code ?? (entry.hours != null ? hoursText(entry.hours) : "");
}

const nightText = (entry) => (entry.nightHours != null ? hoursText(entry.nightHours) : "");

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

const nightClass = (entry) => `cell night-in ${entry.overrideNight != null ? "manual" : ""}`.trim();

/**
 * Sends a cell once for each text: the sheet follows the typing a moment after it stops and again when the
 * cell is left, and the second time the text is the same, so nothing is sent twice.
 */
function sendOnce(input, send) {
  if (input.value === input.dataset.sent) return;
  input.dataset.sent = input.value;
  send(input.value);
}

/**
 * Renders the grid. `onEdit(empIndex, dayIndex, edit)` is called after a cell
 * changes (also while it is typed), `onSelect(empIndex, dayIndex)` when a cell gets focus,
 * `onFunction(empIndex, text)` / `onName(empIndex, text)` when the job title / name is edited
 * (`onName` returns the name now in use).
 * Options: `visible(employee)` leaves some people out (the indexes in the callbacks stay those of the sheet); `heading` is the
 * title of the first column; `onTir(empIndex, on)` adds the "Tir" switch to each name; `onName` and `onFunction` missing
 * make the name and the job read-only (the staff list decides them).
 */
export function renderGrid(container, sheet, { onEdit, onSelect, onFunction, onName, onTir, visible = () => true, heading = "Șofer" }) {
  const days = sheet.days;
  const head = h("tr", {},
    h("th", { class: "sticky-1" }, "Nr"),
    h("th", { class: "sticky-2" }, heading),
    days.map((d) => h("th", { class: `day ${d.weekend || d.holiday ? "off" : ""} ${d.holiday ? "holiday" : ""}`, title: d.holiday ?? "" },
      h("div", {}, d.day), h("small", {}, d.weekday))),
    SUMS.map((s, i) => h("th", { class: "sum", style: sumStyle(i) }, s.label)),
  );

  const body = h("tbody");
  let shown = 0;
  sheet.employees.forEach((emp, ei) => {
    if (!visible(emp)) return;
    shown += 1;
    const hoursRow = h("tr", { class: "hours-row" },
      h("td", { class: "sticky-1 nr", rowspan: 2 }, shown),
      h("td", { class: "sticky-2 name", rowspan: 2 },
        h("input", {
          class: "emp-name",
          value: emp.name,
          readonly: !onName,
          title: onName ? "Numele; îl poți corecta" : "Numele din lista de personal",
          onchange: onName && ((e) => (e.target.value = onName(ei, e.target.value))), // gets back the name in use (the old one if the new is empty or taken)
        }),
        h("input", { class: "emp-func", value: emp.function, readonly: !onFunction, title: "Funcția", onchange: onFunction && ((e) => onFunction(ei, e.target.value)) }),
        onTir ? tirToggle(emp, (on) => onTir(ei, on)) : null),
    );
    const nightRow = h("tr", { class: "night-row" });

    days.forEach((info, di) => {
      const entry = emp.days[di];
      const input = h("input", {
        class: cellClass(info, entry),
        value: cellText(entry),
        dataset: { k: "h", e: ei, d: di, sent: cellText(entry) },
        maxlength: 5,
        spellcheck: "false",
        onfocus: (e) => { e.target.select(); onSelect(ei, di); },
        onchange: (e) => sendOnce(e.target, (text) => onEdit(ei, di, hoursCellEdit(text))),
        onkeydown: (e) => navigate(e, container, ei, di),
      });
      whileTyping(input, () => sendOnce(input, (text) => onEdit(ei, di, hoursCellEdit(text))), { complete: isCompleteCell });
      hoursRow.append(h("td", { class: info.weekend || info.holiday ? "off" : "" }, input));

      // Night hours of the day: typed here they replace the calculated ones; empty = automatic.
      const sendNight = (text) => {
        const value = parseHours(text);
        input2.classList.toggle("invalid", value === undefined);
        if (value !== undefined) onEdit(ei, di, { overrideNight: value });
      };
      const input2 = h("input", {
        class: nightClass(entry),
        value: nightText(entry),
        dataset: { k: "n", e: ei, d: di, sent: nightText(entry) },
        maxlength: 5,
        spellcheck: "false",
        title: "Ore de noapte; scrise aici înlocuiesc calculul, gol = automat",
        onfocus: (e) => e.target.select(),
        onchange: (e) => sendOnce(e.target, sendNight),
        onkeydown: (e) => navigate(e, container, ei, di),
      });
      whileTyping(input2, () => sendOnce(input2, sendNight), { complete: (text) => parseHours(text) != null });
      nightRow.append(h("td", { class: `night ${info.weekend || info.holiday ? "off" : ""}` }, input2));
    });

    SUMS.forEach(({ key }, i) => {
      hoursRow.append(h("td", { class: `sum ${key}`, rowspan: 2, style: sumStyle(i), dataset: { sum: key, se: ei } }, totalValue(emp, key)));
    });
    body.append(hoursRow, nightRow);
  });

  container.replaceChildren(h("table", { class: "grid", dataset: { people: sheet.employees.length } }, h("thead", {}, head), body));
}

function totalValue(emp, key) {
  const t = emp.totals;
  if (key === "absences") {
    // OLP is the computed remainder, not an absence typed in the grid.
    const absences = t.codeHours.reduce((sum, v, i) => (KNOWN_CODES[i] === "OLP" ? sum : sum + v), 0);
    return absences ? hoursText(absences) : "";
  }
  return t[key] ? hoursText(t[key]) : "";
}

/** Refreshes values/classes in place (keeps focus and scroll position). */
export function refreshGrid(container, sheet) {
  container.querySelectorAll("input.cell").forEach((input) => {
    const entry = sheet.employees[input.dataset.e].days[input.dataset.d];
    const info = sheet.days[input.dataset.d];
    const night = input.dataset.k === "n";
    if (document.activeElement !== input) input.value = input.dataset.sent = night ? nightText(entry) : cellText(entry);
    input.className = night ? nightClass(entry) : cellClass(info, entry);
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
  // Up and down go to the next person on screen (people left out of the grid are skipped).
  const rows = Number(container.querySelector("table.grid")?.dataset.people ?? 0);
  for (let step = 1; step <= Math.max(rows, 1); step += 1) {
    const target = container.querySelector(`input.cell[data-k="${e.target.dataset.k}"][data-e="${ei + move[0] * step}"][data-d="${di + move[1]}"]`);
    if (target) return target.focus();
    if (!move[0]) return;
  }
}

/** Explains how a day was computed. */
export function renderDetails(target, sheet, ei, di) {
  const emp = sheet.employees[ei];
  const entry = emp.days[di];
  const info = sheet.days[di];
  const date = `${info.weekday} ${String(info.day).padStart(2, "0")}.${String(sheet.month).padStart(2, "0")}.${sheet.year}`;
  const parts = [h("b", {}, `${emp.name} · ${date}`)];
  if (info.holiday) parts.push(h("span", { class: "pill holiday" }, info.holiday));

  if (emp.group !== "tachograph" && emp.group !== "tir") {
    // Office, garage and city: the hours come from the plan of the person, not from a tachograph report.
  } else if (entry.workedMin > 0) {
    const blocks = entry.cardIntervals.map((s) => `${clock(s.start)}–${clock(s.end)}`).join(", ");
    parts.push(h("span", {}, `Tahograf: ${duration(entry.workedMin)} lucrat (${blocks})`));
  } else {
    parts.push(h("span", { class: "muted" }, "Fără activitate în PDF"));
  }
  if (entry.code) parts.push(h("span", { class: "pill code" }, `Absență: ${entry.code}`));
  if (entry.hours != null) {
    const ivs = entry.intervals.map((i) => `${clock(i.start)}–${clock(i.end)}`).join(" · ");
    parts.push(h("span", {}, `Pontaj: ${hoursText(entry.hours)} ore${entry.overrideHours != null ? " (manual)" : ""} → ${ivs}`));
    parts.push(h("span", { class: "muted" },
      entry.nightHours ? `Noapte: ${hoursText(entry.nightHours)} ore` : entry.nightMin > 0 ? `Noapte: ${duration(entry.nightMin)} (sub prag, nu se trece)` : "Fără ore de noapte"));
  }
  target.replaceChildren(...parts);
}
