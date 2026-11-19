// The name and the job of the driver on screen, both editable (steps 1 and 2).
import { h } from "./dom.js";
import { boundInput } from "./table.js";

/** The name typed for driver `ei`, ready to use (capitals, single spaces), or null if it is empty or another driver's. */
export function acceptedName(sheet, ei, raw) {
  const value = raw.replace(/\s+/g, " ").trim().toUpperCase();
  return value && !sheet.employees.some((e, i) => i !== ei && e.name === value) ? value : null;
}

/**
 * Two inputs for driver `ei`: name and job. `ctx`: { sheet(), onEmployeeEdit(ei, patch) }.
 * Returns { el, update } like the bound inputs (`update` re-reads the sheet).
 */
export function driverFields(ctx, ei) {
  const employee = () => ctx.sheet().employees[ei];
  const bindings = [];
  const field = (label, key, { className, title, parse }) => {
    const binding = boundInput({
      className,
      maxlength: 60,
      title,
      get: () => employee()[key],
      parse,
      commit: (value) => ctx.onEmployeeEdit(ei, { [key]: value }),
    });
    bindings.push(binding);
    return h("label", { class: "driver-field" }, h("span", { class: "muted" }, label), binding.el);
  };

  const name = field("Șofer", "name", {
    className: "driver-name",
    title: "Numele din pontaj; îl poți corecta",
    parse: (text) => acceptedName(ctx.sheet(), ei, text) ?? undefined,
  });
  const job = field("Funcția", "function", {
    className: "driver-job",
    title: "Meseria sau funcția din pontaj",
    parse: (text) => text.trim(),
  });
  return { el: h("div", { class: "driver-fields" }, name, job), update: () => bindings.forEach((binding) => binding.update()) };
}
