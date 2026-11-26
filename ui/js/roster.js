// Birou, Garaj, Oraș: the staff whose hours follow a plan (8 hours on the working days, or 12 hours two days on and two days off).
// Nothing is read from a file or an API: the hours of the month come from the plans and go straight into the Excel file; a tab
// only shows them, so a day can be changed when something differs. The people and their plans live in the settings (`roster`);
// the days the office changed are saved per group and month (see `roster_save`).
import { api } from "./api.js";
import { $, h, field, toast, busy } from "./dom.js";
import { showDialog } from "./dialog.js";
import { renderGrid, refreshGrid, renderDetails } from "./grid.js";
import { chip } from "./catb_driver.js";
import { askLeaves } from "./leaves.js";
import { monthFields } from "./month_bar.js";
import { askShifts, canMoveShifts, moveText } from "./shifts.js";
import { hhmm, hoursText, MONTHS, parseHm } from "./time.js";

const SAVE_DELAY_MS = 1200;

/** The three tabs; `id` is what the backend calls the group. */
export const GROUPS = {
  office: { id: "office", label: "Birou", tagline: "Programul de la birou, din plan: 8 ore luni–vineri sau 12 ore, 2 zile lucrate și 2 libere" },
  garage: { id: "garage", label: "Garaj", tagline: "Programul mecanicilor de la garaj, din plan: 8 ore luni–vineri" },
  city: { id: "city", label: "Oraș", tagline: "Programul șoferilor de pe oraș, din plan: 12 ore, 2 zile lucrate și 2 libere" },
};

const SHIFT = 12 * 60;

/** The plan a new person starts with: 8 hours on weekdays in the office and the garage, 12 hours in cycles in the city. */
function defaultPlan(group, anchor) {
  if (group === "city") return rotation(anchor, [{ start: 9 * 60, end: 9 * 60 + SHIFT }, { start: 9 * 60, end: 9 * 60 + SHIFT }]);
  return { kind: "weekdays", start: 8 * 60, end: 16 * 60 };
}

const rotation = (anchor, on) => ({ kind: "rotation", anchor, on, off: 2 });

