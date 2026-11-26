// Alte ore: periods in which a person of the staff tabs (Birou, Garaj, Oraș) works other hours than his plan, for some days or for
// alternate weeks (one week the new hours, one week the hours of the plan). The periods live on the person in the settings
// (`roster[].changes`); the host (the staff tabs) saves them and brings the saved months in line, this module only asks and shows.
// Only the days the plan has him working change: the free days (weekend, holidays, the days off of a cycle) stay free.
import { h, field } from "./dom.js";
import { showDialog } from "./dialog.js";
import { addDays, hhmm, isoToday, longDate, parseHm, shortDate } from "./time.js";

const WEEK = 7;

const dayDiff = (a, b) => Math.round((new Date(`${a}T12:00:00Z`) - new Date(`${b}T12:00:00Z`)) / 86400000);

/** Only a person with a plan has hours to change; somebody whose hours are typed by hand has no plan. */
export const canChangeHours = (member) => member.plan?.kind !== "manual";

/** The hours of the plan of the person (the first shift of a cycle), as a start for the form. */
export function planHours(plan) {
  if (plan?.kind === "weekdays") return { start: plan.start, end: plan.end };
  if (plan?.kind === "rotation" && plan.on?.length) return { start: plan.on[0].start, end: plan.on[0].end };
  return { start: 8 * 60, end: 16 * 60 };
}

const endAfter = (start, end) => (end <= start ? end + 1440 : end);

const span = (change) => `${hhmm(change.start)}–${hhmm(change.end)}`;

const period = (change) => (change.from === change.to ? longDate(change.from) : `${longDate(change.from)} – ${longDate(change.to)}`);

/** "16.11.2026 – 22.11.2026: 10:00–18:00" (with "o săptămână da, una nu" for alternate weeks). */
export function changeText(change) {
  return `${period(change)}: ${span(change)}${change.alternate ? ", o săptămână da, una nu" : ""}`;
}

/**
 * The stretches of a period: each is a run of days with the other hours or with the hours of the plan (`null`). Without `alternate`
 * the whole period is one run; with it the weeks, counted from the first day, take turns, the first with the other hours.
 * The same arithmetic as the backend (`HoursChange::applies_on`), only to show what a period would do before it is saved.
 */
export function stretches(change, limit = 10) {
  const out = [];
  const days = dayDiff(change.to, change.from) + 1;
  if (!change.alternate) return [{ from: change.from, to: change.to, hours: span(change) }];
  for (let at = 0, week = 0; at < days && week < limit; at += WEEK, week += 1) {
    out.push({ from: addDays(change.from, at), to: addDays(change.from, Math.min(at + WEEK, days) - 1), hours: week % 2 === 0 ? span(change) : null });
  }
  return out;
}

/**
 * The list of periods of one person with the form that adds one.
 * @param {object} options
 * @param {string} options.title
 * @param {() => { name: string, plan: object, changes?: object[] }} options.getPerson the person as he is now (read again after each change)
 * @param {(change: { from: string, to: string, start: number, end: number, alternate: boolean }) => Promise<void>} options.add saves the period and brings the saved months in line
 * @param {(change: object) => Promise<void>} options.remove takes a period back
 */
