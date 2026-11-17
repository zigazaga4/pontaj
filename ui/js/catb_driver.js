// Categoria B — one driver: his trips, the waiting between them and what the office decided, all editable.
// The view is drawn again after every edit (the sheet is recomputed in Rust); inputs carry `data-key` so the
// controller can put the cursor back where it was.
import { h } from "./dom.js";
import { showDialog } from "./dialog.js";
import { boundInput } from "./table.js";
import { duration, formatWhen, hoursText, isoAt, isoClock, isoDate, lei, parseHm, parseHours, parseWhen, shortDate, spanText, weekdayName } from "./time.js";

export const KIND_LABELS = { eco: "Eco", private: "Privată", vip: "VIP", bus: "Autocar", passenger: "Pasager" };
const PAUSE_STATES = [
  ["auto", "Automat (după reguli)"],
  ["stayed", "A stat acolo"],
  ["passenger", "S-a întors ca pasager"],
  ["off", "Zi liberă"],
  ["excursion", "Excursie (iese din Categoria B)"],
];
const PAUSE_BADGES = { normal: "Staționare", confirm: "De decis", rest: "Pauză lungă", overlap: "Suprapunere" };

/** One sentence under the pause: whether it counts in the hours, and what the choice changes. */
function pauseHint(pause) {
  switch (pause.state) {
    case "stayed":
      return "Se numără toată pauza de mai sus (sau scrie tu orele).";
    case "excursion":
      return "Excursia se pontează din tahograf, deci iese din Categoria B: nu se numără nimic aici, nici pauza, nici cursele din ea, nici banii.";
    case "passenger":
      return "Se numără de la sosire până la ora la care a plecat. Completează „A plecat la” după foaia șoferului; fără ea se numără ca în starea automată.";
    case "off":
      return "Nu se numără nimic din această pauză.";
    default:
      if (pause.kind === "confirm") return "Nu intră în ore până alegi ce a fost.";
      if (pause.kind === "rest") return "Lăsată automat, se socotește pauză (s-a întors ca pasager sau altfel), nu staționare: nu intră în ore și nu se plătește. Dacă a stat acolo, alege „A stat acolo”.";
      if (pause.kind === "overlap") return "Cursele se suprapun: verifică orele lor.";
      return "Toată așteptarea dintre cele două curse se numără ca staționare.";
  }
}

/** Pauses whose optional fields (hours, note) were opened by hand although nothing was decided yet. */
const opened = new Set();
const NO_TARIFF = "fără tarif";

// ---------------------------------------------------------------------------------------------------------------------
// Changes to a driver (plain data; the caller recomputes in Rust afterwards)
// ---------------------------------------------------------------------------------------------------------------------

const emptyPauseEdit = (e) => e.state === "auto" && e.hours == null && e.left == null && e.back == null && !e.note;

/** Sets what the office decided about the pause between two trips; an edit that says nothing is dropped. */
export function setPause(driver, pause, patch) {
  let edit = driver.pauseEdits.find((e) => e.prev === pause.prev && e.next === pause.next);
  if (!edit) {
    edit = { prev: pause.prev, next: pause.next, state: "auto", hours: null, left: null, back: null, note: "" };
    driver.pauseEdits.push(edit);
  }
  Object.assign(edit, patch);
  if (emptyPauseEdit(edit)) driver.pauseEdits.splice(driver.pauseEdits.indexOf(edit), 1);
}

/** What the API says is the reference: typing the same value again undoes the edit. */
function setTripField(trip, field, value, original) {
  trip.edit[field] = value === original || value === "" ? null : value;
}

export function applyProposal(driver, proposal) {
  if (proposal.kind === "day") {
    if (!driver.excludedDays.includes(proposal.date)) driver.excludedDays.push(proposal.date);
  } else {
    setPause(driver, { prev: proposal.prev, next: proposal.next }, { hours: proposal.setHours });
  }
}

export function toggleDay(driver, date) {
  const at = driver.excludedDays.indexOf(date);
  if (at >= 0) driver.excludedDays.splice(at, 1);
  else driver.excludedDays.push(date);
}

// ---------------------------------------------------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------------------------------------------------

/** A bound input that already shows its value. */
function bound(options) {
  const binding = boundInput(options);
  binding.update();
  return binding.el;
}

