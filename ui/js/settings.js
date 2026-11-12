// Settings form, generated from a field description list.
import { h } from "./dom.js";

const NIGHT_PRESETS = [
  { label: "Codul muncii · 22:00–06:00 din raport", values: { nightSource: "card", nightStart: "22:00", nightEnd: "06:00" } },
  { label: "HG 38/2008 · 00:00–07:00", values: { nightSource: "intervals", nightStart: "00:00", nightEnd: "07:00" } },
  { label: "Practica anterioară · 22:00–07:00", values: { nightSource: "intervals", nightStart: "22:00", nightEnd: "07:00" } },
];

const FIELDS = [
  { section: "Firmă" },
  { key: "companyName", label: "Denumire firmă", type: "text", hint: "Gol = numele din PDF" },
  { key: "cui", label: "CUI", type: "text" },
  { key: "defaultFunction", label: "Funcția implicită", type: "text" },
  { key: "plainNames", label: "Nume cu majuscule, fără diacritice", type: "checkbox" },
  { key: "preparedBy", label: "Întocmit de", type: "text" },
  { key: "approvedBy", label: "Verificat de", type: "text" },
  { section: "Calculul orelor" },
  { key: "roundUpFromMin", label: "Rotunjește în sus de la (minute)", type: "number", min: 1, max: 60, hint: "20 → 6:19 = 6 ore, 6:20 = 7 ore. Legea nu impune o regulă de rotunjire: este convenția firmei." },
  { key: "hoursPerDay", label: "Ore pe zi lucrătoare (normă)", type: "number", min: 1, max: 12 },
  { key: "mergeGapMin", label: "Pauză max. în aceeași tură (minute)", type: "number", min: 0, max: 240, hint: "Leagă turele care trec peste miezul nopții" },
  { section: "Ore de început și de sfârșit" },
  {
    key: "intervalMode",
    label: "Cum se scriu",
    type: "select",
    options: [
      ["grid", "Pe grilă, însumează orele zilei"],
      ["card", "Orele reale de pe card"],
    ],
    hint: "Codul muncii art. 119 cere ora de început și de sfârșit a programului.",
  },
  { key: "slotMin", label: "Rotunjire pe grilă (minute)", type: "number", min: 5, max: 60 },
  { key: "breakMin", label: "Pauza dintre cele două intervale (minute)", type: "number", min: 0, max: 240 },
  { key: "defaultStart", label: "Ora de început pentru zile adăugate manual", type: "time" },
  { section: "Ore de noapte" },
  { presets: NIGHT_PRESETS },
  {
    key: "nightSource",
    label: "Noaptea se ia din",
    type: "select",
    options: [
      ["card", "Coloana „Noapte” din raportul de tahograf"],
      ["intervals", "Intervalele scrise în pontaj"],
    ],
  },
  { key: "nightStart", label: "Noaptea începe la", type: "time", hint: "Folosit pentru intervale și pentru zile fără card" },
  { key: "nightEnd", label: "Noaptea se termină la", type: "time" },
  { key: "nightThresholdMin", label: "Se trec doar de la (minute)", type: "number", min: 0, max: 600, hint: "180 = minim 3 ore (pragul pentru compensare, art. 125–126). 0 = se trec toate orele de noapte." },
];

function control(field, settings, onChange) {
  const id = `set-${field.key}`;
  if (field.type === "select") {
    const select = h("select", { id }, field.options.map(([value, label]) => h("option", { value }, label)));
    select.value = settings[field.key];
    select.addEventListener("change", () => onChange({ ...settings, [field.key]: select.value }));
    return select;
  }
  const input = h("input", { id, type: field.type, min: field.min, max: field.max });
  if (field.type === "checkbox") input.checked = !!settings[field.key];
  else input.value = settings[field.key] ?? "";
  input.addEventListener("change", () => {
    const value = field.type === "checkbox" ? input.checked : field.type === "number" ? Number(input.value) : input.value;
    onChange({ ...settings, [field.key]: value });
  });
  return input;
}

function presetButtons(presets, settings, onChange) {
  const active = (preset) => Object.entries(preset.values).every(([key, value]) => settings[key] === value);
  return h("div", { class: "presets" },
    presets.map((preset) =>
      h("button", { type: "button", class: `preset ${active(preset) ? "active" : ""}`, onclick: () => onChange({ ...settings, ...preset.values }) }, preset.label)));
}

/** Renders the form into `form`; calls `onChange(newSettings)` after each edit. */
export function renderSettings(form, settings, onChange) {
  form.replaceChildren();
  for (const field of FIELDS) {
    if (field.section) {
      form.append(h("h3", {}, field.section));
    } else if (field.presets) {
      form.append(presetButtons(field.presets, settings, onChange));
    } else {
      form.append(
        h("label", { class: `field ${field.type === "checkbox" ? "check" : ""}`, for: `set-${field.key}` },
          h("span", {}, field.label),
          control(field, settings, onChange),
          field.hint ? h("small", {}, field.hint) : null));
    }
  }
}
