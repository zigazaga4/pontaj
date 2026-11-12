// Application state and wiring.
import { api, onFileDrop } from "./api.js";
import { $, h, toast, busy } from "./dom.js";
import { renderGrid, refreshGrid, renderDetails } from "./grid.js";
import { renderScanned } from "./scanned.js";
import { renderDaily } from "./daily.js";
import { renderSettings } from "./settings.js";
import { signed } from "./time.js";

const MONTHS = ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];

/** The three steps, in the order the user goes through them. */
const TABS = [
  { id: "scanned", label: "Date citite din PDF", hint: "Verifică ce a citit aplicația", perDriver: true },
  { id: "daily", label: "Pontaj pe zile", hint: "Ore și intervale pentru fiecare zi", perDriver: true },
  { id: "sheet", label: "Foaia colectivă", hint: "Rezultatul final, gata de export", perDriver: false },
];

const state = { settings: null, sheet: null, files: [], selected: null, tab: "scanned", emp: 0, view: null };

const basename = (path) => path.split(/[\\/]/).pop();

async function loadFiles(paths) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  if (!pdfs.length) {
    if (paths.length) toast("Se acceptă doar fișiere PDF.", { kind: "warn" });
    return;
  }
  const fresh = pdfs.filter((p) => !state.files.some((f) => basename(f) === basename(p)));
  if (!fresh.length) {
    toast("Fișierele acestea sunt deja încărcate.", { kind: "info", timeout: 2500 });
    return;
  }
  busy(`Se citesc ${fresh.length} PDF-uri…`);
  try {
    const first = !state.sheet;
    const sheet = await api.parsePdfs(fresh, state.settings, state.sheet);
    // A PDF of another month starts a new timesheet: forget the files that are no longer part of it.
    state.files = [...state.files, ...fresh].filter((f) => sheet.sources.includes(basename(f)));
    state.sheet = sheet;
    if (first || state.emp >= sheet.employees.length) Object.assign(state, { tab: "scanned", emp: 0, selected: null });
    render();
    toast(`${sheet.employees.length} șoferi încărcați. Verifică datele citite, apoi treci la pontaj.`, { kind: "ok", timeout: 4000 });
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  } finally {
    busy(null);
  }
}

// Edits are applied and recalculated one after another, so a quick second edit
// is never lost to the answer of the first.
let queue = Promise.resolve();

/** Applies `mutate(sheet)`, recalculates in Rust and updates the screen. */
function edit(mutate, { structural = false } = {}) {
  queue = queue
    .then(async () => {
      mutate?.(state.sheet);
      state.sheet = await api.recalculate(state.sheet, state.settings);
      structural ? renderView() : state.view?.refresh();
      renderSummary();
      renderDrivers();
      renderWarnings();
      if (state.tab === "sheet" && state.selected) renderDetails($("#details"), state.sheet, ...state.selected);
    })
    .catch((err) => toast(String(err), { kind: "error", timeout: 0 }));
  return queue;
}

const viewContext = () => ({
  sheet: () => state.sheet,
  settings: () => state.settings,
  onEdit: (ei, ai, patch) => edit((s) => Object.assign(s.employees[ei].activities[ai], patch)),
  onAdd: (ei) =>
    edit((s) => {
      const rows = s.employees[ei].activities;
      const last = rows[rows.length - 1];
      const date = last?.date ?? `${s.year}-${String(s.month).padStart(2, "0")}-01`;
      rows.push({ date, start: 8 * 60, end: 16 * 60, span: 480, driving: 0, otherWork: 480, availability: 0, rest: 0, dayWork: 480, nightWork: 0, work: 480, source: "", manual: true });
    }, { structural: true }),
  onDelete: (ei, ai) => edit((s) => s.employees[ei].activities.splice(ai, 1), { structural: true }),
  onDayEdit: (ei, di, patch) => edit((s) => Object.assign(s.employees[ei].days[di], patch)),
});

function render() {
  const has = !!state.sheet;
  $("#empty").hidden = has;
  $("#workspace").hidden = !has;
  $("#btn-export").disabled = !has;
  $("#btn-clear").hidden = !has;
  if (!has) return;
  renderSummary();
  renderFiles();
  renderWarnings();
  renderTabs();
  renderDrivers();
  renderView();
}

function renderTabs() {
  const index = TABS.findIndex((t) => t.id === state.tab);
  const go = (i) => {
    state.tab = TABS[i].id;
    render();
  };
  $("#tabs").replaceChildren(
    ...TABS.map((tab, i) =>
      h("button", { class: `tab ${tab.id === state.tab ? "active" : ""}`, onclick: () => go(i) },
        h("span", { class: "tab-num" }, i + 1), h("span", {}, h("b", {}, tab.label), h("small", {}, tab.hint)))),
    h("div", { class: "tab-nav" },
      index > 0 ? h("button", { class: "btn", onclick: () => go(index - 1) }, "← Înapoi") : null,
      index < TABS.length - 1
        ? h("button", { class: "btn primary", onclick: () => go(index + 1) }, `Continuă: ${TABS[index + 1].label} →`)
        : h("button", { class: "btn primary", onclick: exportExcel }, "⬇ Exportă Excel")));
}

