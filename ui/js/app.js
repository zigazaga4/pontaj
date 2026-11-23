// Application state and wiring.
import { api, onFileDrop } from "./api.js";
import { $, h, toast, busy } from "./dom.js";
import { renderGrid, refreshGrid, renderDetails } from "./grid.js";
import { renderScanned } from "./scanned.js";
import { renderDaily } from "./daily.js";
import { acceptedName } from "./driver_fields.js";
import { renderSettings } from "./settings.js";
import { checkForUpdates, watchForUpdates } from "./updater.js";
import { createCatb } from "./catb.js";
import { GROUPS, createRoster } from "./roster.js";
import { isTir, tirToggle, withTir } from "./tir.js";
import { createAutosave } from "./autosave.js";
import { showDialog } from "./dialog.js";
import { monthTitle, savedMonthList } from "./saved_months.js";
import { hoursText, signed, MONTHS } from "./time.js";

/** The three steps, in the order the user goes through them. */
const TABS = [
  { id: "scanned", label: "Date citite din PDF", hint: "Verifică ce a citit aplicația", perDriver: true },
  { id: "daily", label: "Pontaj pe zile", hint: "Ore și intervale pentru fiecare zi", perDriver: true },
  { id: "sheet", label: "Foaia colectivă", hint: "Rezultatul final, gata de export", perDriver: false },
];

/** The two lists of the tachograph mode: the truck drivers (tir) have a tab of their own. */
const KINDS = [
  { id: "tachograph", label: "Tahograf" },
  { id: "tir", label: "Tir" },
];

const TAGLINES = {
  pdf: "Din raportul de tahograf direct în foaia colectivă de prezență",
  catb: "Din cursele aplicației ALT direct în foaia de pontaj",
  ...Object.fromEntries(Object.values(GROUPS).map((group) => [group.id, group.tagline])),
};

const MODES = ["pdf", "catb", ...Object.keys(GROUPS)];

// `months`: the months read from PDFs and kept on this computer (the sheet on screen is saved there by itself).
const state = { settings: null, sheet: null, months: [], selected: null, tab: "scanned", emp: 0, kind: "tachograph", view: null, mode: "pdf", focusDate: null };

const catb = createCatb({ getSettings: () => state.settings, saveSettings: persistSettings });
const roster = createRoster({ getSettings: () => state.settings, saveSettings: persistSettings });

const basename = (path) => path.split(/[\\/]/).pop();

const autosave = createAutosave(async () => state.sheet && (await api.pdfSave(state.sheet)), {
  onError: (err) => toast(`Modificările nu au putut fi salvate: ${err}`, { kind: "warn" }),
});

// Edits are applied and recalculated one after another, so a quick second edit
// is never lost to the answer of the first.
let queue = Promise.resolve();

/** Lets the edits in progress finish and writes the sheet to disk now (before another month opens, before an update restarts the app). */
async function flushPending() {
  await queue;
  await autosave.flush();
}

// The newest kept month opens by itself, once per run (not when PDFs are being opened, nor after the month was closed on purpose).
let restored = false;

async function restoreLastMonth() {
  if (restored || state.sheet) return;
  restored = true;
  const last = state.months.find((m) => !m.broken);
  if (last) await openSaved(last);
}

/** Switches between the tachograph PDFs, the Categoria B (API) pontaj and the office, garage and city staff. */
function setMode(mode) {
  state.mode = mode;
  localStorage.setItem("pontaj.mode", mode);
  document.querySelectorAll("#modes .mode").forEach((button) => button.classList.toggle("active", button.dataset.mode === mode));
  $("#tagline").textContent = TAGLINES[mode];
  $("#btn-add").hidden = mode !== "pdf";
  if (mode !== "catb") catb.hide();
  if (!(mode in GROUPS)) roster.hide();
  if (mode === "pdf") {
    render();
    restoreLastMonth();
    return;
  }
  $("#empty").hidden = true;
  $("#workspace").hidden = true;
  $("#btn-clear").hidden = true;
  $("#btn-export").disabled = false;
  if (mode === "catb") catb.show();
  else roster.show(mode);
}

// ---- the tachograph sheet: drivers and truck drivers (tir) ----

const inKind = (employee) => (state.kind === "tir") === isTir(employee);

/** Keeps the chosen driver inside the open list (Tahograf or Tir): when he is not there, the first one of the list. */
function ensureDriver() {
  const employees = state.sheet?.employees ?? [];
  if (employees[state.emp] && inKind(employees[state.emp])) return;
  state.emp = Math.max(0, employees.findIndex(inKind));
}

/** The "Tir" switch of a driver: the settings keep the list, so he is a truck driver in every month; the sheet is recomputed. */
async function setTir(employee, on) {
  await persistSettings(withTir(state.settings, employee.name, on));
  await edit(null, { structural: true });
  toast(`${employee.name} este acum în fila ${on ? "Tir" : "Tahograf"}.`, { kind: "ok", timeout: 3500 });
}

