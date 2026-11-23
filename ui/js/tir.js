// Who the truck drivers (tir) are. The list lives in the settings (`tirDrivers`), so it is kept from one month to the next, also
// in the months when a driver is missing from the tachograph; the backend gives every driver of a month his `group` from it.
import { h } from "./dom.js";

export const isTir = (employee) => employee.group === "tir";

/** A name the way the backend compares them: capitals, no diacritics, single spaces. */
const key = (name) => name.normalize("NFD").replace(/[̀-ͯ]/g, "").toUpperCase().replace(/\s+/g, " ").trim();

/** The settings with `name` put on the list of truck drivers (`on`) or taken off it. */
export function withTir(settings, name, on) {
  const others = settings.tirDrivers.filter((listed) => key(listed) !== key(name));
  return { ...settings, tirDrivers: on ? [...others, name] : others };
}

/** The "Tir" switch of a driver: `onChange(true|false)` when it is pressed. */
export function tirToggle(employee, onChange) {
  const on = isTir(employee);
  return h("button", {
    class: `tir-toggle ${on ? "on" : ""}`,
    type: "button",
    "aria-pressed": on,
    title: on ? "Șofer de tir (verde în Excel). Apasă ca să-l treci la tahograf." : "Apasă dacă e șofer de tir: devine verde în Excel și apare în fila Tir, în toate lunile.",
    onclick: (e) => {
      e.stopPropagation();
      onChange(!on);
    },
  }, "Tir");
}