function alertList(alerts) {
  return h("ul", { class: "alerts" }, alerts.map((a) => h("li", { class: `alert level-${a.level}` }, a.text)));
}

export const chip = (label, value, { title, tone, onclick } = {}) => h("div", { class: `chip ${tone ?? ""} ${onclick ? "clickable" : ""}`, title, onclick }, h("small", {}, label), h("b", {}, value));

function priceInput(trip, edit) {
  return bound({
    className: "price",
    dataset: { key: `price:${trip.id}` },
    get: () => trip,
    format: (t) => (t.eff.priceNote === NO_TARIFF ? "" : String(t.eff.price)),
    parse: (text) => {
      const value = text.trim().replace(",", ".");
      if (value === "") return null;
      const n = Number(value);
      return Number.isFinite(n) && n >= 0 ? n : undefined;
    },
    classes: (t) => (t.eff.priceNote === NO_TARIFF ? "warn" : t.edit.price != null ? "manual" : ""),
    title: (t) => `${t.eff.priceNote}${t.edit.price != null ? " · scrie gol ca să revii la tariful din listă" : ""}`,
    placeholder: "?",
    commit: (price) => edit((d) => (d.trips.find((x) => x.id === trip.id).edit.price = price)),
  });
}

function placeInput(trip, side, edit) {
  return bound({
    className: "place",
    dataset: { key: `${side}:${trip.id}` },
    list: "catb-places",
    get: () => trip,
    format: (t) => t.eff[side],
    parse: (text) => text.trim(),
    classes: (t) => (t.edit[side] != null ? "manual" : ""),
    title: (t) => (t.edit[side] != null ? `Din API: ${t[side]}` : ""),
    commit: (text) => edit((d) => setTripField(d.trips.find((x) => x.id === trip.id), side, text, trip[side])),
  });
}

/** Departure or arrival, typed as a time of day: the date stays, an arrival before the departure is the next day. */
function timeInput(trip, side, edit) {
  const effective = (t) => t.eff[side];
  return bound({
    className: "clock",
    dataset: { key: `${side === "departure" ? "dep" : "arr"}:${trip.id}` },
    maxlength: 5,
    placeholder: "–",
    get: () => trip,
    format: (t) => isoClock(effective(t)),
    parse: (text) => {
      const min = parseHm(text);
      return Number.isNaN(min) ? undefined : min;
    },
    classes: (t) => (t.edit[side] != null ? "manual" : ""),
    title: (t) => (t.edit[side] != null ? `Din API: ${isoClock(t[side]) || "necunoscut"}` : side === "arrival" ? "Ora sosirii. Gol = necunoscută." : ""),
    commit: (min) =>
      edit((d) => {
        const target = d.trips.find((x) => x.id === trip.id);
        const departure = target.edit.departure ?? target.departure;
        let value = null;
        if (min !== null) {
          value = isoAt(isoDate(departure), min);
          if (side === "arrival" && value <= departure) value = isoAt(isoDate(departure), min + 1440);
        }
        setTripField(target, side, value, target[side]);
      }),
  });
}

// ---------------------------------------------------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------------------------------------------------

/** "CT-22-XDD · VITO · 8 locuri": the car as the API describes it (model and seats are what tell a category D vehicle). */
function carText(trip) {
  const parts = [trip.car, trip.carModel, trip.carSeats > 0 ? `${trip.carSeats} locuri` : ""];
  return parts.filter(Boolean).join(" · ");
}

