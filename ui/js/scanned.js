// Step 1 — what was read from the PDF, row by row, editable.
import { h } from "./dom.js";
import { boundInput, enableKeyNav } from "./table.js";
import { duration, hhmm, parseHm } from "./time.js";

const WEEKDAYS = ["Dum", "Lun", "Mar", "Mie", "Joi", "Vin", "Sâm"];

// Typed columns first, then the ones the report derives from them.
const COLUMNS = [
  { key: "start", label: "Start", kind: "clock" },
  { key: "end", label: "Final", kind: "clock" },
  { key: "span", label: "Durată", kind: "calc" },
  { key: "driving", label: "Conducere", kind: "duration" },
  { key: "otherWork", label: "Alte munci", kind: "duration" },
  { key: "availability", label: "Disponib.", kind: "duration" },
  { key: "rest", label: "Pauză", kind: "duration" },
  { key: "dayWork", label: "Zi", kind: "calc" },
  { key: "nightWork", label: "Noapte", kind: "duration" },
  { key: "work", label: "Total lucrat", kind: "calc", strong: true },
];

const SUMMED = ["driving", "otherWork", "availability", "rest", "dayWork", "nightWork", "work"];

const weekday = (iso) => WEEKDAYS[new Date(`${iso}T12:00:00`).getDay()];

/** Clock cells accept "7:39"; blank is not a valid time. Duration cells treat blank as 0. */
const parsers = {
  clock: (text) => {
    const v = parseHm(text);
    return v === null || Number.isNaN(v) ? undefined : v;
  },
  duration: (text) => {
    const v = parseHm(text);
    return v === null ? 0 : Number.isNaN(v) ? undefined : v;
  },
};

/**
 * Renders the rows of driver `ei`. `ctx`: { sheet(), onEdit(ei, ai, patch), onAdd(ei), onDelete(ei, ai) }.
 * Returns { refresh() } to re-read the sheet without rebuilding the inputs.
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
    COLUMNS.map((c) => h("th", { class: c.kind === "calc" ? "calc" : "" }, c.label)),
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
      if (column.kind === "calc") {
        const cell = h("td", { class: `calc ${column.strong ? "strong" : ""}` });
        updaters.push(() => (cell.textContent = duration(row()[column.key])));
        tr.append(cell);
        continue;
      }
      tr.append(h("td", {}, add(boundInput({
        className: column.kind,
        maxlength: 5,
        get: () => row()[column.key],
        format: column.kind === "clock" ? hhmm : duration,
        parse: parsers[column.kind],
        classes: (v) => (column.kind === "duration" && v === 0 ? "zero" : ""),
        commit: (value) => edit({ [column.key]: value }),
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

  const intro = h("div", { class: "view-intro" },
    h("div", {},
      h("b", {}, "Ce a citit aplicația din PDF"),
      h("span", { class: "muted" }, ` · ${rows().length} rânduri. Corectează orice valoare sau adaugă/șterge rânduri; pontajul se recalculează pe loc.`)),
    h("button", { class: "btn small", onclick: () => ctx.onAdd(ei) }, "＋ Adaugă rând"));

  container.replaceChildren(
    intro,
    rows().length
      ? h("div", { class: "table-scroll" }, table)
      : h("div", { class: "empty-note" }, "Niciun rând pentru acest șofer. Adaugă unul cu butonul de mai sus."));

  const refresh = () => updaters.forEach((fn) => fn());
  refresh();
  return { refresh };
}
