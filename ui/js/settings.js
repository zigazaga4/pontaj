// Settings form, generated from a field description list.
import { h } from "./dom.js";

const NIGHT_PRESETS = [
  { label: "00:00–07:00 · din intervalele scrise (implicit)", values: { nightSource: "intervals", nightStart: "00:00", nightEnd: "07:00" } },
  { label: "22:00–06:00 · din raport (Codul muncii)", values: { nightSource: "card", nightStart: "22:00", nightEnd: "06:00" } },
  { label: "22:00–07:00 · din intervalele scrise", values: { nightSource: "intervals", nightStart: "22:00", nightEnd: "07:00" } },
];

const FIELDS = [
  { section: "Firmă" },
  { key: "companyName", label: "Denumire firmă", type: "text", hint: "Gol = numele din PDF; la Categoria B (fără PDF) se scrie aici" },
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
  { key: "nightThresholdMin", label: "Se trec doar de la (minute)", type: "number", min: 0, max: 600, hint: "0 = se trec toate orele de noapte, de la prima (implicit). 180 = doar nopțile de minim 3 ore (pragul pentru compensare, art. 125–126)." },
  { section: "Categoria B (cursele din API)" },
  { key: "catb.apiKey", label: "Altă cheie API (opțional)", type: "password", hint: "Aplicația are deja cheia încorporată și nu trebuie să treci nimic aici. Completează doar dacă administratorul ALT îți dă o cheie nouă." },
  { key: "catb.apiBase", label: "Adresa API", type: "text" },
  { key: "catb.excludeDDrivers", label: "Cursele cu mașini de categoria D nu intră în Categoria B", type: "checkbox", hint: "Se hotărăște cursă cu cursă, după mașina din API (model sau locuri, mai jos): cursele făcute cu o mașină de categoria D ies din foaie (se pontează din tahograf), iar cele cu mașini mici rămân. Un șofer care a condus în lună doar mașini de categoria D nu apare deloc în foaie. Dacă mașina unei curse se corectează în API din D în B, cursa reapare la următoarea actualizare. Se aplică la următoarea actualizare din API (care are loc singură la câteva minute)." },
  { key: "catb.smallVehicleMaxSeats", label: "Mașină mică: cel mult atâtea locuri pentru pasageri", type: "number", min: 1, step: 1, hint: "Peste numărul acesta mașina e de categoria D. 8 = un microbuz cu 8 pasageri + șoferul rămâne mic (categoria B)." },
  { key: "catb.dModels", label: "Modele de categoria D, oricâte locuri ar avea", type: "textarea", rows: 4, toText: (list) => (list ?? []).join("\n"), fromText: (text) => text.split("\n").map((line) => line.trim()).filter(Boolean), hint: "Un model pe rând, așa cum îl scrie aplicația ALT (ajunge o parte din nume, fără diacritice, litere mari sau mici). Folosit mai ales când API-ul nu dă numărul de locuri." },
  { key: "catb.pairPay", label: "Plata unei zile normale, 2 curse regulate (lei)", type: "number", min: 0, step: 10, hint: "Fiecare cursă regulată = jumătate. Este doar o estimare." },
  { key: "catb.stationIncludedMin", label: "Staționare inclusă în preț (ore)", type: "number", scale: 60, min: 0, step: 0.5 },
  { key: "catb.stationRate", label: "Lei pe oră de staționare peste cele incluse", type: "number", min: 0, step: 1 },
  { key: "catb.stationWarnMin", label: "Avertizează de la staționare de (ore)", type: "number", scale: 60, min: 0, step: 0.5, hint: "Staționare mare = probabil a plecat și s-a întors mai târziu." },
  { key: "catb.longPauseMin", label: "Pauză lungă de la (ore)", type: "number", scale: 60, min: 1, step: 1, hint: "Staționarea e doar la Otopeni. O așteptare de atâtea ore sau mai lungă se socotește automat pauză (s-a întors altfel): nu se numără și nu se plătește. Pe fiecare pauză poți alege „Staționare”." },
  { key: "catb.countOtherTrips", label: "Cursele private și autocarele intră în ore", type: "checkbox", hint: "Doar durata lor cunoscută; nu devin niciodată staționare." },
  { key: "catb.mainRoutePrivateRegular", label: "Privatele spre/dinspre Otopeni sunt curse regulate", type: "checkbox", hint: "Orice cursă privată sau VIP cu un capăt la Otopeni/București (Constanța, Mamaia, Năvodari, chiar și locuri din afara județului) este cursă către Otopeni: se plătește ca o cursă regulată, intră în staționarea zilei și nu se plătește după tariful exterior." },
  { key: "catb.generateTransfers", label: "Adaugă singură drumul care lipsește între două curse", type: "checkbox", hint: "Dacă o cursă regulată pleacă dintr-un loc unde șoferul nu a ajuns cu cursa dinainte (Otopeni la un capăt), nu se știe cum a ajuns acolo: de obicei a mers cu mașina goală. Aplicația adaugă o „Deplasare” care se numără ca o cursă regulată: spre Otopeni, terminată când pleacă cursa de acolo; sau înapoi spre Constanța, pornită când a ajuns la Otopeni. O poți schimba sau scoate. La început și la sfârșit de lună se folosesc cursele din zilele vecine, care vin odată cu actualizarea din API (o lună salvată mai demult le primește la prima actualizare)." },
  { key: "catb.transferMin", label: "Cât durează o deplasare adăugată (ore)", type: "number", scale: 60, min: 0.5, step: 0.5, hint: "Implicit 3 ore. Pe fiecare deplasare poți schimba orele." },
  { key: "catb.fillDay", label: "Fiecare cursă regulată adaugă o oră zilei ei", type: "checkbox", hint: "O zi cu o cursă de 3 ore ajunge la 4 ore, o zi cu două curse la 8 ore: pentru fiecare cursă se adaugă o oră, jumătate înainte de prima plecare și jumătate după ultima sosire. Se aplică și deplasărilor adăugate de aplicație. O zi nu trece de 8 ore din cauza asta (o zi care are deja 8 ore sau mai multe nu primește nimic; cele peste 8 ore primesc o avertizare). Timpul adăugat intră direct în orele cursei: prima plecare apare mai devreme, ultima sosire mai târziu, iar lângă oră scrie cât era înainte și cât s-a scăzut din plecare sau s-a adăugat la sosire (în Excel, o notă pe celula orei). Pe fiecare zi poți schimba timpul adăugat chiar în căsuța de lângă oră." },
  { key: "catb.fillPerTripMin", label: "Cât adaugă fiecare cursă (ore)", type: "number", scale: 60, min: 0, step: 0.5, hint: "Implicit 1 oră pe cursă. Scrie 0,5 dacă vrei jumătate de oră pe cursă." },
  { key: "catb.weekMaxActiveMin", label: "Avertizează dacă într-o săptămână e activ pe cursă mai mult de (ore)", type: "number", scale: 60, min: 0, step: 1, hint: "Săptămâna = câte 7 zile de la începutul lunii (1–7, 8–14…). Se numără doar timpul în cursă, fără staționare și fără orele adăugate zilelor; zilele scoase din pontaj nu se numără. Implicit 40. Scrie 0 ca să nu mai avertizeze." },
  { key: "catb.weekMinFreeDays", label: "Zile libere cerute într-o săptămână de 7 zile", type: "number", min: 0, max: 7, step: 1, hint: "Zi liberă = zi fără ore în pontaj (concediile și zilele pe care le faci libere din panoul „Săptămâni” sunt libere). Implicit 2. Ultima săptămână a lunii, dacă are mai puțin de 7 zile, nu se verifică. Scrie 0 ca să nu mai avertizeze." },
  { key: "catb.overlapKeepPct", label: "La „Rezolvă suprapunerea”, o cursă păstrează cel puțin (% din durata ei)", type: "number", min: 0, max: 100, step: 5, hint: "Sosirea cursei dinainte se oprește unde pleacă cea de după. Dacă ar rămâne sub acest procent din durata ei, cursa de după pleacă ceva mai târziu." },
  { key: "catb.showMoney", label: "Sumele apar în Excel", type: "checkbox", hint: "Oprit = Excel-ul are doar ore și avertizări (pe ecran estimarea se vede oricum)." },
  { key: "catb.tariffs", label: "Tarife pentru cursele exterioare (lei)", type: "textarea", rows: 14, toText: tariffsToText, fromText: textToTariffs, hint: "Un loc pe rând, de exemplu „Mangalia 40”. Cursele private se plătesc după destinație; locurile care lipsesc apar ca „fără tarif”." },
  { key: "catb.extraLocalPlaces", label: "Alte locuri din județul Constanța", type: "textarea", rows: 4, toText: (list) => (list ?? []).join("\n"), fromText: (text) => text.split("\n").map((line) => line.trim()).filter(Boolean), hint: "Aplicația știe localitățile județului. O cursă privată spre un loc care nu e nici pe ruta Constanța–București, nici în județ, e „probabilă excursie”. Dacă o localitate din județ e socotită greșit excursie, scrie-o aici, câte una pe rând, așa cum o scrie aplicația ALT." },
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
