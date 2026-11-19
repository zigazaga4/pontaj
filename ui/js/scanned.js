// Step 1 — what was read from the PDF, row by row, editable.
import { h } from "./dom.js";
import { boundInput, enableKeyNav } from "./table.js";
import { driverFields } from "./driver_fields.js";
import { DAY, duration, hhmm, parseHm, parseLength } from "./time.js";

const WEEKDAYS = ["Dum", "Lun", "Mar", "Mie", "Joi", "Vin", "Sâm"];

// The columns of the report. Those marked `derived` the report works out from the others, but they can be typed too:
// see DERIVED for what a typed value changes.
const COLUMNS = [
  { key: "start", label: "Start", kind: "clock" },
  { key: "end", label: "Final", kind: "clock" },
  { key: "span", label: "Durată", kind: "duration", derived: true, min: 1, hint: "Durata blocului. Dacă o schimbi, se mută ora de final" },
  { key: "driving", label: "Conducere", kind: "duration" },
  { key: "otherWork", label: "Alte munci", kind: "duration" },
  { key: "availability", label: "Disponib.", kind: "duration" },
  { key: "rest", label: "Pauză", kind: "duration" },
  { key: "dayWork", label: "Zi", kind: "duration", derived: true, hint: "Munca de zi. Dacă o schimbi, se modifică munca de noapte" },
  { key: "nightWork", label: "Noapte", kind: "duration" },
  { key: "work", label: "Total lucrat", kind: "duration", derived: true, strong: true, hint: "Timpul lucrat (conducere + alte munci). Dacă îl schimbi, se modifică „Alte munci”" },
];

const SUMMED = ["driving", "otherWork", "availability", "rest", "dayWork", "nightWork", "work"];

const weekday = (iso) => WEEKDAYS[new Date(`${iso}T12:00:00`).getDay()];

/** Clock cells accept "7:39"; blank is not a valid time. Duration cells accept "7:39", "45" (minutes) and treat blank as 0. */
const parsers = {
  clock: (text) => {
    const v = parseHm(text);
    return v === null || Number.isNaN(v) ? undefined : v;
  },
  duration: (text, column) => {
    const v = parseLength(text);
    return v !== undefined && v < (column.min ?? 0) ? undefined : v;
  },
};

/** What a typed value of a derived column changes in the columns it is worked out from. */
const DERIVED = {
  span: (row, minutes) => ({ end: row.start + minutes }),
  work: (row, minutes) => (minutes >= row.driving ? { otherWork: minutes - row.driving } : { driving: minutes, otherWork: 0 }),
  dayWork: (row, minutes) => ({ nightWork: Math.max(0, row.work - minutes) }),
};

const lengthOf = (start, end) => (end <= start ? end + DAY - start : end - start);

/**
 * A block that is all work (a row added by hand) stays all work when its start or end moves, so the hours follow
 * the times. Blocks read from the report, with breaks in them, are left as they are.
 */
function keepAllWork(row, patch) {
  if (!("start" in patch || "end" in patch)) return patch;
  if (row.availability || row.rest || row.driving + row.otherWork !== lengthOf(row.start, row.end)) return patch;
  const span = lengthOf(patch.start ?? row.start, patch.end ?? row.end);
  const driving = Math.min(row.driving, span);
  return { ...patch, driving, otherWork: span - driving };
}

/**
 * Renders the rows of driver `ei`. `ctx`: { sheet(), onEdit(ei, ai, patch), onAdd(ei), onDelete(ei, ai),
 * onEmployeeEdit(ei, patch), takeFocusDate() }. Returns { refresh() } to re-read the sheet without rebuilding the inputs.
 */
