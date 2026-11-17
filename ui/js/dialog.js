// A small modal dialog. `showDialog` resolves with the value of the button pressed (null when it is closed with Esc).
import { h } from "./dom.js";

/**
 * @param {object} options
 * @param {string} options.title
 * @param {Node|string} options.body        the content (a form, a list…)
 * @param {{label:string, kind?:string, value?:any, action?:()=>any}[]} [options.buttons]
 *        `action()` runs when the button is pressed; returning `undefined` keeps the dialog open (invalid input),
 *        anything else is the result.
 * @param {boolean} [options.wide]
 */
export function showDialog({ title, body, buttons = [{ label: "Închide", kind: "primary", value: true }], wide = false }) {
  return new Promise((resolve) => {
    const close = (value) => {
      document.removeEventListener("keydown", onKey, true);
      back.remove();
      resolve(value);
    };
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close(null);
      }
    };
    const press = (button) => {
      if (!button.action) return close(button.value ?? true);
      const result = button.action();
      if (result !== undefined) close(result);
    };
    const back = h("div", { class: "modal-back", onmousedown: (e) => e.target === back && close(null) },
      h("div", { class: `modal ${wide ? "wide" : ""}`, role: "dialog", "aria-label": title },
        h("h3", {}, title),
        h("div", { class: "modal-body" }, body),
        h("div", { class: "modal-foot" }, buttons.map((button) => h("button", { class: `btn ${button.kind ?? ""}`, type: "button", onclick: () => press(button) }, button.label)))));
    document.addEventListener("keydown", onKey, true);
    document.body.append(back);
    back.querySelector("input, select, textarea")?.focus();
  });
}
