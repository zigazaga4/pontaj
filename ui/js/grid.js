// Editable monthly grid: one hours row + one night row per driver.
import { h } from "./dom.js";
import { DAY, clock, duration, hhmm, hoursText, parseHm, parseHours } from "./time.js";
import { KNOWN_CODES, LEAVE_CODES, LEAVE_TYPES, hoursCellEdit, isCompleteCell, whileTyping } from "./table.js";
import { tirToggle } from "./tir.js";

// Total columns, pinned to the right edge of the grid.
const SUMS = [
  { key: "worked", label: "Lucrate" },
  { key: "night", label: "Noapte" },
  { key: "leave", label: "Concediu", hint: "Toate tipurile de concediu (CO, CM, CP, CFP, CC, CIM), în ore" },
  { key: "absences", label: "Absențe", hint: "Celelalte absențe (BP, AM, N, ZLP), în ore; concediul este în coloana lui" },
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

// The times of the day, as in the Excel file: under the hours a row with the start and a row with the end of each interval.
const TIME_LABELS = { s: "Ora începerii", e: "Ora terminării" };

/** The start (`side` "s") or the end ("e") of interval `slot` of the day, as the clock shows it ("" when the day has none). */
function timeText(entry, slot, side) {
  const interval = entry.intervals[slot];
  return interval ? hhmm(side === "s" ? interval.start : interval.end) : "";
}

const timeClass = (entry) => `cell time ${entry.intervalsOff ? "warn" : ""}`.trim();

/**
 * The edit for the start and the end typed for interval `slot` of a day: the intervals of the day with that one replaced, the hours
 * following their length. Both ends empty take the interval out (an interval that was not there leaves the day as it is). A time left as it was keeps its value (an end after midnight stays after midnight).
 */
function timesEdit(entry, slot, startText, endText) {
  const original = entry.intervals[slot];
  const read = (text, was) => (was != null && text.trim() === hhmm(was) ? was : parseHm(text));
  const from = read(startText, original?.start);
  const to = read(endText, original?.end);
  if (Number.isNaN(from) || Number.isNaN(to)) return {};
  const list = entry.intervals.map(({ start, end }) => ({ start, end }));
  if (from == null && to == null) {
    if (!original) return {}; // nothing was there and nothing is written: the day stays as it is
    list.splice(slot, 1);
  } else if (from == null || to == null) {
    return {};
  } else {
    let end = to;
    while (end <= from) end += DAY;
    while (end - DAY > from) end -= DAY;
    list[Math.min(slot, list.length)] = { start: from, end };
  }
  return { overrideIntervals: list.length ? list : null, overrideHours: null, code: null };
}

/**
 * Sends the start and the end of interval `slot` of a day, read from the two boxes: a time that is not a time marks them wrong and
 * sends nothing; a pair with one end still empty waits for the other end (its text stays through the refreshes).
 */
function sendTimes(container, ei, di, slot, onEdit) {
  const boxes = ["s", "e"].map((side) => container.querySelector(`input.cell.time[data-k="${side}"][data-e="${ei}"][data-d="${di}"][data-p="${slot}"]`));
  const wrong = boxes.some((box) => Number.isNaN(parseHm(box.value)));
  const half = boxes.filter((box) => box.value.trim() === "").length === 1;
  boxes.forEach((box) => {
    box.classList.toggle("invalid", wrong);
    box.dataset.pending = half && !wrong ? "1" : "";
  });
  if (wrong || half) return;
  onEdit(ei, di, (current) => timesEdit(current, slot, boxes[0].value, boxes[1].value));
}

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
 * make the name and the job read-only (the staff list decides them). `times` adds, under the hours and the night, the rows with
 * the start and the end of the day's interval ("Ora începerii", "Ora terminării", like the Excel file); typed there they reach
 * `onEdit` as a function `(entry) => edit` that the caller applies to the day as it is when the edit runs.
 */
export function renderGrid(container, sheet, { onEdit, onSelect, onFunction, onName, onTir, visible = () => true, heading = "Șofer", times = false }) {
  const days = sheet.days;
  const head = h("tr", {},
    h("th", { class: "sticky-1" }, "Nr"),
    h("th", { class: "sticky-2" }, heading),
    days.map((d) => h("th", { class: `day ${d.weekend || d.holiday ? "off" : ""} ${d.holiday ? "holiday" : ""}`, title: d.holiday ?? "" },
      h("div", {}, d.day), h("small", {}, d.weekday))),
    SUMS.map((s, i) => h("th", { class: "sum", style: sumStyle(i), title: s.hint ?? "" }, s.label)),
  );

  const body = h("tbody");
  let shown = 0;
  sheet.employees.forEach((emp, ei) => {
    if (!visible(emp)) return;
    shown += 1;
    // With the times, a person has a pair of rows (start, end) for every interval of its busiest day.
    const pairs = times ? Math.max(1, ...emp.days.map((d) => d.intervals.length)) : 0;
    const rows = 2 + 2 * pairs;
    const hoursRow = h("tr", { class: "hours-row" },
      h("td", { class: "sticky-1 nr", rowspan: rows }, shown),
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
    const timeRows = Array.from({ length: pairs * 2 }, (_, k) =>
      h("tr", { class: `time-row ${k % 2 ? "end" : "start"}` }, h("td", { class: "sticky-2 time-label" }, TIME_LABELS[k % 2 ? "e" : "s"])));

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

      // The start and the end of each interval of the day; the pair is sent whole once both are written.
      timeRows.forEach((row, k) => {
        const [slot, side] = [k >> 1, k % 2 ? "e" : "s"];
        const text = timeText(entry, slot, side);
        const input3 = h("input", {
          class: timeClass(entry),
          value: text,
          dataset: { k: side, e: ei, d: di, p: slot, sent: text },
          maxlength: 5,
          spellcheck: "false",
          placeholder: "––:––",
          title: `${TIME_LABELS[side]}; scrie ora (08:00) și ziua urmează orele, sau lasă ambele goale pentru o zi liberă`,
          onfocus: (e) => { e.target.select(); onSelect(ei, di); },
          onchange: (e) => sendOnce(e.target, () => sendTimes(container, ei, di, slot, onEdit)),
          onkeydown: (e) => navigateTimes(e, container, di),
        });
        row.append(h("td", { class: info.weekend || info.holiday ? "off" : "" }, input3));
      });
    });

    SUMS.forEach(({ key }, i) => {
      hoursRow.append(h("td", { class: `sum ${key}`, rowspan: rows, style: sumStyle(i), title: sumTitle(emp, key), dataset: { sum: key, se: ei } }, totalValue(emp, key)));
    });
    body.append(hoursRow, nightRow, ...timeRows);
  });

  container.replaceChildren(h("table", { class: "grid", dataset: { people: sheet.employees.length } }, h("thead", {}, head), body));
}

