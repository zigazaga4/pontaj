// Application state and wiring.
import { api, onFileDrop } from "./api.js";
import { $, h, toast, busy } from "./dom.js";
import { renderGrid, refreshGrid, renderDetails } from "./grid.js";
import { renderSettings } from "./settings.js";

const MONTHS = ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];

const state = { settings: null, sheet: null, files: [], selected: null };

/** Keeps manual edits (codes, hours, functions) when the PDFs are re-read. */
function carryEdits(previous, next) {
  if (!previous || previous.year !== next.year || previous.month !== next.month) return next;
  const old = new Map(previous.employees.map((e) => [e.name, e]));
  for (const emp of next.employees) {
    const prev = old.get(emp.name);
    if (!prev) continue;
    emp.function = prev.function;
    emp.days.forEach((d, i) => {
      d.overrideHours = prev.days[i]?.overrideHours ?? null;
      d.code = prev.days[i]?.code ?? null;
    });
  }
  return next;
}

async function loadFiles(paths) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  if (!pdfs.length) {
    if (paths.length) toast("Se acceptă doar fișiere PDF.", { kind: "warn" });
    return;
  }
  const files = [...new Set([...state.files, ...pdfs])];
  busy(`Se citesc ${files.length} PDF-uri…`);
  try {
    const sheet = await api.parsePdfs(files, state.settings);
    state.files = files;
    state.sheet = await api.recalculate(carryEdits(state.sheet, sheet), state.settings);
    render();
    toast(`${sheet.employees.length} șoferi încărcați.`, { kind: "ok", timeout: 2500 });
  } catch (err) {
    toast(String(err), { kind: "error", timeout: 0 });
  } finally {
    busy(null);
  }
}

async function recalc() {
  state.sheet = await api.recalculate(state.sheet, state.settings);
  refreshGrid($("#grid-wrap"), state.sheet);
  renderSummary();
  if (state.selected) renderDetails($("#details"), state.sheet, ...state.selected);
}

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
  renderGrid($("#grid-wrap"), state.sheet, {
    onEdit: async (ei, di, edit) => {
      Object.assign(state.sheet.employees[ei].days[di], edit);
      await recalc();
    },
    onSelect: (ei, di) => {
      state.selected = [ei, di];
      renderDetails($("#details"), state.sheet, ei, di);
    },
    onFunction: (ei, text) => {
      state.sheet.employees[ei].function = text;
    },
  });
}

function renderSummary() {
  const s = state.sheet;
  const sum = (key) => s.employees.reduce((acc, e) => acc + e.totals[key], 0);
  const company = state.settings.companyName || s.company;
  const chip = (label, value) => h("div", { class: "chip" }, h("small", {}, label), h("b", {}, value));
  $("#summary").replaceChildren(
    h("div", { class: "title" }, h("h2", {}, `${MONTHS[s.month - 1]} ${s.year}`), h("span", {}, company + (s.location ? ` · ${s.location}` : ""))),
    chip("Zile lucrătoare", s.workingDays),
    chip("Normă", `${s.normHours} ore`),
    chip("Șoferi", s.employees.length),
    chip("Total ore lucrate", sum("worked")),
    chip("Ore de noapte", sum("night")),
  );
}

function renderFiles() {
  $("#files").replaceChildren(
    ...state.files.map((path) => h("span", { class: "file", title: path }, "📄 ", path.split(/[\\/]/).pop())),
  );
}

function renderWarnings() {
  const list = state.sheet.warnings;
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
  if (state.sheet) await recalc();
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
    Object.assign(state, { sheet: null, files: [], selected: null });
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
