// Mută tura: the window where the shift cycle of a person on a rotation (Oraș, or Birou/Garaj on a cycle) is moved by some days from a
// chosen day on. The moves live on the person in the settings (`roster[].moves`); the host (the staff tabs) saves them and brings the
// saved months in line, this module only asks and shows. The cycle goes on by itself after the move, month after month.
import { h, field } from "./dom.js";
import { showDialog } from "./dialog.js";
import { addDays, isoToday, longDate, shortDate } from "./time.js";

const dayDiff = (a, b) => Math.round((new Date(`${a}T12:00:00Z`) - new Date(`${b}T12:00:00Z`)) / 86400000);

/** Does a person on a cycle (`plan.kind === "rotation"`) have a move? Only a cycle can be moved. */
export const canMoveShifts = (member) => member.plan?.kind === "rotation";

/** "10.12.2026: o zi mai târziu" */
export function moveText(move) {
  const n = Math.abs(move.days);
  return `${longDate(move.from)}: ${n === 1 ? "o zi" : `${n} zile`} ${move.days > 0 ? "mai târziu" : "mai devreme"}`;
}

/**
 * The days of the next `count` days (from `from`, both ends in) the person works on his cycle with these moves.
 * The same arithmetic as the backend (`StaffMember::shift_on`), only to show what a move would do before it is made.
 */
export function shiftDays(plan, moves, from, count) {
  const cycle = plan.on.length + plan.off;
  const out = [];
  for (let i = 0; i < count; i += 1) {
    const day = addDays(from, i);
    const offset = moves.filter((move) => move.from <= day).reduce((sum, move) => sum + move.days, 0);
    const at = (((dayDiff(addDays(day, -offset), plan.anchor)) % cycle) + cycle) % cycle;
    if (at < plan.on.length) out.push(day);
  }
  return out;
}

/**
 * The list of moves of one person with the form that adds one.
 * @param {object} options
 * @param {string} options.title
 * @param {() => { name: string, plan: object, moves?: object[] }} options.getPerson the person as he is now (read again after each change)
 * @param {(move: { from: string, days: number }) => Promise<void>} options.add saves the move and brings the saved months in line
 * @param {(move: { from: string, days: number }) => Promise<void>} options.remove takes a move back
 */
export function askShifts({ title, getPerson, add, remove }) {
  return showDialog({
    title,
    wide: true,
    buttons: [{ label: "Gata", kind: "primary", value: true }],
    body: () => {
      const box = h("div", { class: "shifts" });
      const error = h("p", { class: "form-error", role: "alert" });
      const draw = () => box.replaceChildren(form(), list());
      const redo = (work) => {
        error.replaceChildren();
        return work().then(draw).catch((err) => error.replaceChildren(String(err)));
      };

      function form() {
        const from = h("input", { type: "date", value: isoToday() });
        const days = h("input", { type: "number", min: "1", max: "30", step: "1", value: "1", size: 4 });
        const way = h("select", {}, h("option", { value: "1" }, "mai târziu (după)"), h("option", { value: "-1" }, "mai devreme (înainte)"));
        const preview = h("small", { class: "muted shift-preview" });
        const move = () => ({ from: from.value, days: Number(way.value) * Number(days.value) });
        const valid = () => /^\d{4}-\d{2}-\d{2}$/.test(from.value) && Number.isInteger(Number(days.value)) && Number(days.value) >= 1 && Number(days.value) <= 30;
        const show = () => {
          if (!valid()) return void preview.replaceChildren();
          const person = getPerson();
          const list = (moves) => shiftDays(person.plan, moves, from.value, 14).map(shortDate).join(", ") || "nicio zi";
          preview.replaceChildren(
            h("span", {}, "Zilele de tură din următoarele 14 zile, de la ziua aleasă"),
            h("span", { class: "shift-line" }, "Acum: ", h("b", {}, list(person.moves ?? []))),
            h("span", { class: "shift-line" }, "După mutare: ", h("b", {}, list([...(person.moves ?? []), move()]))));
        };
        for (const input of [from, days, way]) for (const event of ["input", "change"]) input.addEventListener(event, show);
        show();
        const submit = () => {
          error.replaceChildren();
          if (!valid()) return void error.replaceChildren("Alege ziua și un număr de zile între 1 și 30.");
          redo(() => add(move()));
        };
        return h("div", { class: "shifts-form" },
          h("div", { class: "form-row" }, field("De la ziua", from), field("Cu câte zile", days), field("Direcția", way),
            h("button", { class: "btn primary", type: "button", onclick: submit }, "＋ Mută tura")),
          preview,
          h("small", { class: "muted" }, "Alege prima zi la care se schimbă tura. De la ea încolo toate turele se mută cu atâtea zile, iar ce urmează se calculează singur, lună după lună; vezi mai sus cum arată zilele înainte și după. Zilele pe care ai scris tu ceva rămân cum sunt."));
      }

      function list() {
        const moves = [...(getPerson().moves ?? [])].sort((a, b) => a.from.localeCompare(b.from));
        if (!moves.length) return h("p", { class: "muted" }, "Tura nu a fost mutată: ciclul merge ca în program.");
        return h("ul", { class: "leave-plans" }, moves.map((move) =>
          h("li", { class: "leave-plan shift-move" },
            h("span", {}, "⇄ ", moveText(move)),
            h("button", { class: "btn small ghost", type: "button", title: "Scoate mutarea: de la acea zi ciclul revine cum era", onclick: () => redo(() => remove(move)) }, "✕ Șterge"))));
      }

      draw();
      return h("div", {}, box, error);
    },
  });
}
