// Settings form, generated from a field description list.
import { h } from "./dom.js";

const FIELDS = [
  { section: "Firmă" },
  { key: "companyName", label: "Denumire firmă", type: "text", hint: "Gol = numele din PDF" },
  { key: "cui", label: "CUI", type: "text" },
  { key: "defaultFunction", label: "Funcția implicită", type: "text" },
  { key: "plainNames", label: "Nume cu majuscule, fără diacritice", type: "checkbox" },
  { key: "preparedBy", label: "Întocmit de", type: "text" },
  { key: "approvedBy", label: "Verificat de", type: "text" },
  { section: "Calculul orelor" },
  { key: "roundUpFromMin", label: "Rotunjește în sus de la (minute)", type: "number", min: 1, max: 60, hint: "20 → 6:19 = 6 ore, 6:20 = 7 ore" },
  { key: "hoursPerDay", label: "Ore pe zi lucrătoare (normă)", type: "number", min: 1, max: 12 },
  { key: "mergeGapMin", label: "Pauză max. în aceeași tură (minute)", type: "number", min: 0, max: 240, hint: "Leagă turele care trec peste miezul nopții" },
  { section: "Intervale orare" },
  { key: "slotMin", label: "Rotunjire ore început/sfârșit (minute)", type: "number", min: 5, max: 60 },
  { key: "breakMin", label: "Pauza dintre cele două intervale (minute)", type: "number", min: 0, max: 240 },
  { key: "defaultStart", label: "Ora de început pentru zile adăugate manual", type: "time" },
  { section: "Ore de noapte" },
  { key: "nightStart", label: "Noaptea începe la", type: "time" },
  { key: "nightEnd", label: "Noaptea se termină la", type: "time" },
  { key: "nightThresholdMin", label: "Se trec doar de la (minute)", type: "number", min: 0, max: 600, hint: "180 = minim 3 ore de noapte" },
];

/** Renders the form into `form`; calls `onChange(newSettings)` after each edit. */
export function renderSettings(form, settings, onChange) {
  form.replaceChildren();
  for (const field of FIELDS) {
    if (field.section) {
      form.append(h("h3", {}, field.section));
      continue;
    }
    const id = `set-${field.key}`;
    const input = h("input", { id, type: field.type, min: field.min, max: field.max });
    if (field.type === "checkbox") input.checked = !!settings[field.key];
    else input.value = settings[field.key] ?? "";
    input.addEventListener("change", () => {
      const value = field.type === "checkbox" ? input.checked : field.type === "number" ? Number(input.value) : input.value;
      onChange({ ...settings, [field.key]: value });
    });
    form.append(
      h("label", { class: `field ${field.type === "checkbox" ? "check" : ""}`, for: id },
        h("span", {}, field.label),
        input,
        field.hint ? h("small", {}, field.hint) : null)
    );
  }
}
