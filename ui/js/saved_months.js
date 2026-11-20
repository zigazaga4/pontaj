// The tachograph months kept on this computer, as a list to open one from (or to forget one).
import { h } from "./dom.js";
import { hoursText, MONTHS } from "./time.js";

export const monthTitle = ({ year, month }) => `${MONTHS[month - 1]} ${year}`;

/** One line about a kept month: "7 șoferi · 714 ore · 14 PDF-uri". */
export function monthDetails(m) {
  if (m.broken) return "nu poate fi citită (copia .bak e lângă ea)";
  return `${m.drivers} ${m.drivers === 1 ? "șofer" : "șoferi"} · ${hoursText(m.worked)} ore · ${m.files} ${m.files === 1 ? "PDF" : "PDF-uri"}`;
}

/**
 * @param {object} options
 * @param {{year:number, month:number, drivers:number, files:number, worked:number, broken:boolean}[]} options.months
 * @param {{year:number, month:number}|null} [options.current]   the month on screen, marked in the list
 * @param {(month: object) => void} options.onOpen
 * @param {(month: object) => Promise<boolean>} options.onDelete   resolves true when the month was forgotten (its row goes away)
 */
export function savedMonthList({ months, current = null, onOpen, onDelete }) {
  let items = months;
  const box = h("div", { class: "saved-months" });

  const row = (m) =>
    h("div", { class: `saved-month ${current && current.year === m.year && current.month === m.month ? "current" : ""} ${m.broken ? "broken" : ""}` },
      h("button", { class: "driver saved-open", type: "button", disabled: m.broken, onclick: () => onOpen(m) },
        h("b", {}, monthTitle(m)), h("small", {}, monthDetails(m))),
      h("button", {
        class: "saved-remove", type: "button", title: `Șterge ${monthTitle(m)} din memorie`, "aria-label": `Șterge ${monthTitle(m)} din memorie`,
        onclick: async () => {
          if (await onDelete(m)) {
            items = items.filter((x) => x !== m);
            draw();
          }
        },
      }, "×"));

  function draw() {
    box.replaceChildren(...(items.length ? items.map(row) : [h("p", { class: "muted" }, "Nicio lună salvată încă.")]));
  }

  draw();
  return box;
}