function tripRow(trip, edit) {
  const removed = trip.edit.removed;
  const counted = trip.eff.countsHours && trip.eff.minutes > 0;
  const regular = trip.eff.regular;
  const cells = [
    h("td", { class: "kind" }, h("span", { class: `badge ${trip.kind}` }, KIND_LABELS[trip.kind] ?? trip.kind), trip.manual ? h("span", { class: "muted", title: "Adăugată manual" }, " ✎") : null),
    h("td", {}, timeInput(trip, "departure", edit)),
    h("td", {}, timeInput(trip, "arrival", edit)),
    h("td", {}, placeInput(trip, "from", edit)),
    h("td", {}, placeInput(trip, "to", edit)),
    h("td", { class: `calc ${counted ? "strong" : ""}`, title: counted ? "" : trip.eff.arrival ? "" : "Sosire necunoscută: durata nu se numără" }, counted ? duration(trip.eff.minutes) : "–"),
    h("td", { class: "money" }, trip.eff.excursion ? h("small", { class: "muted", title: trip.eff.priceNote }, "tahograf") : regular ? lei(trip.eff.price) : priceInput(trip, edit)),
    h("td", { class: "actions" },
      removed
        ? h("button", { class: "icon-btn", title: "Pune cursa la loc", onclick: () => edit((d) => (d.trips.find((x) => x.id === trip.id).edit.removed = false)) }, "↺")
        : h("button", { class: "icon-btn", title: trip.manual ? "Șterge cursa adăugată" : "Scoate cursa din pontaj", onclick: () => edit((d) => {
            if (trip.manual) d.trips.splice(d.trips.findIndex((x) => x.id === trip.id), 1);
            else d.trips.find((x) => x.id === trip.id).edit.removed = true;
          }) }, "✕"),
      trip.car ? h("small", { class: "muted car", title: "Mașina, modelul și locurile din API" }, carText(trip)) : null),
  ];
  return h("tr", { class: `trip-row ${removed ? "removed" : ""} ${trip.eff.excursion ? "excursion" : ""} ${trip.edit.departure != null || trip.edit.arrival != null || trip.edit.from != null || trip.edit.to != null ? "edited" : ""}` }, cells);
}

/** A caption above a control, so nobody has to guess what an empty box is for. */
const labelled = (text, control, title) => h("label", { class: "ctl", title }, h("span", {}, text), control);

const pauseId = (pause) => `${pause.prev}>${pause.next}`;

/**
 * What the office decides about the time between two trips. The box with the hours and the two boxes taken from the
 * driver's sheet appear only when they mean something for the chosen state.
 */
function pauseControls(driver, pause, edit) {
  const decide = (patch) => edit((d) => setPause(d, pause, patch));
  const state = pause.state;
  const select = h("select", { class: `pause-state ${state !== "auto" ? "manual" : ""}`, "data-key": `state:${pause.prev}`, onchange: (e) => decide({ state: e.target.value }) },
    PAUSE_STATES.map(([value, label]) => h("option", { value, selected: value === state }, label)));

  const hours = bound({
    className: "pause-hours",
    dataset: { key: `hours:${pause.prev}` },
    placeholder: "ex. 4:30",
    get: () => pause,
    format: (p) => (p.hoursOverride != null ? hoursText(p.hoursOverride) : ""),
    parse: (text) => parseHours(text, 999),
    classes: (p) => (p.hoursOverride != null ? "manual" : ""),
    title: "Ore de staționare scrise de tine (ex. 4, 4:30, 30). Gol = se numără după alegerea de mai sus.",
    commit: (value) => decide({ hours: value }),
  });
  const when = (field, placeholder) =>
    bound({
      className: "pause-when",
      dataset: { key: `${field}:${pause.prev}` },
      placeholder,
      get: () => pause,
      format: (p) => formatWhen(p[field], p.start),
      parse: (text) => parseWhen(text, field === "back" ? pause.left ?? pause.start : pause.start),
      classes: (p) => (p[field] ? "manual" : ""),
      title: 'Ora de pe foaia șoferului: "11:00" sau, dacă e altă zi, "07.10 11:00".',
      commit: (value) => decide({ [field]: value }),
    });
  const note = bound({
    className: "pause-note",
    dataset: { key: `note:${pause.prev}` },
    placeholder: "ce scrie pe foaia șoferului",
    get: () => pause,
    format: (p) => p.note,
    parse: (text) => text.trim(),
    commit: (value) => decide({ note: value }),
  });

  const id = pauseId(pause);
  const open = opened.has(id);
  const decided = state !== "auto";
  const fields = [labelled("Ce a fost?", select)];
  if (state === "passenger" || pause.left || pause.back) {
    fields.push(labelled("A plecat la", when("left", "ex. 11:00"), "Ora la care a plecat, după foaia șoferului"));
    fields.push(labelled("S-a întors la", when("back", "ex. 17:00"), "Ora la care s-a întors, după foaia șoferului (doar pentru evidență)"));
  }
  // Hours and note are always there once the controls are open; before that a button opens them.
  // An excursion counts nothing here, so hours typed for it would mean nothing.
  if (state !== "excursion" && (decided || open || pause.hoursOverride != null)) fields.push(labelled("Ore de staționare", hours));
  if (decided || open || pause.note) fields.push(labelled("Notă", note));
  if (!decided && !open && pause.hoursOverride == null && !pause.note) {
    fields.push(h("button", { class: "btn small ghost", title: "Scrie orele singur sau adaugă o notă", onclick: () => { opened.add(id); edit(() => {}); } }, "✎ Ore sau notă"));
  }
  return h("div", { class: "pause-ctl" }, h("div", { class: "pause-fields" }, fields), h("p", { class: "pause-hint" }, pauseHint(pause)));
}

