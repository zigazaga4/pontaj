// Step 2 — the pontaj written for each day of a driver: hours, start/end
// times, the break between them and night hours, all editable, next to what the tachograph says.
// The hours follow the times (and the times the hours) while typing.
import { h } from "./dom.js";
import { KNOWN_CODES, boundInput, enableKeyNav, hoursCellEdit, isCompleteCell } from "./table.js";
import { driverFields } from "./driver_fields.js";
import { DAY, duration, hhmm, hoursText, parseHm, parseHours, parseLength, signed } from "./time.js";

const day2 = (n) => String(n).padStart(2, "0");

/** Interval boundaries of a row: slot 0/1 × start/end. */
const SLOTS = [
  { slot: 0, side: "start" },
  { slot: 0, side: "end" },
  { slot: 1, side: "start" },
  { slot: 1, side: "end" },
];

const cardRange = (entry) => entry.cardIntervals.map((s) => `${hhmm(s.start)}–${hhmm(s.end)}`).join(", ");

const sumMin = (list) => list.reduce((sum, i) => sum + i.end - i.start, 0);

/** The break between the first two written intervals (minutes; negative if they overlap), or null with fewer than two. */
const pauseOf = (entry) => (entry.intervals.length >= 2 ? entry.intervals[1].start - entry.intervals[0].end : null);

const pauseText = (min) => (min < 0 ? signed(min) : duration(min));

/**
 * The edit that sets the times of a day. On the grid the hours of the day are the length of the times (the sheet rounds
 * them to the half hour), so they follow while typing. With the real card times (one block that includes the breaks)
 * the hours stay what they were, unless the day had none. No time at all is always a day of zero hours.
 */
function intervalsEdit(list, entry, settings) {
  const follow = settings.intervalMode !== "card" || entry.hours == null;
  const hours = list.length === 0 ? { overrideHours: 0 } : follow ? { overrideHours: sumMin(list) / 60 } : {};
  return { overrideIntervals: list.length ? list : null, ...hours, code: null };
}

const hasEdits = (e) => e.overrideHours != null || e.overrideNight != null || !!e.overrideIntervals || !!e.code;

/** The times a new interval gets: a half of the normal day first, then what is missing from the day after a break. */
function withNewInterval(list, entry, settings) {
  const slot = Math.max(1, settings.slotMin);
  const day = settings.hoursPerDay * 60;
  if (!list.length) {
    const typed = parseHm(settings.defaultStart);
    const start = entry.firstStart ?? (typed == null || Number.isNaN(typed) ? 8 * 60 : typed);
    return [{ start, end: start + Math.max(slot, Math.round(day / 2 / slot) * slot) }];
  }
  const start = list[list.length - 1].end + settings.breakMin;
  return [...list, { start, end: start + Math.max(slot, day - sumMin(list)) }];
}