/** The hours of every code of the person that is a leave. */
const leaveHours = (emp) => LEAVE_TYPES.map(([code, name]) => [code, name, emp.totals.codeHours[KNOWN_CODES.indexOf(code)] ?? 0]).filter(([, , hours]) => hours);

function totalValue(emp, key) {
  const t = emp.totals;
  if (key === "leave") {
    const leave = leaveHours(emp).reduce((sum, [, , hours]) => sum + hours, 0);
    return leave ? hoursText(leave) : "";
  }
  if (key === "absences") {
    // Leave has its own column and OLP is the computed remainder: neither is an absence typed in the grid.
    const absences = t.codeHours.reduce((sum, v, i) => (KNOWN_CODES[i] === "OLP" || LEAVE_CODES.includes(KNOWN_CODES[i]) ? sum : sum + v), 0);
    return absences ? hoursText(absences) : "";
  }
  return t[key] ? hoursText(t[key]) : "";
}

/** What the cell of a total says when the mouse rests on it: for the leave, how much of each kind. */
function sumTitle(emp, key) {
  return key === "leave" ? leaveHours(emp).map(([code, name, hours]) => `${name} (${code}): ${hoursText(hours)}`).join("\n") : "";
}

/** Refreshes values/classes in place (keeps focus and scroll position). */
export function refreshGrid(container, sheet) {
  container.querySelectorAll("input.cell").forEach((input) => {
    const entry = sheet.employees[input.dataset.e].days[input.dataset.d];
    const info = sheet.days[input.dataset.d];
    const { k } = input.dataset;
    const timed = k === "s" || k === "e";
    const text = timed ? timeText(entry, Number(input.dataset.p), k) : k === "n" ? nightText(entry) : cellText(entry);
    // A pair of times being written (the start typed, the end not yet) stays as it is.
    if (document.activeElement !== input && !(timed && input.dataset.pending)) input.value = input.dataset.sent = text;
    input.className = timed ? timeClass(entry) : k === "n" ? nightClass(entry) : cellClass(info, entry);
    if (timed && Number.isNaN(parseHm(input.value))) input.classList.add("invalid");
  });
  container.querySelectorAll("td.sum[data-sum]").forEach((td) => {
    const emp = sheet.employees[td.dataset.se];
    td.textContent = totalValue(emp, td.dataset.sum);
    td.title = sumTitle(emp, td.dataset.sum);
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

/** Keys on the rows of times: up and down walk the column, left, right and Enter go to the next day of the same row. */
function navigateTimes(e, container, di) {
  const moves = { ArrowUp: [-1, 0], ArrowDown: [1, 0], Enter: [0, 1] };
  if (e.key === "ArrowLeft" && e.target.selectionStart === 0) moves.ArrowLeft = [0, -1];
  if (e.key === "ArrowRight" && e.target.selectionEnd === e.target.value.length) moves.ArrowRight = [0, 1];
  const move = moves[e.key];
  if (!move) return;
  e.preventDefault();
  e.target.dispatchEvent(new Event("change"));
  const column = [...container.querySelectorAll(`input.cell.time[data-d="${di}"]`)];
  const target = move[0] ? column[column.indexOf(e.target) + move[0]] : e.target.closest("tr").querySelector(`input.cell.time[data-d="${di + move[1]}"]`);
  target?.focus();
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
  if (entry.code) {
    const kind = LEAVE_CODES.includes(entry.code) ? "Concediu" : "Absență";
    parts.push(h("span", { class: "pill code" }, `${kind}: ${entry.code}`));
    if (entry.leaveOnFreeDay) parts.push(h("span", { class: "muted" }, "Zi liberă oricum: concediul apare în celulă, dar orele nu intră în total"));
  }
  if (entry.hours != null) {
    const ivs = entry.intervals.map((i) => `${clock(i.start)}–${clock(i.end)}`).join(" · ");
    parts.push(h("span", {}, `Pontaj: ${hoursText(entry.hours)} ore${entry.overrideHours != null ? " (manual)" : ""} → ${ivs}`));
    parts.push(h("span", { class: "muted" },
      entry.nightHours ? `Noapte: ${hoursText(entry.nightHours)} ore` : entry.nightMin > 0 ? `Noapte: ${duration(entry.nightMin)} (sub prag, nu se trece)` : "Fără ore de noapte"));
  }
  target.replaceChildren(...parts);
}
