// Concedii: the window where leaves are written down in advance for a person (Categoria B, Birou, Garaj, Oraș), and the window that
// tells the office a trip arrived on a day of leave. The list of plans lives in the settings (`leaves`); the host (Categoria B or one of
// the staff tabs) saves it and brings its open month in line, this module only asks and shows.
import { h, field } from "./dom.js";
import { showDialog } from "./dialog.js";
import { LEAVE_TYPES, leaveConflictRows, leaveName } from "./catb_driver.js";
import { isoToday, longDate } from "./time.js";

const FREE_DAYS_HINT = "Bifat, concediul se trece și pe zilele în care persoana oricum nu lucrează. Nebifat, se trece doar pe zilele în care ar lucra. La birou, garaj și oraș, orele unei zile libere nu intră în total (nu sunt în normă), dar codul se vede în celulă și în Excel.";

const newId = () => `L${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;

/** "3 zile" / "o zi" for the calendar days of a plan, both ends included. */
export function planLength(plan) {
  const days = Math.round((new Date(`${plan.to}T12:00:00Z`) - new Date(`${plan.from}T12:00:00Z`)) / 86400000) + 1;
  return days === 1 ? "o zi" : `${days} zile`;
}

/**
 * The list of planned leaves of one place (Categoria B, or one staff tab) with the form that adds one.
 * @param {object} options
 * @param {string} options.title
 * @param {string} options.group the place the plans are for: "catB", "office", "garage" or "city"
 * @param {{ name: string, driverId?: number }[]} options.people who a leave can be written for
 * @param {() => object[]} options.getPlans the plans of this place, as they are now (read again after each change)
 * @param {(plan: object) => Promise<void>} options.add saves a new plan and brings the open month in line with it
 * @param {(plan: object) => Promise<void>} options.remove takes a plan back (its days in the saved months too)
 * @param {string} [options.note] a line under the form (what a leave does here)
 */
export function askLeaves({ title, group, people, getPlans, add, remove, note }) {
  return showDialog({
    title,
    wide: true,
    buttons: [{ label: "Gata", kind: "primary", value: true }],
    body: () => {
      const box = h("div", { class: "leaves" });
      const draw = () => box.replaceChildren(form(), list());
      const redo = (work) => {
        error.replaceChildren();
        return work().then(draw).catch((err) => error.replaceChildren(String(err)));
      };
      const error = h("p", { class: "form-error", role: "alert" });

      function form() {
        if (!people.length) return h("p", { class: "muted" }, "Nu este nimeni în listă. Pentru Categoria B, descarcă mai întâi luna din API ca să apară șoferii.");
        const who = h("select", {}, people.map((p, i) => h("option", { value: i }, p.name)));
        const kind = h("select", {}, LEAVE_TYPES.map(([code, name]) => h("option", { value: code }, `${name} (${code})`)));
        const from = h("input", { type: "date", value: isoToday() });
        const to = h("input", { type: "date", value: isoToday() });
        const onFree = h("input", { type: "checkbox" });
        // The last day follows the first one until it is chosen on its own, and never stays before it.
        from.addEventListener("change", () => { if (!to.value || to.value < from.value) to.value = from.value; });
        const submit = () => {
          error.replaceChildren();
          if (!from.value || !to.value) return void error.replaceChildren("Alege prima și ultima zi de concediu.");
          if (to.value < from.value) return void error.replaceChildren("Ultima zi nu poate fi înainte de prima.");
          const person = people[Number(who.value)];
          redo(() => add({ id: newId(), group, name: person.name, driverId: person.driverId ?? null, code: kind.value, from: from.value, to: to.value, onFreeDays: onFree.checked }));
        };
        return h("div", { class: "leaves-form" },
          h("div", { class: "form-row" }, field("Persoana", who), field("Felul concediului", kind)),
          h("div", { class: "form-row" }, field("De la", from), field("Până la (inclusiv)", to),
            h("button", { class: "btn primary", type: "button", onclick: submit }, "＋ Adaugă concediul")),
          h("label", { class: "check-line", title: FREE_DAYS_HINT }, onFree, "Și pe zilele libere (sâmbătă, duminică, sărbători, zilele libere ale turei)"),
          note ? h("small", { class: "muted" }, note) : null);
      }

      function list() {
        const plans = [...getPlans()].sort((a, b) => (a.from + a.name).localeCompare(b.from + b.name));
        if (!plans.length) return h("p", { class: "muted" }, "Nu este scris niciun concediu.");
        const today = isoToday();
        return h("ul", { class: "leave-plans" }, plans.map((plan) =>
          h("li", { class: `leave-plan ${plan.to < today ? "past" : ""}` },
            h("span", {},
              h("b", {}, plan.name), " · ",
              `${leaveName(plan.code)} (${plan.code}) · ${plan.from === plan.to ? longDate(plan.from) : `${longDate(plan.from)} – ${longDate(plan.to)}`} · ${planLength(plan)}${plan.onFreeDays ? " · și pe zilele libere" : ""}`),
            h("button", { class: "btn small ghost", type: "button", title: "Scoate concediul: zilele lui revin cum erau", onclick: () => redo(() => remove(plan)) }, "✕ Șterge"))));
      }

      draw();
      return h("div", {}, box, error);
    },
  });
}

/**
 * "Vezi că am primit cursă la persoana asta care avea concediu": every driver with a trip on a day of leave, each day with its
 * two answers. The list is read again after every answer, so what was decided leaves it.
 * @param {() => { name: string }[]} drivers the drivers that still have a day to decide (with `leaveConflicts`)
 * @param {(driver: object, conflict: object, worked: boolean) => Promise<void>} choose
 */
export function askLeaveConflicts(drivers, choose) {
  return showDialog({
    title: "Cursă la o persoană cu concediu",
    wide: true,
    buttons: [{ label: "Hotărăsc mai târziu", value: true }],
    body: (close) => {
      const box = h("div", { class: "leave-warning" });
      const draw = () => {
        const open = drivers();
        if (!open.length) return close(true);
        box.replaceChildren(
          h("h4", {}, "⚠ Vezi că am primit cursă la persoane care aveau concediu."),
          h("p", { class: "muted" }, "Pentru fiecare zi alege dacă a lucrat sau nu. Poți hotărî și mai târziu, din pontajul fiecărui șofer."),
          ...open.map((driver) => h("div", { class: "leave-driver" },
            h("h5", {}, driver.name),
            leaveConflictRows(driver, (conflict, worked) => choose(driver, conflict, worked).then(draw)))));
      };
      draw();
      return box;
    },
  });
}