/** Driver list for the per-driver steps. */
function renderDrivers() {
  const tab = TABS.find((t) => t.id === state.tab);
  const box = $("#drivers");
  box.hidden = !tab.perDriver;
  if (!tab.perDriver) return;
  box.replaceChildren(
    h("h3", {}, "Șoferi"),
    ...state.sheet.employees.map((emp, i) => {
      const edited = emp.activities.filter((a) => a.manual).length + emp.days.filter((d) => d.overrideHours != null || d.overrideNight != null || d.overrideIntervals || d.code).length;
      return h("button", { class: `driver ${i === state.emp ? "active" : ""}`, onclick: () => selectDriver(i) },
        h("b", {}, emp.name),
        h("small", {}, `${emp.totals.worked} ore · ${emp.days.filter((d) => d.hours != null).length} zile`, edited ? h("span", { class: "edited" }, ` · ✎ ${edited}`) : null));
    }));
}

function selectDriver(i) {
  state.emp = i;
  renderDrivers();
  renderView();
}

function renderView() {
  const tab = state.tab;
  const holder = $("#view");
  $("#workspace").dataset.tab = tab;
  if (tab === "scanned") {
    state.view = renderScanned(holder, viewContext(), state.emp);
  } else if (tab === "daily") {
    state.view = renderDaily(holder, viewContext(), state.emp);
  } else {
    const wrap = h("div", { class: "grid-wrap", id: "grid-wrap" });
    const details = h("div", { class: "details", id: "details" },
      h("span", { class: "muted" }, "Selectează o zi pentru detalii. Scrie un număr pentru ore, un cod (CO, CM, CFP, ZLP…) pentru absențe sau lasă gol ca să revii la calculul automat."));
    holder.replaceChildren(wrap, details);
    renderGrid(wrap, state.sheet, {
      onEdit: (ei, di, patch) => edit((s) => Object.assign(s.employees[ei].days[di], patch)),
      onSelect: (ei, di) => {
        state.selected = [ei, di];
        renderDetails(details, state.sheet, ei, di);
      },
      onFunction: (ei, text) => {
        state.sheet.employees[ei].function = text;
      },
    });
    state.view = { refresh: () => refreshGrid(wrap, state.sheet) };
  }
}

function renderSummary() {
  const s = state.sheet;
  const sum = (key) => s.employees.reduce((acc, e) => acc + e.totals[key], 0);
  const company = state.settings.companyName || s.company;
  const chip = (label, value, title) => h("div", { class: "chip", title }, h("small", {}, label), h("b", {}, value));
  const gap = sum("worked") * 60 - sum("cardMin");
  $("#summary").replaceChildren(
    h("div", { class: "title" }, h("h2", {}, `${MONTHS[s.month - 1]} ${s.year}`), h("span", {}, company + (s.location ? ` · ${s.location}` : ""))),
    chip("Zile lucrătoare", s.workingDays),
    chip("Normă", `${s.normHours} ore`),
    chip("Șoferi", s.employees.length),
    chip("Total ore lucrate", sum("worked")),
    chip("Față de tahograf", signed(gap), "Orele din pontaj minus timpul lucrat pe card (rotunjiri și ore trecute manual)"),
    chip("Ore de noapte", sum("night")),
  );
}

function renderFiles() {
  $("#files").replaceChildren(
    ...state.files.map((path) => h("span", { class: "file", title: path }, "📄 ", basename(path))),
  );
}

function renderWarnings() {
  const list = [...state.sheet.warnings, ...(state.sheet.notices ?? [])];
  const box = $("#warnings");
  box.hidden = !list.length;
  box.replaceChildren(h("b", {}, "Atenție"), h("ul", {}, list.map((w) => h("li", {}, w))));
}

async function exportExcel() {
  try {
    const path = await api.exportExcel(state.sheet, state.settings);
    if (path) toast(`Excel salvat: ${path.split(/[\\/]/).pop()}`, { kind: "ok", timeout: 10000, action: { label: "Deschide", run: () => api.openFile(path) } });
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  }
}

function openSettings(open) {
  $("#settings-panel").hidden = !open;
  if (open) renderSettings($("#settings-form"), state.settings, onSettingsChange);
}

async function onSettingsChange(settings) {
  state.settings = settings;
  renderSettings($("#settings-form"), settings, onSettingsChange);
  try {
    await api.saveSettings(settings);
  } catch (err) {
    toast(`Setările nu au putut fi salvate: ${err}`, { kind: "warn" });
  }
  if (state.sheet) await edit(null, { structural: true });
}

async function addViaDialog() {
  const paths = await api.pickPdfs();
  if (paths.length) await loadFiles(paths);
}

async function init() {
  state.settings = await api.loadSettings();
  $("#btn-add").addEventListener("click", addViaDialog);
  $("#btn-add-empty").addEventListener("click", addViaDialog);
  $("#btn-export").addEventListener("click", exportExcel);
  $("#btn-settings").addEventListener("click", () => openSettings($("#settings-panel").hidden));
  $("#btn-close-settings").addEventListener("click", () => openSettings(false));
  $("#btn-clear").addEventListener("click", () => {
    Object.assign(state, { sheet: null, files: [], selected: null, tab: "scanned", emp: 0, view: null });
    render();
  });
  onFileDrop({
    enter: () => ($("#drop-hint").hidden = false),
    leave: () => ($("#drop-hint").hidden = true),
    drop: (paths) => {
      $("#drop-hint").hidden = true;
      loadFiles(paths);
    },
  });
  render();
  const startup = await api.startupFiles();
  if (startup.length) await loadFiles(startup);
}

init().catch((err) => toast(String(err), { kind: "error", timeout: 0 }));