function setKind(kind) {
  state.kind = kind;
  render();
}

function renderKinds() {
  const employees = state.sheet.employees;
  $("#kinds").replaceChildren(
    ...KINDS.map(({ id, label }) =>
      h("button", { class: `kind ${id === state.kind ? "active" : ""}`, type: "button", onclick: () => setKind(id) },
        label, h("span", { class: "kind-count" }, employees.filter((e) => isTir(e) === (id === "tir")).length))));
}

async function loadFiles(paths) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  if (!pdfs.length) {
    if (paths.length) toast("Se acceptă doar fișiere PDF.", { kind: "warn" });
    return;
  }
  restored = true; // the PDFs are what he came for, not the month kept from before
  if (state.mode !== "pdf") setMode("pdf");
  const loaded = state.sheet?.sources ?? [];
  const fresh = pdfs.filter((p) => !loaded.includes(basename(p)));
  if (!fresh.length) {
    toast("Fișierele acestea sunt deja încărcate.", { kind: "info", timeout: 2500 });
    return;
  }
  busy(`Se citesc ${fresh.length} PDF-uri…`);
  try {
    await flushPending(); // the month on screen is written first: the PDFs may belong to another month
    const before = state.sheet;
    const sheet = await api.parsePdfs(fresh, state.settings, before);
    // PDFs of another month open that month: the one on screen stays kept as it was.
    const sameMonth = before && before.year === sheet.year && before.month === sheet.month;
    state.sheet = sheet;
    if (!sameMonth || state.emp >= sheet.employees.length) Object.assign(state, { tab: "scanned", emp: 0, selected: null });
    render();
    await refreshMonths();
    toast(`${sheet.employees.length} șoferi încărcați. Verifică datele citite, apoi treci la pontaj.`, { kind: "ok", timeout: 4000 });
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  } finally {
    busy(null);
  }
}