/** Where the driver is while he waits: "la Constanta" or "Otopeni → Constanta" when he comes back some other way. */
const pausePlace = (pause) => (pause.placeFrom === pause.placeTo ? `la ${pause.placeFrom}` : `${pause.placeFrom} → ${pause.placeTo}`);

/** A pause that needs a look or was already decided shows its controls; a routine wait shows a single line. */
function needsControls(pause) {
  const decided = pause.state !== "auto" || pause.hoursOverride != null || !!pause.note || !!pause.left || !!pause.back;
  return decided || pause.kind === "confirm" || pause.kind === "overlap" || pause.alerts.some((a) => a.level !== "info") || opened.has(pauseId(pause));
}

function pauseRows(driver, pause, edit) {
  const quiet = pause.kind === "rest" && pause.state === "auto" && !pause.alerts.length;
  const counted = pause.countedMin > 0;
  const overlap = pause.kind === "overlap";
  const controls = needsControls(pause);
  const change = h("button", { class: "btn small ghost", title: "Alege ce a fost, scrie orele sau o notă", onclick: () => { opened.add(pauseId(pause)); edit(() => {}); } }, "Schimbă");
  const rows = [
    h("tr", { class: `pause-row ${pause.kind} ${quiet ? "quiet" : ""} ${controls ? "with-controls" : ""}` },
      h("td", { class: "kind" }, h("span", { class: `badge pause ${pause.kind}` }, PAUSE_BADGES[pause.kind])),
      h("td", { colspan: 4, class: "pause-what" },
        h("b", {}, `${formatWhen(pause.start, pause.start)} → ${formatWhen(pause.end, pause.start)}`),
        h("span", { class: "pause-span" }, overlap ? `se suprapun cu ${spanText(-pause.gapMin)}` : `pauză de ${spanText(pause.gapMin)}`),
        h("span", { class: "muted" }, pausePlace(pause))),
      h("td", { class: `calc ${counted ? "strong" : ""}`, title: counted ? "Cât se numără din pauză, în ore" : "Nu se numără în ore" }, counted ? duration(pause.countedMin) : "–"),
      h("td", { class: "money calc" }, pause.extraPay > 0 ? lei(pause.extraPay) : ""),
      h("td", { class: "actions" }, controls ? null : change)),
  ];
  if (controls) rows.push(h("tr", { class: `pause-ctl-row ${pause.kind} ${quiet ? "quiet" : ""}` }, h("td"), h("td", { colspan: 7, class: "pause-cell" }, pauseControls(driver, pause, edit))));
  if (pause.alerts.length) rows.push(h("tr", { class: "alert-row" }, h("td"), h("td", { colspan: 7 }, alertList(pause.alerts))));
  return rows;
}

/** One figure of the day header with its caption under it. */
const dayStat = (label, value, tone = "") => h("div", { class: `day-stat ${tone}` }, h("b", {}, value), h("small", {}, label));