const isoDay = (date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

const span = (interval) => `${hhmm(interval.start)}–${hhmm(interval.end)}`;

/** A plan in words, for the list of people. */
function planText(plan) {
  if (plan.kind === "manual") return "fără program: orele se scriu de mână";
  if (plan.kind === "weekdays") return `${span(plan)}, luni–vineri`;
  const hours = plan.on.length ? (plan.on[0].end - plan.on[0].start) / 60 : 0;
  return `${hoursText(hours)} ore, ${plan.on.length} zile lucrate / ${plan.off} libere, de la ${hhmm(plan.on[0]?.start ?? 0)}`;
}

/** The way two names are told apart: capitals, no diacritics, single spaces (as the backend writes them). */
const nameKey = (name) => name.normalize("NFD").replace(/[̀-ͯ]/g, "").toUpperCase().replace(/\s+/g, " ").trim();

/** "2026-11-16" → "16.11.2026". */
const dateText = (iso) => iso.split("-").reverse().join(".");

/**
 * Is the person employed on some day of the month? `startsOn` is the first day he works and `stopsOn` the first day he no longer
 * works (both optional, ISO dates, which compare as text). Only those people are in the pontaj of the month.
 */
function employedIn(member, year, month) {
  const first = isoDay(new Date(year, month - 1, 1));
  const last = isoDay(new Date(year, month, 0));
  return (!member.startsOn || member.startsOn <= last) && (!member.stopsOn || member.stopsOn > first);
}

/**
 * Has the month the person stops working in already passed (by the calendar)? Then he is gone from the lists: his past months keep him
 * (he worked in them), but he is no longer offered among the people who do not work, and the dismissal can no longer be taken back.
 */
const goneForGood = (member) => Boolean(member.stopsOn) && member.stopsOn.slice(0, 7) < isoDay(new Date()).slice(0, 7);

/** Dismissed, and the month he stops in has not passed yet: the dismissal can still be taken back. */
const canBePutBack = (member) => Boolean(member.stopsOn) && !goneForGood(member);

/** What the dates of a person say, for the list of people. */
const employmentText = (member) =>
  [member.startsOn ? `lucrează din ${dateText(member.startsOn)}` : "", member.stopsOn ? `nu mai lucrează din ${dateText(member.stopsOn)}` : ""].filter(Boolean).join(" · ");

/** An end time at or before the start is the next day's (a shift over midnight). */
const endAfter = (start, end) => (end <= start ? end + 1440 : end);

/**
 * The form of one person. `existing` is null for a new one. Resolves with the person (as the settings keep him) or null.
 * `others` are the names already in the list, `anchor` the day a new cycle starts on (for a new person also the day he starts working).
 */
function askPerson({ existing, group, others, anchor }) {
  const person = existing ?? { name: "", function: "", group, plan: defaultPlan(group, anchor), note: "", moves: [] };
  const plan = person.plan;
  const kinds = { weekdays: "8 ore, luni–vineri", rotation: "12 ore, 2 zile lucrate și 2 libere", manual: "Fără program (orele se scriu de mână)" };
  const shifts = plan.kind === "rotation" ? plan.on : [{ start: 9 * 60, end: 21 * 60 }, { start: 9 * 60, end: 21 * 60 }];
  const weekday = plan.kind === "weekdays" ? plan : { start: 8 * 60, end: 16 * 60 };

  const text = (value, attrs = {}) => h("input", { type: "text", value, spellcheck: "false", ...attrs });
  const name = text(person.name, { maxlength: 60 });
  const job = text(person.function, { maxlength: 60 });
  const where = h("select", {}, Object.values(GROUPS).map((g) => h("option", { value: g.id, selected: g.id === person.group }, g.label)));
  const kind = h("select", { onchange: () => showKind() }, Object.entries(kinds).map(([value, label]) => h("option", { value, selected: value === plan.kind }, label)));
  const from = text(hhmm(weekday.start), { size: 6 });
  const to = text(hhmm(weekday.end), { size: 6 });
  const first = h("input", { type: "date", value: plan.kind === "rotation" ? plan.anchor : anchor });
  // The dates he is employed between: a new person must have the day he starts, somebody already in the list may have none (always).
  const starts = h("input", { type: "date", value: person.startsOn ?? (existing ? "" : anchor) });
  const stops = h("input", { type: "date", value: person.stopsOn ?? "" });
  // The first day of a new cycle follows the day he starts, until it is written by hand.
  let anchorWritten = false;
  first.addEventListener("input", () => (anchorWritten = true));
  starts.addEventListener("change", () => {
    if (!existing && !anchorWritten && starts.value) first.value = starts.value;
  });
  const days = shifts.slice(0, 2).map((shift) => ({ from: text(hhmm(shift.start), { size: 6 }), to: text(hhmm(shift.end), { size: 6 }) }));
  const error = h("p", { class: "form-error", role: "alert" });

  const weekdayBox = h("div", {}, h("div", { class: "form-row" }, field("De la", from), field("Până la", to)), h("small", { class: "muted" }, "Zilele libere și sărbătorile legale rămân goale."));
  const rotationBox = h("div", {},
    field("Prima zi lucrată", first, "Ciclul merge mai departe singur, lună după lună, de la această zi."),
    ...days.map((day, i) => h("div", { class: "form-row" }, h("b", { class: "form-day" }, `Ziua ${i + 1}`), field("De la", day.from), field("Până la", day.to))));
  const manualBox = h("small", { class: "muted" }, "Pontajul lui rămâne la zero: nu se pune nimic singur, orele se scriu de mână în zilele în care a lucrat.");
  const showKind = () => {
    weekdayBox.hidden = kind.value !== "weekdays";
    rotationBox.hidden = kind.value !== "rotation";
    manualBox.hidden = kind.value !== "manual";
  };
  showKind();

  const read = () => {
    const fail = (message) => {
      error.textContent = message;
      return undefined;
    };
    const cleaned = name.value.replace(/\s+/g, " ").trim().toUpperCase();
    if (!cleaned) return fail("Scrie numele.");
    if (others.some((other) => nameKey(other) === nameKey(cleaned))) return fail("Există deja o persoană cu acest nume.");
    const [startsOn, stopsOn] = [starts.value, stops.value];
    if (!existing && !startsOn) return fail("Alege ziua de la care lucrează.");
    if (startsOn && stopsOn && stopsOn <= startsOn) return fail("„Nu mai lucrează din” trebuie să fie după „Lucrează de la”.");
    const time = (input) => parseHm(input.value);
    let next;
    if (kind.value === "manual") {
      next = { kind: "manual" };
    } else if (kind.value === "weekdays") {
      const [start, end] = [time(from), time(to)];
      if (!Number.isInteger(start) || !Number.isInteger(end)) return fail("Scrie orele ca 08:00 și 16:00.");
      next = { kind: "weekdays", start, end: endAfter(start, end) };
    } else {
      if (!/^\d{4}-\d{2}-\d{2}$/.test(first.value)) return fail("Alege prima zi lucrată.");
      const on = [];
      for (const day of days) {
        const [start, end] = [time(day.from), time(day.to)];
        if (!Number.isInteger(start) || !Number.isInteger(end)) return fail("Scrie orele ca 09:00 și 21:00.");
        on.push({ start, end: endAfter(start, end) });
      }
      next = rotation(first.value, on);
    }
    return { name: cleaned, function: job.value.trim(), group: where.value, plan: next, note: "", moves: person.moves ?? [], ...(startsOn ? { startsOn } : {}), ...(stopsOn ? { stopsOn } : {}) };
  };

  return showDialog({
    title: existing ? `Modifică · ${existing.name}` : "Persoană nouă",
    body: h("div", {},
      person.note ? h("p", { class: "form-note" }, `⚠ ${person.note}`) : null,
      field("Nume", name, "Cum apare în pontaj, de exemplu POPESCU ION."),
      field("Funcția", job),
      field("Grupa", where),
      field("Lucrează de la", starts, existing ? "Prima zi lucrată. Gol = a lucrat dintotdeauna. Înainte de această zi nu apare în pontaj." : "Prima zi lucrată. Înainte de ea nu apare în pontaj; din această zi are programul de mai jos."),
      existing ? field("Nu mai lucrează din", stops, "Prima zi în care nu mai lucrează (ultima zi lucrată este cea dinainte). Gol = lucrează în continuare. De la această zi nu mai apare în pontaj.") : null,
      field("Program", kind),
      weekdayBox,
      rotationBox,
      manualBox,
      error),
    buttons: [{ label: "Anulează", value: null }, { label: existing ? "Salvează" : "Adaugă", kind: "primary", action: read }],
  });
}

/**
 * The dismissal of a person: the first day he no longer works. Resolves with that day (an ISO date), "delete" (take him out of every
 * month, for somebody added by mistake) or null.
 */
function askDismissal({ member, day }) {
  const date = h("input", { type: "date", value: member.stopsOn ?? day });
  const error = h("p", { class: "form-error", role: "alert" });
  return showDialog({
    title: `Dezangajare · ${member.name}`,
    body: h("div", {},
      field("Nu mai lucrează din", date, "În ziua aleasă și după ea nu mai are ore și nu mai apare în pontaj; ultima zi lucrată este cea dinainte. Lunile dinainte rămân cum sunt. Până trece luna în care pleacă îl poți pune la loc; după aceea dispare din liste."),
      error),
    buttons: [
      { label: "Anulează", value: null },
      { label: "Șterge din toate lunile", value: "delete" },
      {
        label: "Dezangajează",
        kind: "primary",
        action: () => {
          if (!/^\d{4}-\d{2}-\d{2}$/.test(date.value)) return void (error.textContent = "Alege ziua din care nu mai lucrează.");
          if (member.startsOn && date.value <= member.startsOn) return void (error.textContent = `A început să lucreze pe ${dateText(member.startsOn)}: alege o zi după aceea.`);
          return date.value;
        },
      },
    ],
  });
}

/**
 * @param {{ getSettings: () => object, saveSettings: (settings: object) => Promise<void> }} host
 * Returns what the page needs: show a group, hide the tab, save, react to changed settings, say which month is open.
 */
export function createRoster({ getSettings, saveSettings }) {
  const today = new Date();
  const state = { group: "office", year: today.getFullYear(), month: today.getMonth() + 1, sheet: null, selected: null, dirty: false };
  let queue = Promise.resolve();
  let saveTimer = null;

  const fail = (err) => toast(String(err), { kind: "error", timeout: 0 });
  // Only real people: a stray entry in the list (anything that is not a person) must never reach the screen or the backend.
  const staff = () => (getSettings().roster ?? []).filter((member) => member && typeof member === "object");
  const people = () => staff().filter((member) => member.group === state.group).sort((a, b) => a.name.localeCompare(b.name));
  /** The people of the group who are employed on some day of the open month (the others are not in its pontaj). */
  const working = () => people().filter((member) => employedIn(member, state.year, state.month));

  // ---- saving ----

  function markDirty() {
    state.dirty = true;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(flush, SAVE_DELAY_MS);
  }

  async function flush() {
    clearTimeout(saveTimer);
    if (!state.dirty || !state.sheet) return;
    state.dirty = false;
    try {
      await api.rosterSave(state.group, state.sheet);
    } catch (err) {
      state.dirty = true;
      toast(`Modificările nu au putut fi salvate: ${err}`, { kind: "warn" });
    }
  }

  // ---- editing the days ----

  /** Applies `mutate(sheet)`, recalculates in Rust and updates the tab. Edits run one after another. */
  function edit(mutate) {
    queue = queue
      .then(async () => {
        mutate(state.sheet);
        state.sheet = await api.recalculate(state.sheet, getSettings());
        markDirty();
        refreshGrid($("#roster-grid"), state.sheet);
        renderSummary();
        if (state.selected) renderDetails($("#roster-details"), state.sheet, ...state.selected);
      })
      .catch(fail);
    return queue;
  }

  // ---- the list of people (in the settings) ----

  /** Puts a new list of people in the settings and brings the open month in line with it. `regenerate`: the person whose days follow the new plan. */
  async function setStaff(roster, regenerate = null) {
    await queue;
    await flush();
    await saveSettings({ ...getSettings(), roster });
    state.sheet = await api.rosterOpen(state.group, state.year, state.month, getSettings());
    if (regenerate) {
      state.sheet = await api.rosterRegenerate(state.sheet, regenerate, getSettings());
      markDirty();
    }
    renderAll();
  }

  /** The day a new person starts on unless the office says otherwise: today when the month on the screen is this month, else its first day. */
  const defaultStart = () => (state.year === today.getFullYear() && state.month === today.getMonth() + 1 ? isoDay(today) : isoDay(new Date(state.year, state.month - 1, 1)));

  async function addPerson() {
    const person = await askPerson({ existing: null, group: state.group, others: staff().map((m) => m.name), anchor: defaultStart() });
    if (person) await setStaff([...staff(), person], person.name).catch(fail);
  }

  async function editPerson(member) {
    const others = staff().filter((m) => m !== member).map((m) => m.name);
    const person = await askPerson({ existing: member, group: member.group, others, anchor: isoDay(new Date(state.year, state.month - 1, 1)) });
    if (!person) return;
    const changed = JSON.stringify(person.plan) !== JSON.stringify(member.plan) || person.name !== member.name;
    await setStaff(staff().map((m) => (m === member ? person : m)), changed ? person.name : null).catch(fail);
  }

  /** Takes the person out of the list altogether: he is gone from every month (only for somebody added by mistake). */
  async function removePerson(member) {
    const at = staff().indexOf(member);
    await setStaff(staff().filter((m) => m !== member)).catch(fail);
    toast(`${member.name} a fost șters din lista de personal, din toate lunile.`, {
      kind: "ok",
      timeout: 12000,
      action: { label: "Anulează", run: () => setStaff([...staff().slice(0, at), member, ...staff().slice(at)]).catch(fail) },
    });
  }

  /** The person stops working on the day the office picks: the months before stay as they were, from that day on he is not in the pontaj. */
  async function dismissPerson(member) {
    const answer = await askDismissal({ member, day: isoDay(today) });
    if (!answer) return;
    if (answer === "delete") return removePerson(member);
    await setStaff(swap(member, { ...member, stopsOn: answer })).catch(fail);
    toast(`${member.name} nu mai lucrează din ${dateText(answer)}.`, {
      kind: "ok",
      timeout: 12000,
      action: { label: "Anulează", run: () => setStaff(swap(member, member)).catch(fail) },
    });
  }

  /** The list of people with `member` replaced by `next` (found by name, so it also works on the entry the settings now hold). */
  const swap = (member, next) => staff().map((m) => (m.group === member.group && nameKey(m.name) === nameKey(member.name) ? next : m));

  /** The dismissal is taken back: no day of leaving any more, he works again. Possible until the month he stops in has passed. */
  async function putBack(member) {
    const next = { ...member };
    delete next.stopsOn;
    await setStaff(swap(member, next)).catch(fail);
    toast(`${member.name} a fost pus la loc: lucrează din nou.`, { kind: "ok", timeout: 6000 });
  }

  /** The permanent way back of a dismissal (the button of the toast goes away after a few seconds); nothing once the month has passed. */
  const putBackButton = (member, extra = "") =>
    canBePutBack(member)
      ? h("button", { class: `btn small ${extra}`.trim(), type: "button", title: `${member.name} lucrează din nou, fără ziua de plecare`, onclick: () => putBack(member) }, "↶ Anulează dezangajarea")
      : null;

  /** One person's days put back as the plan says, dropping what was typed on them. */
  async function resetPerson(member) {
    if (!confirm(`Zilele lui ${member.name} din ${MONTHS[state.month - 1]} ${state.year} revin la program; ce ai scris pe ele se șterge. Continui?`)) return;
    queue = queue
      .then(async () => {
        state.sheet = await api.rosterRegenerate(state.sheet, member.name, getSettings());
        markDirty();
        renderAll();
      })
      .catch(fail);
    await queue;
  }

  // ---- the shift cycle moved by some days from a day on (in the settings, on the person) ----

  const personNamed = (name) => staff().find((m) => nameKey(m.name) === nameKey(name) && m.group === state.group);

  /**
   * Saves the person with the moves `change` makes, then brings the saved months in line: the days nobody typed on follow the new
   * cycle from the day of `moved`. The open month is read again from disk.
   */
  async function setMoves(name, moved, change) {
    await queue;
    await flush();
    const before = personNamed(name);
    if (!before) return;
    const after = { ...before, moves: change(before.moves ?? []) };
    await saveSettings({ ...getSettings(), roster: staff().map((m) => (m === before ? after : m)) });
    await api.rosterMoveCycle(before, moved, getSettings());
    state.sheet = await api.rosterOpen(state.group, state.year, state.month, getSettings());
    renderAll();
  }

  function openShifts(member) {
    return askShifts({
      title: `Mută tura · ${member.name}`,
      getPerson: () => personNamed(member.name) ?? member,
      add: (move) => setMoves(member.name, move, (all) => [...all, move]),
      remove: (move) => setMoves(member.name, move, (all) => {
        const at = all.findIndex((m) => m.from === move.from && m.days === move.days);
        return all.filter((_, i) => i !== at);
      }),
    });
  }

  // ---- leaves written down in advance (in the settings, for every month) ----

  const plansOfGroup = () => (getSettings().leaves ?? []).filter((plan) => plan.group === state.group);

  /** Saves the new list of plans and opens the month again from disk: the plans put their days on it as it is opened. */
  async function setLeaves(change) {
    await queue;
    await flush();
    await saveSettings({ ...getSettings(), leaves: change(getSettings().leaves ?? []) });
    state.sheet = await api.rosterOpen(state.group, state.year, state.month, getSettings());
    renderAll();
  }

  const addLeave = (plan) => setLeaves((all) => [...all, plan]);

  /** Takes the plan back out of the saved months first (they hold its days), then out of the list. */
  async function removeLeave(plan) {
    await queue;
    await flush();
    await api.leavesStrip(plan, getSettings());
    await setLeaves((all) => all.filter((p) => p.id !== plan.id));
  }

  function openLeaves() {
    return askLeaves({
      title: `Concedii · ${GROUPS[state.group].label}`,
      group: state.group,
      people: people().map((member) => ({ name: member.name })),
      getPlans: plansOfGroup,
      add: addLeave,
      remove: removeLeave,
      note: "Concediul se pune pe toate zilele alese, și pe cele libere (sâmbătă, duminică, sărbători, zilele libere ale turei); debifează „Și pe zilele libere” ca să se pună doar pe zilele în care persoana ar lucra. Poți scrie și concedii pentru luni care nu au început încă.",
    });
  }

  // ---- drawing ----

  function renderBar() {
    $("#roster-bar").replaceChildren(
      ...monthFields(state, openMonth, today),
      h("button", { class: "btn", type: "button", title: "Scrie concedii pentru persoanele din această listă, și în avans, pentru luni care nu au început", onclick: openLeaves }, "🌴 Concedii"),
      h("span", { class: "muted" }, "Orele vin din program și intră direct în Excel. Scrie într-o zi doar dacă ceva diferă."));
  }

  function renderSummary() {
    const sheet = state.sheet;
    const box = $("#roster-summary");
    const title = h("div", { class: "title" }, h("h2", {}, `${MONTHS[state.month - 1]} ${state.year}`), h("span", {}, GROUPS[state.group].label));
    if (!sheet) {
      box.replaceChildren(title);
      return;
    }
    const sum = (key) => sheet.employees.reduce((acc, e) => acc + e.totals[key], 0);
    box.replaceChildren(
      title,
      chip("Persoane", sheet.employees.length),
      chip("Normă", `${sheet.normHours} ore`, { title: `${sheet.workingDays} zile lucrătoare × 8 ore` }),
      chip("Total ore lucrate", hoursText(sum("worked"))),
      chip("Ore de noapte", hoursText(sum("night"))));
  }

  function renderPeople() {
    const list = working();
    // Somebody whose month of leaving has passed is not listed any more.
    const away = people().filter((member) => !list.includes(member) && !goneForGood(member));
    $("#roster-people").replaceChildren(
      h("h3", {}, `${GROUPS[state.group].label} (${list.length})`),
      h("button", { class: "btn primary", type: "button", onclick: addPerson }, "＋ Adaugă persoană"),
      ...list.map((member) =>
        h("div", { class: `driver-row has-reset ${canMoveShifts(member) ? "has-shift" : ""}` },
          h("button", { class: "driver", title: "Schimbă programul, numele sau datele de angajare", onclick: () => editPerson(member) },
            h("b", {}, member.name),
            h("small", {}, member.function ? `${member.function} · ` : "", planText(member.plan)),
            employmentText(member) ? h("small", { class: "employment" }, employmentText(member)) : null,
            member.note ? h("small", { class: "flag warn" }, `⚠ ${member.note}`) : null,
            canMoveShifts(member) && member.moves?.length ? h("small", { class: "shift-moves", title: member.moves.map(moveText).join("\n") }, `⇄ Tura mutată: ${member.moves.length === 1 ? "o dată" : `de ${member.moves.length} ori`}`) : null),
          putBackButton(member, "put-back"),
          canMoveShifts(member) ? h("button", { class: "driver-shift", type: "button", title: `Mută tura lui ${member.name} cu una sau mai multe zile, de la o zi încolo; ce urmează se calculează singur`, "aria-label": `Mută tura lui ${member.name}`, onclick: () => openShifts(member) }, "⇄") : null,
          h("button", { class: "driver-reset", type: "button", title: `Pune zilele lui ${member.name} la loc, după program (șterge ce ai scris pe ele în luna asta)`, "aria-label": `Zilele lui ${member.name} după program`, onclick: () => resetPerson(member) }, "↺"),
          h("button", { class: "driver-remove", type: "button", title: `Dezangajează-l pe ${member.name}: alegi ziua din care nu mai lucrează`, "aria-label": `Dezangajează pe ${member.name}`, onclick: () => dismissPerson(member) }, "×"))),
      away.length ? renderAway(away) : null);
  }

  /** The people of the group who do not work in the open month (not hired yet, or already dismissed), each with the way to change his dates. */
  function renderAway(away) {
    return h("details", { class: "hidden-drivers" },
      h("summary", {}, `Nu lucrează în ${MONTHS[state.month - 1].toLowerCase()} (${away.length})`),
      h("p", { class: "muted" }, "Nu sunt în pontajul acestei luni; apar în lunile în care lucrează. Cine a plecat dispare de aici după ce trece luna în care pleacă; până atunci îl poți pune la loc."),
      ...away.map((member) => h("div", { class: "hidden-driver" },
        h("span", {}, member.name, h("small", { class: "employment" }, employmentText(member))),
        h("span", { class: "hidden-actions" },
          putBackButton(member),
          h("button", { class: "btn small", type: "button", title: "Schimbă datele de angajare", onclick: () => editPerson(member) }, "Datele")))));
  }

  function renderView() {
    const view = $("#roster-view");
    if (!state.sheet) {
      view.replaceChildren(h("div", { class: "empty-note" }, "Se deschide luna…"));
      return;
    }
    if (!state.sheet.employees.length) {
      const nobody = people().length ? `Nu lucrează nimeni din „${GROUPS[state.group].label}” în ${MONTHS[state.month - 1].toLowerCase()} ${state.year}.` : `Nu este nimeni în lista „${GROUPS[state.group].label}”.`;
      view.replaceChildren(h("div", { class: "empty-note" }, h("p", {}, nobody), h("button", { class: "btn primary", onclick: addPerson }, "＋ Adaugă persoană")));
      return;
    }
    const wrap = h("div", { class: "grid-wrap", id: "roster-grid" });
    const details = h("div", { class: "details", id: "roster-details" },
      h("span", { class: "muted" }, "Selectează o zi pentru detalii. Scrie un număr pentru ore, un cod (CO, CM, CFP…) pentru concediu sau absențe sau lasă gol pentru o zi liberă. Sub ore sunt ora începerii și ora terminării, ca în Excel: scrie-le (08:00) ca să schimbi tura unei zile. ↺ de lângă nume pune zilele la loc, după program."));
    view.replaceChildren(wrap, details);
    renderGrid(wrap, state.sheet, {
      heading: "Angajat",
      times: true,
      // The times typed under the hours come as a function of the day, so they are read from the day as it is when the edit runs.
      onEdit: (ei, di, patch) => edit((s) => {
        const day = s.employees[ei].days[di];
        Object.assign(day, typeof patch === "function" ? patch(day) : patch);
      }),
      onSelect: (ei, di) => {
        state.selected = [ei, di];
        renderDetails(details, state.sheet, ei, di);
      },
    });
  }

  function renderAll() {
    renderBar();
    renderSummary();
    renderPeople();
    renderView();
  }

  // ---- the month ----

  async function openMonth(year, month) {
    await queue;
    await flush();
    Object.assign(state, { year, month, sheet: null, selected: null });
    renderAll();
    busy("Se deschide luna…");
    try {
      state.sheet = await api.rosterOpen(state.group, year, month, getSettings());
    } catch (err) {
      fail(err);
    } finally {
      busy(null);
    }
    renderAll();
  }

  /** The rules changed (hours per day, night window…): the month is made again from what is saved. */
  async function settingsChanged() {
    if ($("#roster").hidden) return;
    await openMonth(state.year, state.month);
  }

  async function show(group) {
    $("#roster").hidden = false;
    await queue;
    await flush();
    state.group = group;
    await openMonth(state.year, state.month);
  }

  function hide() {
    $("#roster").hidden = true;
    flush();
  }

  window.addEventListener("pagehide", flush);
  document.addEventListener("visibilitychange", () => document.hidden && flush());

  /** Saves what is pending, so the Excel file (made from what is saved) has the last edit. */
  async function saved() {
    await queue;
    await flush();
  }

  return { show, hide, flush: saved, settingsChanged, month: () => ({ year: state.year, month: state.month }) };
}