/** Applies `mutate(sheet)`, recalculates in Rust and updates the screen. */
function edit(mutate, { structural = false } = {}) {
  queue = queue
    .then(async () => {
      if (!state.sheet) return; // the month was closed meanwhile
      mutate?.(state.sheet);
      state.sheet = await api.recalculate(state.sheet, state.settings);
      autosave.markDirty();
      structural ? renderView() : state.view?.refresh();
      renderSummary();
      renderKinds();
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
  onEmployeeEdit: (ei, patch) => edit((s) => Object.assign(s.employees[ei], patch)),
  // From a day of the pontaj to the rows of that day in the data read from the PDF.
  onOpenScanned: (isoDate) => {
    state.focusDate = isoDate;
    state.tab = "scanned";
    render();
  },
  takeFocusDate: () => {
    const date = state.focusDate;
    state.focusDate = null;
    return date;
  },
});

function render() {
  if (state.mode !== "pdf") return;
  const has = !!state.sheet;
  $("#empty").hidden = has;
  $("#workspace").hidden = !has;
  $("#btn-export").disabled = !has;
  $("#btn-clear").hidden = !has;
  if (!has) return renderSaved();
  renderSummary();
  renderFiles();
  renderWarnings();
  renderTabs();
  renderKinds();
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
  ensureDriver();
  const tab = TABS.find((t) => t.id === state.tab);
  const box = $("#drivers");
  box.hidden = !tab.perDriver;
  if (!tab.perDriver) return;
  box.replaceChildren(
    h("h3", {}, state.kind === "tir" ? "Șoferi de tir" : "Șoferi"),
    ...state.sheet.employees.flatMap((emp, i) => {
      if (!inKind(emp)) return [];
      const edited = emp.activities.filter((a) => a.manual).length + emp.days.filter((d) => d.overrideHours != null || d.overrideNight != null || d.overrideIntervals || d.code).length;
      return h("div", { class: "driver-row has-tir" },
        h("button", { class: `driver ${i === state.emp ? "active" : ""}`, onclick: () => selectDriver(i) },
          h("b", {}, emp.name),
          h("small", {}, `${hoursText(emp.totals.worked)} ore · ${emp.days.filter((d) => d.hours > 0).length} zile`, edited ? h("span", { class: "edited" }, ` · ✎ ${edited}`) : null)),
        tirToggle(emp, (on) => setTir(emp, on)));
    }));
}

function selectDriver(i) {
  state.emp = i;
  renderDrivers();
  renderView();
}

function renderView() {
  ensureDriver();
  const tab = state.tab;
  const holder = $("#view");
  $("#workspace").dataset.tab = tab;
  if (!state.sheet.employees.some(inKind)) {
    holder.replaceChildren(h("div", { class: "empty-note" }, state.kind === "tir"
      ? "Niciun șofer de tir în luna aceasta. Apasă „Tir” lângă un șofer din lista Tahograf ca să-l treci aici; rămâne șofer de tir în toate lunile."
      : "Niciun șofer la tahograf în luna aceasta."));
    state.view = null;
  } else if (tab === "scanned") {
    state.view = renderScanned(holder, viewContext(), state.emp);
  } else if (tab === "daily") {
    state.view = renderDaily(holder, viewContext(), state.emp);
  } else {
    const wrap = h("div", { class: "grid-wrap", id: "grid-wrap" });
    const details = h("div", { class: "details", id: "details" },
      h("span", { class: "muted" }, "Selectează o zi pentru detalii. Scrie un număr pentru ore, un cod (CO, CM, CFP, ZLP…) pentru absențe sau lasă gol ca să revii la calculul automat."));
    holder.replaceChildren(wrap, details);
    renderGrid(wrap, state.sheet, {
      visible: inKind,
      onTir: (ei, on) => setTir(state.sheet.employees[ei], on),
      onEdit: (ei, di, patch) => edit((s) => Object.assign(s.employees[ei].days[di], patch)),
      onSelect: (ei, di) => {
        state.selected = [ei, di];
        renderDetails(details, state.sheet, ei, di);
      },
      onFunction: (ei, text) => {
        state.sheet.employees[ei].function = text;
        autosave.markDirty();
      },
      onName: (ei, raw) => {
        const current = state.sheet.employees[ei].name;
        const value = acceptedName(state.sheet, ei, raw) ?? current; // empty or another driver's name: keep the old one
        if (value !== current) viewContext().onEmployeeEdit(ei, { name: value });
        return value;
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
    chip("Total ore lucrate", hoursText(sum("worked"))),
    chip("Față de tahograf", signed(gap), "Orele din pontaj minus timpul lucrat pe card (rotunjiri și ore trecute manual)"),
    chip("Ore de noapte", hoursText(sum("night"))),
  );
}

function renderFiles() {
  $("#files").replaceChildren(
    ...state.sheet.sources.map((name) => h("span", { class: "file", title: name }, "📄 ", name)),
    h("button", { class: "btn small saved-button", type: "button", title: "Lunile citite din PDF-uri rămân salvate pe acest calculator", onclick: showSavedMonths }, `Luni salvate (${state.months.length})`),
  );
}

// ---- the months kept on this computer ----

/** The list of kept months, as the empty screen shows it. */
function renderSaved() {
  $("#saved").hidden = !state.months.length;
  $("#saved-list").replaceChildren(savedMonthList({ months: state.months, onOpen: openSaved, onDelete: deleteSaved }));
}

async function refreshMonths() {
  try {
    state.months = await api.pdfMonths();
  } catch (err) {
    toast(`Lista lunilor salvate nu a putut fi citită: ${err}`, { kind: "warn" });
  }
  if (state.mode !== "pdf") return;
  if (state.sheet) renderFiles();
  else renderSaved();
}

async function openSaved(month) {
  if (state.mode !== "pdf") setMode("pdf");
  restored = true;
  busy("Se deschide luna…");
  try {
    await flushPending();
    const sheet = await api.pdfOpen(month.year, month.month, state.settings);
    if (!sheet) {
      toast("Luna aceasta nu mai este în memorie.", { kind: "warn" });
      return await refreshMonths();
    }
    Object.assign(state, { sheet, tab: "scanned", emp: 0, selected: null });
    render();
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  } finally {
    busy(null);
  }
}

/** Forgets a kept month after asking (the PDFs and the Excel files of the person are not touched). Resolves true when it is gone. */
async function deleteSaved(month) {
  if (!confirm(`Ștergi ${monthTitle(month)} din memoria aplicației? Modificările făcute în această lună se pierd. Fișierele tale PDF și Excel nu se ating.`)) return false;
  try {
    await flushPending(); // what is waiting to be written must not bring the month back after it is deleted
    await api.pdfDelete(month.year, month.month);
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
    return false;
  }
  if (state.sheet?.year === month.year && state.sheet?.month === month.month) {
    Object.assign(state, { sheet: null, selected: null, tab: "scanned", emp: 0, view: null });
    render();
  }
  await refreshMonths();
  return true;
}

/** The "Luni salvate" button of an open month: a list to open another one from. */
async function showSavedMonths() {
  const chosen = await showDialog({
    title: "Luni salvate pe acest calculator",
    body: (close) => savedMonthList({ months: state.months, current: state.sheet, onOpen: close, onDelete: deleteSaved }),
  });
  if (chosen && typeof chosen === "object") await openSaved(chosen);
}

/** Closes the month on screen; it stays kept and can be opened again from the list. */
async function closeMonth() {
  await flushPending();
  Object.assign(state, { sheet: null, selected: null, tab: "scanned", emp: 0, view: null });
  render();
  await refreshMonths();
}

function renderWarnings() {
  const list = [...state.sheet.warnings, ...(state.sheet.notices ?? [])];
  const box = $("#warnings");
  box.hidden = !list.length;
  box.replaceChildren(h("b", {}, "Atenție"), h("ul", {}, list.map((w) => h("li", {}, w))));
}

/** The month the Excel file is for: the one of the tab in front (null while a tachograph month is not loaded). */
function exportMonth() {
  if (state.mode === "pdf") return state.sheet && { year: state.sheet.year, month: state.sheet.month };
  return state.mode === "catb" ? catb.month() : roster.month();
}

/**
 * The one Excel file of the month: the tachograph drivers and the truck drivers (when this is the month loaded from the PDFs),
 * Categoria B and the office, garage and city staff, all on one foaie colectivă. The last edit of every tab is saved first,
 * because Categoria B and the staff are read from what is saved.
 */
async function exportExcel() {
  const when = exportMonth();
  if (!when) return;
  try {
    if (!(await catb.confirmExport(when.year, when.month))) return;
    await roster.flush();
    const path = await api.exportAll(state.sheet, when.year, when.month, state.settings);
    if (!path) return;
    // The tachograph of the exported month is the one on screen or the one kept on this computer; the month on screen may be another one.
    const kept = state.months.some((m) => !m.broken && m.year === when.year && m.month === when.month);
    const left = state.sheet && (state.sheet.year !== when.year || state.sheet.month !== when.month) && !kept;
    const note = left ? `\nTahograful încărcat este pe ${String(state.sheet.month).padStart(2, "0")}.${state.sheet.year} și nu a intrat în fișier.` : "";
    toast(`Excel salvat: ${path.split(/[\\/]/).pop()}${note}`, { kind: left ? "warn" : "ok", timeout: left ? 20000 : 10000, action: { label: "Deschide", run: () => api.openFile(path) } });
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  }
}

function openSettings(open) {
  $("#settings-panel").hidden = !open;
  if (open) renderSettings($("#settings-form"), state.settings, onSettingsChange);
}

/** Keeps the settings in memory and on disk (the form, when open, is drawn again so it never edits an old copy). */
async function persistSettings(settings) {
  state.settings = settings;
  if (!$("#settings-panel").hidden) renderSettings($("#settings-form"), settings, onSettingsChange);
  try {
    await api.saveSettings(settings);
  } catch (err) {
    toast(`Setările nu au putut fi salvate: ${err}`, { kind: "warn" });
  }
}

async function onSettingsChange(settings) {
  await persistSettings(settings);
  await Promise.all([state.sheet ? edit(null, { structural: true }) : null, catb.settingsChanged(), roster.settingsChanged()]);
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
  document.querySelectorAll("#modes .mode").forEach((button) => button.addEventListener("click", () => setMode(button.dataset.mode)));
  const idle = () => state.mode === "pdf" && !state.sheet && $("#overlay").hidden; // nothing open, nothing loading: safe to restart
  // Every mode saves by itself; before the app restarts the last edit of each is written first.
  const updates = {
    canInstallNow: idle,
    beforeInstall: () => Promise.all([flushPending(), catb.flush(), roster.flush()]),
  };
  $("#btn-update").addEventListener("click", () => checkForUpdates({ ...updates, manual: true }));
  api.appVersion().then((v) => ($("#app-version").textContent = `Versiunea ${v}`)).catch(() => {});
  $("#btn-clear").addEventListener("click", closeMonth);
  window.addEventListener("pagehide", () => autosave.flush());
  document.addEventListener("visibilitychange", () => document.hidden && autosave.flush());
  onFileDrop({
    enter: () => ($("#drop-hint").hidden = false),
    leave: () => ($("#drop-hint").hidden = true),
    drop: (paths) => {
      $("#drop-hint").hidden = true;
      loadFiles(paths);
    },
  });
  const startup = await api.startupFiles();
  try {
    state.months = await api.pdfMonths();
  } catch (err) {
    toast(`Lista lunilor salvate nu a putut fi citită: ${err}`, { kind: "warn" });
  }
  restored = startup.length > 0;
  const remembered = localStorage.getItem("pontaj.mode");
  setMode(startup.length || !MODES.includes(remembered) ? "pdf" : remembered);
  if (startup.length) await loadFiles(startup);
  // At start nothing was edited yet and every month is saved on disk, so the app can restart whatever is open; later checks
  // only offer the update while something is open.
  checkForUpdates({ ...updates, canInstallNow: () => $("#overlay").hidden });
  watchForUpdates(updates);
}

init().catch((err) => toast(String(err), { kind: "error", timeout: 0 }));