/**
 * Renders the days of driver `ei`.
 * `ctx`: { sheet(), settings(), onDayEdit(ei, di, patch), onEmployeeEdit(ei, patch), onOpenScanned(isoDate) }.
 * Returns { refresh() }.
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
    h("th", { colspan: 3, class: "card-head" }, "Pe card (tahograf)"),
    h("th", { colspan: 5, class: "pontaj-head" }, "Pontaj"),
    h("th", { rowspan: 2, class: "calc" }, "Față de card"),
    h("th", { rowspan: 2 }));
  // Each interval cell holds two inputs (de la – până la).
  const sub = h("tr", {},
    h("th", { class: "card-head" }, "Lucrat"),
    h("th", { class: "card-head", title: "Click pe un program ca să-i modifici rândurile din PDF" }, "Program"),
    h("th", { class: "card-head", title: "Pauzele înregistrate de tahograf în ziua asta (coloana Pauză din raport)" }, "Pauză"),
    h("th", { class: "pontaj-head" }, "Ore / cod"),
    h("th", { class: "pontaj-head" }, "Interval 1 (de la – până la)"),
    h("th", { class: "pontaj-head", title: "Pauza dintre cele două intervale; o poți modifica" }, "Pauză"),
    h("th", { class: "pontaj-head" }, "Interval 2 (de la – până la)"),
    h("th", { class: "pontaj-head" }, "Noapte"));

  const body = h("tbody");
  ctx.sheet().days.forEach((info, di) => {
    const entry = () => entryOf(di);
    const off = info.weekend || info.holiday;
    const tr = h("tr", { class: `day-row ${off ? "off" : ""} ${info.holiday ? "holiday" : ""}`, title: info.holiday ?? "" });
    const isoDay = () => `${ctx.sheet().year}-${day2(ctx.sheet().month)}-${day2(info.day)}`;
    // What is sent for a change of the times of this day.
    const sendIntervals = (list) => ctx.onDayEdit(ei, di, intervalsEdit([...list].sort((a, b) => a.start - b.start), entry(), ctx.settings()));
    const intervalsNow = () => entry().intervals.map(({ start, end }) => ({ start, end }));

    tr.append(h("td", { class: "day-name" }, h("b", {}, day2(info.day)), " ", h("small", {}, info.weekday)));

    const worked = h("td", { class: "calc" });
    const program = h("td", { class: "calc small card-link", onclick: () => entry().workedMin > 0 && ctx.onOpenScanned(isoDay()) });
    const cardBreak = h("td", { class: "calc small" });
    updaters.push(() => {
      worked.textContent = entry().workedMin > 0 ? duration(entry().workedMin) : "–";
      program.textContent = cardRange(entry());
      program.title = [entry().workedMin > 0 ? "Click ca să modifici rândurile din PDF ale zilei" : "", entry().pdfNightMin > 0 ? `Noapte pe card: ${duration(entry().pdfNightMin)}` : ""].filter(Boolean).join(" · ");
      cardBreak.textContent = entry().restMin > 0 ? duration(entry().restMin) : "";
    });
    tr.append(worked, program, cardBreak);

    // Hours or an absence code, like the cells of the monthly sheet. Typing here makes the times again from the hours.
    tr.append(h("td", {}, add(boundInput({
      className: "hours",
      maxlength: 5,
      live: true,
      complete: isCompleteCell,
      get: entry,
      format: (e) => e.code ?? (e.hours != null ? hoursText(e.hours) : ""),
      parse: hoursCellEdit,
      classes: (e) =>
        [e.code ? (KNOWN_CODES.includes(e.code) ? "code" : "code unknown") : "", e.overrideHours != null || (e.hours != null && e.workedMin === 0 && !e.code) ? "manual" : "", e.hours === 0 ? "zero-day" : ""].join(" "),
      title: (e) => (e.overrideHours != null ? "Ore trecute manual (0 = fără muncă în ziua asta; gol = calculul automat)" : "Ore întregi sau cu minute: 8 · 7:30 · 730 · 7,30 (se rotunjește la jumătate de oră). 0 = fără muncă"),
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
        live: true,
        get: value,
        format: (v) => (v == null ? "" : hhmm(v)),
        parse: (text) => {
          const v = parseHm(text);
          return Number.isNaN(v) ? undefined : v;
        },
        classes: () => (entry().overrideIntervals ? "manual" : entry().intervalsOff ? "warn" : ""),
        title: () => (entry().intervalsOff ? "Intervalele nu însumează orele trecute" : ""),
        commit: (_, { final }) => commitIntervals(final),
      });
      inputs.push({ slot, side, binding, value });
      updaters.push(binding.update);
      return binding.el;
    };
    const commitIntervals = (final) => {
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
          // Wait until both ends are filled in; while typing, say nothing yet.
          if (final) inputs.slice(slot * 2, slot * 2 + 2).forEach((i) => i.binding.el.classList.add("invalid"));
          return;
        }
        let end = to;
        while (end <= from) end += DAY;
        intervals.push({ start: from, end });
      }
      inputs.forEach((i) => i.binding.el.classList.remove("invalid"));
      sendIntervals(intervals);
    };

    // ✕ deletes an interval (the last one leaves a day of zero hours), ＋ adds one after the others.
    const slotButton = (slot) => {
      const button = h("button", { type: "button", class: "icon-btn slot" });
      button.addEventListener("click", () => {
        const list = intervalsNow();
        if (list[slot]) {
          list.splice(slot, 1);
          sendIntervals(list);
        } else {
          sendIntervals(withNewInterval(list, entry(), ctx.settings()));
        }
      });
      updaters.push(() => {
        const count = entry().intervals.length;
        const has = slot < count;
        button.hidden = !has && slot !== count; // ＋ only on the first free place
        button.textContent = has ? "✕" : "＋";
        button.title = has ? "Șterge intervalul" : "Adaugă un interval";
        button.classList.toggle("add", !has);
      });
      return button;
    };

    // The break between the two intervals: changing it moves the second interval, which keeps its length.
    const pause = boundInput({
      className: "clock pause",
      maxlength: 5,
      placeholder: "––",
      live: true,
      get: entry,
      format: (e) => (pauseOf(e) == null ? "" : pauseText(pauseOf(e))),
      parse: parseLength,
      classes: (e) => (pauseOf(e) != null && pauseOf(e) < 0 ? "warn" : e.overrideIntervals && pauseOf(e) != null ? "manual" : ""),
      title: (e) => (pauseOf(e) == null ? "Pauza apare când ziua are două intervale" : pauseOf(e) < 0 ? "Intervalele se suprapun" : "Pauza dintre cele două intervale. Dacă o schimbi, se mută al doilea interval"),
      commit: (minutes) => {
        const [first, second] = intervalsNow();
        if (!second) return;
        const start = first.end + minutes;
        sendIntervals([first, { start, end: start + second.end - second.start }]);
      },
    });
    updaters.push(pause.update, () => (pause.el.readOnly = entry().intervals.length < 2));

    tr.append(
      h("td", { class: "pair" }, boundary(SLOTS[0]), h("span", { class: "dash" }, "–"), boundary(SLOTS[1]), slotButton(0)),
      h("td", {}, pause.el),
      h("td", { class: "pair" }, boundary(SLOTS[2]), h("span", { class: "dash" }, "–"), boundary(SLOTS[3]), slotButton(1)));

    tr.append(h("td", {}, add(boundInput({
      className: "night",
      maxlength: 5,
      live: true,
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
      if (e.hours == null || (e.hours === 0 && e.workedMin === 0)) {
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

    // Back to the automatic values for the whole day (hours, times, night, absence).
    const reset = h("button", {
      type: "button",
      class: "icon-btn reset",
      title: "Înapoi la calculul automat pentru ziua asta",
      onclick: () => ctx.onDayEdit(ei, di, { overrideHours: null, overrideIntervals: null, overrideNight: null, code: null }),
    }, "↺");
    updaters.push(() => (reset.hidden = !hasEdits(entry())));
    tr.append(h("td", {}, reset));
    body.append(tr);
  });

  // Totals for the month.
  const totals = { worked: h("td", { class: "calc strong" }), hours: h("td", { class: "strong" }), night: h("td", { class: "strong" }), diff: h("td", { class: "calc diff strong" }) };
  const foot = h("tr", { class: "total-row" },
    h("td", {}, "Total"), totals.worked, h("td", { colspan: 2 }), totals.hours, h("td", { colspan: 3 }), totals.night, totals.diff, h("td", {}));
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

  const fields = driverFields(ctx, ei);
  updaters.push(fields.update);
  const intro = h("div", { class: "view-intro" },
    h("div", {},
      h("b", {}, "Pontajul pe zile"),
      h("span", { class: "muted" }, " · totul se recalculează pe măsură ce scrii. Ore (8 · 7:30 · 730 · 7,30) sau cod (CO, CM…); 0 = fără muncă; ✕ șterge un interval, ＋ adaugă unul; pauza dintre intervale se poate modifica; gol = calcul automat, ↺ = toată ziua înapoi la automat. Albastru = trecut manual.")),
    fields.el);

  container.replaceChildren(intro, h("div", { class: "table-scroll" }, table));
  const refresh = () => updaters.forEach((fn) => fn());
  refresh();
  return { refresh };
}
