// Small DOM helpers shared by every view.

/** Creates an element: h("td", { class: "x", onclick: fn }, child, "text"). */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs ?? {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on") && typeof value === "function") el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else if (key === "dataset") Object.assign(el.dataset, value);
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

export const $ = (selector) => document.querySelector(selector);

/** Clock time from minutes relative to the day's midnight ("23:30", "+1 02:00"). */
export function clock(min) {
  const day = 1440;
  const m = ((min % day) + day) % day;
  const text = `${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
  if (min < 0) return `${text} (ziua ant.)`;
  if (min >= day) return `${text} (+1)`;
  return text;
}

export const duration = (min) => `${Math.floor(min / 60)}:${String(min % 60).padStart(2, "0")}`;

export function toast(message, { kind = "info", action, timeout = 5000 } = {}) {
  const box = h("div", { class: `toast ${kind}` }, h("span", {}, message));
  if (action) box.append(h("button", { class: "btn small", onclick: () => { action.run(); box.remove(); } }, action.label));
  box.append(h("button", { class: "toast-close", onclick: () => box.remove() }, "✕"));
  $("#toasts").append(box);
  if (timeout) setTimeout(() => box.remove(), timeout);
}

export function busy(text) {
  $("#overlay-text").textContent = text ?? "";
  $("#overlay").hidden = !text;
}