export function askHours({ title, getPerson, add, remove }) {
  return showDialog({
    title,
    wide: true,
    buttons: [{ label: "Gata", kind: "primary", value: true }],
    body: () => {
      const box = h("div", { class: "hours-changes" });
      const error = h("p", { class: "form-error", role: "alert" });
      const draw = () => box.replaceChildren(form(), list());
      const redo = (work) => {
        error.replaceChildren();
        return work().then(draw).catch((err) => error.replaceChildren(String(err)));
      };

      function form() {
        const plan = planHours(getPerson().plan);
        const from = h("input", { type: "date", value: isoToday() });
        const to = h("input", { type: "date", value: addDays(isoToday(), WEEK - 1) });
        const start = h("input", { type: "text", value: hhmm(plan.start), size: 6, inputmode: "numeric", placeholder: "09:00" });
        const end = h("input", { type: "text", value: hhmm(plan.end), size: 6, inputmode: "numeric", placeholder: "17:00" });
        const alternate = h("input", { type: "checkbox" });
        const preview = h("small", { class: "muted hours-preview" });
        // The last day follows the first one until it is chosen on its own, and never stays before it.
        from.addEventListener("change", () => { if (!to.value || to.value < from.value) to.value = from.value; });
        const read = () => {
          const [s, e] = [parseHm(start.value), parseHm(end.value)];
          if (!/^\d{4}-\d{2}-\d{2}$/.test(from.value) || !/^\d{4}-\d{2}-\d{2}$/.test(to.value)) return { error: "Alege prima și ultima zi." };
          if (to.value < from.value) return { error: "Ultima zi nu poate fi înainte de prima." };
          if (!Number.isInteger(s) || !Number.isInteger(e)) return { error: "Scrie orele ca 10:00 și 18:00." };
          return { change: { from: from.value, to: to.value, start: s, end: endAfter(s, e), alternate: alternate.checked } };
        };
        const show = () => {
          const { change } = read();
          if (!change) return void preview.replaceChildren();
          const runs = stretches(change);
          const more = change.alternate && dayDiff(change.to, change.from) + 1 > runs.length * WEEK ? [h("span", { class: "hours-line" }, "…")] : [];
          preview.replaceChildren(
            h("span", {}, change.alternate ? "Săptămânile, numărate din prima zi aleasă:" : "Zilele de lucru din perioadă:"),
            ...runs.map((run) => h("span", { class: "hours-line" }, `${shortDate(run.from)}${run.from === run.to ? "" : ` – ${shortDate(run.to)}`}: `, h("b", {}, run.hours ?? "orele din program"))),
            ...more);
        };
        for (const input of [from, to, start, end, alternate]) for (const event of ["input", "change"]) input.addEventListener(event, show);
        show();
        const submit = () => {
          error.replaceChildren();
          const { change, error: message } = read();
          if (!change) return void error.replaceChildren(message);
          redo(() => add(change));
        };
        return h("div", { class: "hours-form" },
          h("div", { class: "form-row" }, field("De la ziua", from), field("Până în ziua (inclusiv)", to), field("Ora începerii", start), field("Ora terminării", end),
            h("button", { class: "btn primary", type: "button", onclick: submit }, "＋ Adaugă perioada")),
          h("label", { class: "check-line", title: "Prima săptămână (șapte zile de la ziua aleasă) are orele noi, a doua cele din program, a treia iar cele noi, și tot așa până în ultima zi" }, alternate,
            "O săptămână cu aceste ore, o săptămână cu orele din program (pe rând)"),
          preview,
          h("small", { class: "muted" }, "Se schimbă doar zilele în care persoana lucrează după program; zilele libere (sâmbătă, duminică, sărbători, zilele libere ale turei) rămân libere. Zilele pe care ai scris tu ceva rămân cum sunt. Dacă două perioade se suprapun, o ține pe ultima adăugată."));
      }

      function list() {
        const changes = [...(getPerson().changes ?? [])].sort((a, b) => a.from.localeCompare(b.from));
        if (!changes.length) return h("p", { class: "muted" }, "Nicio perioadă cu alte ore: lucrează după program.");
        return h("ul", { class: "leave-plans" }, changes.map((change) =>
          h("li", { class: "leave-plan hours-change" },
            h("span", {}, "🕒 ", changeText(change)),
            h("button", { class: "btn small ghost", type: "button", title: "Scoate perioada: zilele ei revin la orele din program", onclick: () => redo(() => remove(change)) }, "✕ Șterge"))));
      }

      draw();
      return h("div", {}, box, error);
    },
  });
}