export function renderScanned(container, ctx, ei) {
  const rows = () => ctx.sheet().employees[ei].activities;
  const updaters = [];
  const add = (binding) => {
    updaters.push(binding.update);
    return binding.el;
  };

  const head = h("tr", {},
    h("th", {}, "Data"),
    h("th", {}, "Zi"),
    COLUMNS.map((c) => h("th", { class: c.derived ? "calc" : "", title: c.hint ?? "" }, c.label)),
    h("th", { class: "calc" }, "Fișier"),
    h("th", {}));

  const body = h("tbody");
  rows().forEach((_, ai) => {
    const row = () => rows()[ai];
    const edit = (patch) => ctx.onEdit(ei, ai, { ...patch, manual: true });
    const tr = h("tr", { class: "scan-row" });
    updaters.push(() => {
      const r = row();
      const month = ctx.sheet().month;
      tr.dataset.date = r.date;
      tr.className = `scan-row ${r.manual ? "manual-row" : ""} ${Number(r.date.slice(5, 7)) !== month ? "outside" : ""}`.trim();
      tr.title = Number(r.date.slice(5, 7)) !== month ? "Rândul este din altă lună și nu intră în pontaj" : "";
    });

    tr.append(h("td", {}, add(boundInput({
      type: "date",
      className: "date",
      get: () => row().date,
      parse: (text) => text || undefined,
      commit: (value) => edit({ date: value }),
    }))));
    const dayCell = h("td", { class: "muted" });
    updaters.push(() => (dayCell.textContent = weekday(row().date)));
    tr.append(dayCell);

    for (const column of COLUMNS) {
      tr.append(h("td", { class: column.strong ? "strong" : "" }, add(boundInput({
        className: column.kind,
        maxlength: 5,
        live: true,
        title: column.hint ?? "",
        get: () => row()[column.key],
        format: column.kind === "clock" ? hhmm : duration,
        parse: (text) => parsers[column.kind](text, column),
        classes: (v) => [column.kind === "duration" && v === 0 ? "zero" : "", column.derived ? "derived" : ""].join(" "),
        commit: (value) => edit(keepAllWork(row(), DERIVED[column.key] ? DERIVED[column.key](row(), value) : { [column.key]: value })),
      }))));
    }

    const source = h("td", { class: "calc source" });
    updaters.push(() => {
      source.textContent = row().source ? row().source.replace(/\.pdf$/i, "") : "adăugat manual";
      source.title = row().source || "";
    });
    tr.append(source, h("td", {}, h("button", { class: "icon-btn", title: "Șterge rândul", onclick: () => ctx.onDelete(ei, ai) }, "✕")));
    body.append(tr);
  });

  const foot = h("tr", { class: "total-row" }, h("td", { colspan: 2 }, "Total"));
  const totalCells = new Map();
  for (const column of COLUMNS) {
    const cell = h("td", { class: "calc strong" });
    totalCells.set(column.key, cell);
    foot.append(cell);
  }
  foot.append(h("td", { colspan: 2 }));
  updaters.push(() => {
    for (const key of SUMMED) {
      totalCells.get(key).textContent = duration(rows().reduce((sum, r) => sum + r[key], 0));
    }
  });

  const table = h("table", { class: "data-table" }, h("thead", {}, head), body, h("tfoot", {}, foot));
  enableKeyNav(table);

  const fields = driverFields(ctx, ei);
  updaters.push(fields.update);
  const intro = h("div", { class: "view-intro" },
    h("div", {},
      h("b", {}, "Ce a citit aplicația din PDF"),
      h("span", { class: "muted" }, ` · ${rows().length} rânduri. Orice valoare se poate corecta (și Durata, Zi, Total lucrat), iar rândurile se adaugă sau se șterg; pontajul se recalculează pe măsură ce scrii.`)),
    fields.el,
    h("button", { class: "btn small", onclick: () => ctx.onAdd(ei) }, "＋ Adaugă rând"));

  container.replaceChildren(
    intro,
    rows().length
      ? h("div", { class: "table-scroll" }, table)
      : h("div", { class: "empty-note" }, "Niciun rând pentru acest șofer. Adaugă unul cu butonul de mai sus."));

  const refresh = () => updaters.forEach((fn) => fn());
  refresh();

  // Coming from a day of the pontaj: show the rows of that day.
  const focus = ctx.takeFocusDate?.();
  if (focus) {
    const shown = [...body.querySelectorAll("tr")].filter((tr) => tr.dataset.date === focus);
    shown[0]?.scrollIntoView({ block: "center" });
    shown.forEach((tr) => tr.classList.add("flash"));
    setTimeout(() => shown.forEach((tr) => tr.classList.remove("flash")), 2200);
  }
  return { refresh };
}
