// Step 2 — the pontaj written for each day of a driver: hours, start/end
// times and night hours, all editable, next to what the tachograph says.
import { h } from "./dom.js";
import { KNOWN_CODES, boundInput, enableKeyNav, parseCellInput } from "./table.js";
import { DAY, duration, hhmm, hoursText, parseHm, parseHours, signed } from "./time.js";

const day2 = (n) => String(n).padStart(2, "0");

/** Interval boundaries of a row: slot 0/1 × start/end. */
const SLOTS = [
  { slot: 0, side: "start" },
  { slot: 0, side: "end" },
  { slot: 1, side: "start" },
  { slot: 1, side: "end" },
];

const cardRange = (entry) => entry.cardIntervals.map((s) => `${hhmm(s.start)}–${hhmm(s.end)}`).join(", ");

/**
 * Renders the days of driver `ei`.
 * `ctx`: { sheet(), settings(), onDayEdit(ei, di, patch) }. Returns { refresh() }.
 */
export function renderDaily(container, ctx, ei) {
  const employee = () => ctx.sheet().employees[ei];
  const entryOf = (di) => employee().days[di];
  const updaters = [];
  const add = (binding) => {
    updaters.push(binding.update);
    return binding.el;
  };

  const head = h("tr", {},
    h("th", { rowspan: 2 }, "Zi"),
    h("th", { colspan: 2, class: "card-head" }, "Pe card (tahograf)"),
    h("th", { colspan: 4, class: "pontaj-head" }, "Pontaj"),
    h("th", { rowspan: 2, class: "calc" }, "Față de card"));
  // Each interval cell holds two inputs (de la – până la).
  const sub = h("tr", {},
    h("th", { class: "card-head" }, "Lucrat"),
    h("th", { class: "card-head" }, "Program"),
    h("th", { class: "pontaj-head" }, "Ore / cod"),
    h("th", { class: "pontaj-head" }, "Interval 1 (de la – până la)"),
    h("th", { class: "pontaj-head" }, "Interval 2 (de la – până la)"),
    h("th", { class: "pontaj-head" }, "Noapte"));

  const body = h("tbody");
  ctx.sheet().days.forEach((info, di) => {
    const entry = () => entryOf(di);
    const off = info.weekend || info.holiday;
    const tr = h("tr", { class: `day-row ${off ? "off" : ""} ${info.holiday ? "holiday" : ""}`, title: info.holiday ?? "" });

    tr.append(h("td", { class: "day-name" }, h("b", {}, day2(info.day)), " ", h("small", {}, info.weekday)));

    const worked = h("td", { class: "calc" });
    const program = h("td", { class: "calc small" });
    updaters.push(() => {
      worked.textContent = entry().workedMin > 0 ? duration(entry().workedMin) : "–";
      program.textContent = cardRange(entry());
      program.title = entry().pdfNightMin > 0 ? `Noapte pe card: ${duration(entry().pdfNightMin)}` : "";
    });
    tr.append(worked, program);

    // Hours or an absence code, like the cells of the monthly sheet.
    tr.append(h("td", {}, add(boundInput({
      className: "hours",
      maxlength: 5,
      get: entry,
      format: (e) => e.code ?? (e.hours != null ? hoursText(e.hours) : ""),
      parse: (text) => parseCellInput(text),
      classes: (e) =>
        [e.code ? (KNOWN_CODES.includes(e.code) ? "code" : "code unknown") : "", e.overrideHours != null || (e.hours != null && e.workedMin === 0 && !e.code) ? "manual" : ""].join(" "),
      title: (e) => (e.overrideHours != null ? "Ore trecute manual" : "Ore întregi sau jumătăți: 8 · 7,5 · 7:30 · 730"),
      commit: (patch) => ctx.onDayEdit(ei, di, patch),
    }))));

    // The four interval boundaries. Any change sends all of them together.
    const inputs = [];
    const boundary = ({ slot, side }) => {
      const value = () => entry().intervals[slot]?.[side];
      const binding = boundInput({
        className: "clock",
        maxlength: 5,
        placeholder: "––:––",
        get: value,
        format: (v) => (v == null ? "" : hhmm(v)),
        parse: (text) => {
          const v = parseHm(text);
          return Number.isNaN(v) ? undefined : v;
        },
        classes: () => (entry().overrideIntervals ? "manual" : entry().intervalsOff ? "warn" : ""),
        title: () => (entry().intervalsOff ? "Intervalele nu însumează orele trecute" : ""),
        commit: () => commitIntervals(),
      });
      inputs.push({ slot, side, binding, value });
      updaters.push(binding.update);
      return binding.el;
    };
    const commitIntervals = () => {
      const read = ({ side, binding, value }) => {
        const text = binding.el.value.trim();
        const original = value();
        if (original != null && text === hhmm(original)) return original; // untouched
        const parsed = parseHm(text);
        // A start typed on a day that begins the previous evening stays on that evening.
        return side === "start" && original < 0 && parsed >= 12 * 60 ? parsed - DAY : parsed;
      };
      const values = inputs.map(read);
      if (values.some(Number.isNaN)) return;
      const intervals = [];
      for (const slot of [0, 1]) {
        const [from, to] = values.slice(slot * 2, slot * 2 + 2);
        if (from == null && to == null) continue;
        if (from == null || to == null) {
          inputs.slice(slot * 2, slot * 2 + 2).forEach((i) => i.binding.el.classList.add("invalid"));
          return; // wait until both ends are filled in
        }
        let end = to;
        while (end <= from) end += DAY;
        intervals.push({ start: from, end });
      }
      inputs.forEach((i) => i.binding.el.classList.remove("invalid"));
      ctx.onDayEdit(ei, di, { overrideIntervals: intervals.length ? intervals : null });
    };
    tr.append(
      h("td", { class: "pair" }, boundary(SLOTS[0]), h("span", { class: "dash" }, "–"), boundary(SLOTS[1])),
      h("td", { class: "pair" }, boundary(SLOTS[2]), h("span", { class: "dash" }, "–"), boundary(SLOTS[3])));

    tr.append(h("td", {}, add(boundInput({
      className: "night",
      maxlength: 5,
      get: entry,
      format: (e) => (e.nightHours != null ? hoursText(e.nightHours) : ""),
      parse: parseHours,
      classes: (e) => (e.overrideNight != null ? "manual" : ""),
      title: (e) =>
        e.overrideNight != null
          ? "Ore de noapte trecute manual"
          : e.nightMin > 0
            ? `Noapte: ${duration(e.nightMin)}${e.nightHours ? "" : " (sub prag, nu se trece)"}`
            : "",
      commit: (value) => ctx.onDayEdit(ei, di, { overrideNight: value }),
    }))));

    const diff = h("td", { class: "calc diff" });
    updaters.push(() => {
      const e = entry();
      if (e.hours == null) {
        diff.textContent = "";
        diff.className = "calc diff";
        return;
      }
      if (e.workedMin === 0) {
        diff.textContent = "fără card";
        diff.className = "calc diff manual";
        return;
      }
      const d = e.hours * 60 - e.workedMin;
      diff.textContent = signed(d);
      diff.className = `calc diff ${d > 0 ? "plus" : d < 0 ? "minus" : ""}`;
    });
    tr.append(diff);
    body.append(tr);
  });

  // Totals for the month.
  const totals = { worked: h("td", { class: "calc strong" }), hours: h("td", { class: "strong" }), night: h("td", { class: "strong" }), diff: h("td", { class: "calc diff strong" }) };
  const foot = h("tr", { class: "total-row" },
    h("td", {}, "Total"), totals.worked, h("td", {}), totals.hours, h("td", { colspan: 2 }), totals.night, totals.diff);
  updaters.push(() => {
    const t = employee().totals;
    totals.worked.textContent = duration(employee().days.reduce((sum, d) => sum + d.workedMin, 0));
    totals.hours.textContent = `${hoursText(t.worked)} ore`;
    totals.night.textContent = t.night ? hoursText(t.night) : "";
    const diff = t.worked * 60 - t.cardMin;
    totals.diff.textContent = signed(diff);
    totals.diff.className = `calc diff strong ${diff > 0 ? "plus" : diff < 0 ? "minus" : ""}`;
  });

  const table = h("table", { class: "data-table daily" }, h("thead", {}, head, sub), body, h("tfoot", {}, foot));
  enableKeyNav(table);

  const intro = h("div", { class: "view-intro" },
    h("div", {},
      h("b", {}, "Pontajul pe zile"),
      h("span", { class: "muted" }, " · scrie ore întregi sau jumătăți (8 · 7,5 · 7:30 · 730) sau cod (CO, CM…), orele de început/sfârșit și orele de noapte; gol = calcul automat. Portocaliu = trecut manual.")));

  container.replaceChildren(intro, h("div", { class: "table-scroll" }, table));
  const refresh = () => updaters.forEach((fn) => fn());
  refresh();
  return { refresh };
}