function dayRows(driver, sheet, date, trips, edit) {
  const row = driver.days.find((d) => d.date === date);
  const info = sheet.days.find((d) => `${sheet.year}-${String(sheet.month).padStart(2, "0")}-${String(d.day).padStart(2, "0")}` === date);
  const outside = !info;
  const excursionOnly = trips.length > 0 && trips.every((t) => t.eff.excursion);
  const excluded = !!row?.excluded;
  const head = h("tr", { class: `day-head ${info?.weekend ? "off" : ""} ${info?.holiday ? "holiday" : ""} ${excluded ? "excluded" : ""}` },
    h("td", { colspan: 8 },
      h("div", { class: "day-line" },
        h("div", { class: "day-title" },
          h("b", {}, weekdayName(date)),
          h("span", { class: "day-date" }, shortDate(date)),
          info?.holiday ? h("span", { class: "pill holiday" }, info.holiday) : null,
          outside ? h("span", { class: "pill" }, "în afara lunii: nu se numără") : null,
          excursionOnly ? h("span", { class: "pill excursion" }, "excursie · se pontează din tahograf") : null,
          excluded ? h("span", { class: "pill" }, "scoasă din pontaj") : null),
        row
          ? h("div", { class: "day-stats" },
              dayStat("Curse regulate", row.legs),
              dayStat("Ore curse", duration(row.tripMin)),
              dayStat("Staționare", duration(row.stationMin)),
              dayStat("Total ziua", duration(row.totalMin), "total"),
              dayStat("Estimare", `~${lei(row.payTotal)} lei`, "muted"))
          : null,
        row ? h("button", { class: "btn small ghost", onclick: () => edit((d) => toggleDay(d, date)) }, excluded ? "Pune ziua la loc" : "Scoate ziua din pontaj") : null),
      row?.alerts.length ? alertList(row.alerts) : null));
  const body = [];
  const pauseOf = new Map(driver.pauses.map((p) => [p.prev, p]));
  for (const trip of trips) {
    body.push(tripRow(trip, edit));
    const pause = pauseOf.get(trip.id);
    if (pause) body.push(...pauseRows(driver, pause, edit));
  }
  const group = [h("tr", { class: "day-gap" }, h("td", { colspan: 8 })), head, ...body];
  if (excluded) group.forEach((tr) => tr.classList.add("day-excluded"));
  return group;
}

// ---------------------------------------------------------------------------------------------------------------------
// Adding a trip
// ---------------------------------------------------------------------------------------------------------------------

/** Asks for a trip the API does not have (typically the passenger leg back). Resolves with the trip, or null. */
export async function askNewTrip(driver, sheet) {
  const last = driver.trips.filter((t) => !t.edit.removed).at(-1);
  const month = `${sheet.year}-${String(sheet.month).padStart(2, "0")}`;
  const field = (label, control, hint) => h("label", { class: "field" }, h("span", {}, label), control, hint ? h("small", {}, hint) : null);
  const date = h("input", { type: "date", value: last ? isoDate(last.eff.departure) : `${month}-01` });
  const kind = h("select", {}, [["passenger", "Pasager (drumul înapoi, cursă regulată)"], ["eco", "Eco"], ["private", "Privată"], ["vip", "VIP"], ["bus", "Autocar"]].map(([value, label]) => h("option", { value }, label)));
  const from = h("input", { list: "catb-places", value: last?.eff.to ?? "" });
  const to = h("input", { list: "catb-places", value: last?.eff.from ?? "" });
  const departure = h("input", { placeholder: "ex. 11:00", maxlength: 5 });
  const arrival = h("input", { placeholder: "ex. 14:00 (gol = necunoscută)", maxlength: 5 });
  const price = h("input", { placeholder: "doar pentru cursele care nu sunt regulate" });
  const error = h("p", { class: "form-error", hidden: true });
  const body = h("div", { class: "form" }, field("Data", date), field("Tip", kind), field("Din", from), field("Spre", to), field("Plecare", departure), field("Sosire", arrival, "Dacă o completezi, durata drumului intră în ore."), field("Preț (lei)", price), error);
  return showDialog({
    title: `Cursă nouă pentru ${driver.name}`,
    body,
    buttons: [
      { label: "Renunță", value: null },
      {
        label: "Adaugă cursa",
        kind: "primary",
        action: () => {
          const out = (message) => {
            error.textContent = message;
            error.hidden = false;
            return undefined;
          };
          const dep = parseHm(departure.value);
          const arr = parseHm(arrival.value);
          const cost = price.value.trim() === "" ? null : Number(price.value.trim().replace(",", "."));
          if (!date.value) return out("Alege data.");
          if (dep === null || Number.isNaN(dep)) return out("Scrie ora plecării, de exemplu 11:00.");
          if (Number.isNaN(arr)) return out("Ora sosirii nu este validă.");
          if (!from.value.trim() || !to.value.trim()) return out("Completează locul de plecare și cel de sosire.");
          if (cost !== null && !(Number.isFinite(cost) && cost >= 0)) return out("Prețul nu este valid.");
          const start = isoAt(date.value, dep);
          let end = arr === null ? null : isoAt(date.value, arr);
          if (end && end <= start) end = isoAt(date.value, arr + 1440);
          return {
            id: `manual-${Date.now().toString(36)}`,
            kind: kind.value,
            label: "adăugată manual",
            from: from.value.trim(),
            to: to.value.trim(),
            departure: start,
            arrival: end,
            returnAt: null,
            car: "",
            passengers: 0,
            manual: true,
            edit: { departure: null, arrival: null, from: null, to: null, price: cost, removed: false },
          };
        },
      },
    ],
  });
}

