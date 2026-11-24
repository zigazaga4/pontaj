// A small modal dialog. `showDialog` resolves with the value of the button pressed (null when it is closed with Esc).
import { h } from "./dom.js";

/**
 * @param {object} options
 * @param {string} options.title
 * @param {Node|string|((close: (value?: any) => void) => Node)} options.body
 *        the content (a form, a list…); a function gets `close(value)` so that something inside can close the dialog
 * @param {{label:string, kind?:string, value?:any, action?:()=>any}[]} [options.buttons]
 *        a button without `value` closes with true; `value: null` closes with null (cancel). `action()` runs when the button is pressed; returning `undefined` keeps the dialog open (invalid input),
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
      // A button with an explicit `value: null` (Anulează, Renunță) closes with null, not with true: a cancelled form is no answer.
      if (!button.action) return close("value" in button ? button.value : true);
      const result = button.action();
      if (result !== undefined) close(result);
    };
    const back = h("div", { class: "modal-back", onmousedown: (e) => e.target === back && close(null) },
      h("div", { class: `modal ${wide ? "wide" : ""}`, role: "dialog", "aria-label": title },
        h("h3", {}, title),
        h("div", { class: "modal-body" }, typeof body === "function" ? body(close) : body),
        h("div", { class: "modal-foot" }, buttons.map((button) => h("button", { class: `btn ${button.kind ?? ""}`, type: "button", onclick: () => press(button) }, button.label)))));
    document.addEventListener("keydown", onKey, true);
    document.body.append(back);
    back.querySelector("input, select, textarea")?.focus();
  });
}
