// Settings form, generated from a field description list.
import { h } from "./dom.js";

const NIGHT_PRESETS = [
  { label: "00:00–07:00 · din intervalele scrise (implicit)", values: { nightSource: "intervals", nightStart: "00:00", nightEnd: "07:00" } },
  { label: "22:00–06:00 · din raport (Codul muncii)", values: { nightSource: "card", nightStart: "22:00", nightEnd: "06:00" } },
  { label: "22:00–07:00 · din intervalele scrise", values: { nightSource: "intervals", nightStart: "22:00", nightEnd: "07:00" } },
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
  { section: "Categoria B (cursele din API)" },
  { key: "catb.apiKey", label: "Altă cheie API (opțional)", type: "password", hint: "Aplicația are deja cheia încorporată și nu trebuie să treci nimic aici. Completează doar dacă administratorul ALT îți dă o cheie nouă." },
  { key: "catb.apiBase", label: "Adresa API", type: "text" },
  { key: "catb.pairPay", label: "Plata unei zile normale, 2 curse regulate (lei)", type: "number", min: 0, step: 10, hint: "Fiecare cursă regulată = jumătate. Este doar o estimare." },
  { key: "catb.stationIncludedMin", label: "Staționare inclusă în preț (ore)", type: "number", scale: 60, min: 0, step: 0.5 },
  { key: "catb.stationRate", label: "Lei pe oră de staționare peste cele incluse", type: "number", min: 0, step: 1 },
  { key: "catb.stationWarnMin", label: "Avertizează de la staționare de (ore)", type: "number", scale: 60, min: 0, step: 0.5, hint: "Staționare mare = probabil a plecat și s-a întors mai târziu." },
  { key: "catb.longPauseMin", label: "Pauză lungă de la (ore)", type: "number", scale: 60, min: 1, step: 1, hint: "O pauză mai lungă e zi liberă între ture: nu se numără ca staționare până nu spui tu că a fost (excursie, a stat)." },
  { key: "catb.countOtherTrips", label: "Cursele private și autocarele intră în ore", type: "checkbox", hint: "Doar durata lor cunoscută; nu devin niciodată staționare." },
  { key: "catb.mainRoutePrivateRegular", label: "Privatele Constanța–Otopeni sunt curse regulate", type: "checkbox" },
  { key: "catb.showMoney", label: "Sumele apar în Excel", type: "checkbox", hint: "Oprit = Excel-ul are doar ore și avertizări (pe ecran estimarea se vede oricum)." },
  { key: "catb.tariffs", label: "Tarife pentru cursele exterioare (lei)", type: "textarea", rows: 14, toText: tariffsToText, fromText: textToTariffs, hint: "Un loc pe rând, de exemplu „Mangalia 40”. Cursele private se plătesc după destinație; locurile care lipsesc apar ca „fără tarif”." },
];

/** "Mangalia 40" per line ↔ [{ name, amount }]. */
function tariffsToText(list) {
  return (list ?? []).map((t) => `${t.name} ${t.amount}`).join("\n");
}

function textToTariffs(text) {
  return text
    .split("\n")
    .map((line) => line.trim().match(/^(.*?)[\s:=;-]*(\d+(?:[.,]\d+)?)\s*(?:lei)?$/i))
    .filter((m) => m && m[1].trim())
    .map((m) => ({ name: m[1].trim(), amount: Number(m[2].replace(",", ".")) }));
}

/** Settings are flat, except the Categoria B ones: "catb.apiKey" reads settings.catb.apiKey. */
const read = (settings, key) => key.split(".").reduce((value, part) => value?.[part], settings);

function write(settings, key, value) {
  const [head, ...rest] = key.split(".");
  return { ...settings, [head]: rest.length ? write(settings[head] ?? {}, rest.join("."), value) : value };
}

function control(field, settings, onChange) {
  const id = `set-${field.key}`;
  const current = read(settings, field.key);
  const change = (value) => onChange(write(settings, field.key, value));
  if (field.type === "select") {
    const select = h("select", { id }, field.options.map(([value, label]) => h("option", { value }, label)));
    select.value = current;
    select.addEventListener("change", () => change(select.value));
    return select;
  }
  if (field.type === "textarea") {
    const area = h("textarea", { id, rows: field.rows ?? 6, spellcheck: "false" });
    area.value = field.toText(current);
    area.addEventListener("change", () => change(field.fromText(area.value)));
    return area;
  }
  const input = h("input", { id, type: field.type, min: field.min, max: field.max, step: field.step, autocomplete: "off" });
  const scale = field.scale ?? 1; // some values are stored in minutes and shown in hours
  if (field.type === "checkbox") input.checked = !!current;
  else input.value = field.type === "number" && typeof current === "number" ? current / scale : current ?? "";
  input.addEventListener("change", () => {
    if (field.type === "checkbox") return change(input.checked);
    if (field.type !== "number") return change(input.value);
    const value = Number(input.value);
    if (input.value.trim() === "" || !Number.isFinite(value)) return renderAgain();
    change(field.scale ? Math.round(value * scale) : value);
  });
  // An invalid number goes back to the stored value.
  const renderAgain = () => (input.value = typeof current === "number" ? current / scale : "");
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