// ---------------------------------------------------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------------------------------------------------

/**
 * Draws a driver into `container`.
 * `ctx`: { driver(), sheet(), edit(mutate(driver)) → Promise, addTrip() → Promise }.
 */
export function renderCatbDriver(container, ctx) {
  const driver = ctx.driver();
  const sheet = ctx.sheet();
  const edit = ctx.edit;
  const t = driver.totals;
  const excursions = driver.trips.filter((trip) => trip.eff.excursion && !trip.edit.removed).length;

  const proposals = driver.proposals.length
    ? h("div", { class: "proposals" },
        h("div", { class: "proposals-head" },
          h("b", {}, "Cum se poate încadra în plafon"),
          h("button", { class: "btn small", onclick: () => edit((d) => d.proposals.forEach((p) => applyProposal(d, p))) }, "Aplică toate"),
          h("small", { class: "muted" }, "Doar sugestii: tu hotărăști ce se scoate.")),
        h("ul", {}, driver.proposals.map((p) =>
          h("li", {}, h("span", {}, p.text, p.pay >= 0.5 ? h("small", { class: "muted" }, ` (~${lei(p.pay)} lei din estimare)`) : null), h("button", { class: "btn small", onclick: () => edit((d) => applyProposal(d, p)) }, "Aplică")))))
    : null;

  const head = h("div", { class: "driver-head" },
    h("div", { class: "driver-title" },
      h("h3", {}, driver.name),
      h("div", { class: "driver-actions" },
        h("button", { class: "btn small", onclick: async () => { const trip = await ctx.addTrip(); if (trip) edit((d) => d.trips.push(trip)); } }, "＋ Adaugă cursă"))),
    h("div", { class: "chips" },
      chip("Zile", t.days),
      chip("Curse regulate", t.legs),
      chip("Ore curse", duration(t.tripMin)),
      chip("Ore staționare", duration(t.stationMin), { title: "Dedusă: plecarea cursei următoare minus sosirea celei dinainte" }),
      excursions ? chip("Curse de excursie (tahograf)", excursions, { title: "Cursele în afara județului fără tarif și cele din pauzele marcate „Excursie” ies din Categoria B: se pontează din tahograf, nu intră în ore și nu se plătesc aici." }) : null,
      chip("Total ore", duration(t.totalMin), { tone: t.overMin > 0 ? "bad" : "" }),
      chip("Plafon", duration(sheet.capMin), { title: `${sheet.weekdays} zile luni–vineri × 8 ore (sărbătorile incluse)` }),
      t.overMin > 0 ? chip("Depășire", duration(t.overMin), { tone: "bad" }) : null,
      chip("Estimare plată", `~${lei(t.payTotal)} lei`, { title: `Curse ${lei(t.payLegs)} · exterioare ${lei(t.payExterior)} · staționare ${lei(t.payStation)}. Orientativ.` })),
    driver.alerts.length ? alertList(driver.alerts) : null,
    proposals);

  const byDate = new Map();
  for (const trip of driver.trips) {
    const date = isoDate(trip.eff.departure ?? trip.departure);
    if (!byDate.has(date)) byDate.set(date, []);
    byDate.get(date).push(trip);
  }
  const body = h("tbody");
  for (const date of [...byDate.keys()].sort()) body.append(...dayRows(driver, sheet, date, byDate.get(date), edit));

  const table = h("table", { class: "data-table catb-table" },
    h("thead", {}, h("tr", {}, ["Tip", "Plecare", "Sosire", "Din", "Spre", "Ore", "Lei", ""].map((text) => h("th", {}, text)))),
    body);
  container.replaceChildren(
    head,
    driver.trips.length ? h("div", { class: "table-scroll", id: "catb-scroll" }, table) : h("div", { class: "empty-note" }, "Șoferul nu are curse în luna aceasta."));
}
