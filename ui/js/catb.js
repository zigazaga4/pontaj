// Categoria B — the drivers' trips from the ALT API, turned into a pontaj. This module is the controller: it loads and
// saves the month, keeps the edits in order, and draws the summary, the driver list and the chosen driver.
import { api } from "./api.js";
import { $, h, toast, busy } from "./dom.js";
import { showDialog } from "./dialog.js";
import { askNewTrip, chip, renderCatbDriver } from "./catb_driver.js";
import { duration, lei } from "./time.js";

const MONTHS = ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];
const SAVE_DELAY_MS = 1200;
const COMMON_PLACES = ["Constanta", "Aeroport Otopeni Bucuresti", "Bucuresti"];

/** Remarks that ask for a look (information-level ones do not count as attention). */
const worrying = (alert) => alert.level !== "info";

const worryingAlerts = (driver) => [
  ...driver.alerts,
  ...driver.pauses.flatMap((p) => p.alerts),
  ...driver.days.filter((d) => !d.excluded).flatMap((d) => d.alerts),
].filter(worrying);

const attention = (driver) => worryingAlerts(driver).length;

/** Red ones: things to decide, errors and missing values (the rest are orange warnings). */
const urgent = (driver) => worryingAlerts(driver).filter((alert) => alert.level === "alert").length;

/** A pause waiting for the office: not counted until somebody says what it was. */
const needsDecision = (pause) => pause.kind === "confirm" && pause.state === "auto" && pause.hoursOverride == null;

const untariffed = (driver) => driver.trips.filter((t) => !t.edit.removed && t.eff.priceNote === "fără tarif").length;

/**
 * @param {{ getSettings: () => object }} host
 * Returns the functions the page needs: show/hide the mode, download, export, react to changed settings.
 */
export function createCatb({ getSettings }) {
  const today = new Date();
  const previous = new Date(today.getFullYear(), today.getMonth() - 1, 1);
  const state = { sheet: null, year: previous.getFullYear(), month: previous.getMonth() + 1, saved: [], driverId: null, filter: "", onlyIssues: false, dirty: false, loaded: false };
  let queue = Promise.resolve();
  let saveTimer = null;

  const fail = (err) => toast(String(err), { kind: "error", timeout: 0 });
  const driver = () => state.sheet?.drivers.find((d) => d.id === state.driverId);

  // ---- saving ----

  function markDirty() {
    state.dirty = true;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(flush, SAVE_DELAY_MS);
  }

  async function flush() {
    clearTimeout(saveTimer);
    if (!state.dirty || !state.sheet) return;
    state.dirty = false;
    try {
      await api.catbSave(state.sheet);
    } catch (err) {
      state.dirty = true;
      toast(`Modificările nu au putut fi salvate: ${err}`, { kind: "warn" });
    }
  }

  // ---- editing ----

  /** Applies `mutate(driver)` to the chosen driver, recomputes him in Rust and redraws. Edits run one after another. */
  function edit(mutate) {
    queue = queue
      .then(async () => {
        const current = driver();
        if (!current) return;
        mutate(current);
        const fresh = await api.catbRecalculateDriver(current, state.sheet.year, state.sheet.month, getSettings());
        const at = state.sheet.drivers.findIndex((d) => d.id === fresh.id);
        if (at >= 0) state.sheet.drivers[at] = fresh;
        markDirty();
        renderAll();
      })
      .catch(fail);
    return queue;
  }

  // ---- drawing ----

  function renderBar() {
    const years = [...new Set([today.getFullYear() - 1, today.getFullYear(), today.getFullYear() + 1, state.year])].sort();
    const month = h("select", { onchange: (e) => openMonth(state.year, Number(e.target.value)) },
      MONTHS.map((name, i) => h("option", { value: i + 1, selected: i + 1 === state.month }, name)));
    const year = h("select", { onchange: (e) => openMonth(Number(e.target.value), state.month) },
      years.map((y) => h("option", { value: y, selected: y === state.year }, y)));
    const saved = state.saved.some(([y, m]) => y === state.year && m === state.month);
    $("#catb-bar").replaceChildren(
      h("label", { class: "bar-field" }, "Luna", month),
      h("label", { class: "bar-field" }, "Anul", year),
      h("button", { class: "btn primary", onclick: fetchMonth }, state.sheet ? "⟳ Descarcă din nou" : "⬇ Descarcă din API"),
      h("span", { class: "muted" },
        state.sheet?.fetchedAt ? `Descărcat la ${state.sheet.fetchedAt}. Modificările tale rămân și după o nouă descărcare.` : saved ? "Luna este salvată pe acest calculator." : "Luna nu a fost descărcată încă."));
  }

  function renderSummary() {
    const sheet = state.sheet;
    const box = $("#catb-summary");
    if (!sheet) {
      box.replaceChildren(h("div", { class: "title" }, h("h2", {}, `${MONTHS[state.month - 1]} ${state.year}`), h("span", {}, "Categoria B · curse din API")));
      return;
    }
    const drivers = sheet.drivers;
    const sum = (pick) => drivers.reduce((acc, d) => acc + pick(d), 0);
    const over = drivers.filter((d) => d.totals.overMin > 0).length;
    const confirm = sum((d) => d.pauses.filter(needsDecision).length);
    const noTariff = sum(untariffed);
    box.replaceChildren(
      h("div", { class: "title" }, h("h2", {}, `${MONTHS[sheet.month - 1]} ${sheet.year}`), h("span", {}, "Categoria B · curse din API")),
      chip("Șoferi", drivers.length),
      chip("Plafon lunar", duration(sheet.capMin), { title: `${sheet.weekdays} zile luni–vineri × 8 ore, sărbătorile incluse` }),
      chip("Total ore", duration(sum((d) => d.totals.totalMin))),
      chip("Din care staționare", duration(sum((d) => d.totals.stationMin))),
      chip("Peste plafon", over, { tone: over ? "bad" : "", title: "Click: arată doar șoferii care au ceva de verificat", onclick: toggleIssues }),
      chip("Pauze de confirmat", confirm, { tone: confirm ? "bad" : "", title: "Pauze lungi sau cu altă cursă între ele: nu se numără până nu spui ce au fost" }),
      chip("Curse fără tarif", noTariff, { tone: noTariff ? "bad" : "", title: "Cursele private/autocar cu destinații care nu sunt în lista de tarife (Reguli → Categoria B)" }),
      chip("Estimare plată", `~${lei(sum((d) => d.totals.payTotal))} lei`, { title: "Orientativ" }));
  }

  function renderNotices() {
    const notes = state.sheet?.notices ?? [];
    const box = $("#catb-notices");
    box.hidden = !notes.length;
    box.replaceChildren(h("b", {}, "De reținut de la ultima descărcare"), h("ul", {}, notes.map((n) => h("li", {}, n))));
  }

  function renderDrivers() {
    const box = $("#catb-drivers");
    box.hidden = !state.sheet;
    if (!state.sheet) return;
    // The search box and the checkbox are built once, so typing in them is never interrupted by a redraw.
    if (!box.querySelector(".driver-filter")) {
      box.replaceChildren(
        h("h3", { id: "catb-drivers-title" }),
        h("input", { class: "driver-filter", type: "search", placeholder: "Caută după nume…", oninput: (e) => { state.filter = e.target.value; renderDrivers(); } }),
        h("label", { class: "check-line" }, h("input", { id: "catb-only-issues", type: "checkbox", onchange: toggleIssues }), "Doar cei de verificat"),
        h("div", { class: "driver-list", id: "catb-driver-list" }));
    }
    $("#catb-only-issues").checked = state.onlyIssues;
    const needle = state.filter.trim().toLowerCase();
    const list = state.sheet.drivers.filter((d) => (!needle || d.name.toLowerCase().includes(needle)) && (!state.onlyIssues || attention(d) > 0 || d.id === state.driverId));
    $("#catb-drivers-title").textContent = `Șoferi (${list.length})`;
    $("#catb-driver-list").replaceChildren(
      ...list.map((d) => {
        const flags = attention(d);
        return h("button", { class: `driver ${d.id === state.driverId ? "active" : ""} ${d.totals.overMin > 0 ? "over" : ""}`, onclick: () => selectDriver(d.id) },
          h("b", {}, d.name),
          h("small", {}, `${duration(d.totals.totalMin)} ore · ${d.totals.days} zile`,
            d.totals.overMin > 0 ? h("span", { class: "flag bad" }, ` · +${duration(d.totals.overMin)}`) : null,
            flags ? h("span", { class: `flag ${urgent(d) ? "bad" : "warn"}` }, ` · ⚠ ${flags}`) : null));
      }));
  }

  function renderView() {
    const view = $("#catb-view");
    if (!state.sheet) {
      view.replaceChildren(emptyState());
      return;
    }
    const current = driver();
    if (!current) {
      view.replaceChildren(h("div", { class: "empty-note" }, state.sheet.drivers.length ? "Alege un șofer din listă." : "API-ul nu a trimis nicio cursă pentru luna aceasta."));
      return;
    }
    // Drawing again must not take the cursor or the scroll position away.
    const active = document.activeElement;
    const key = view.contains(active) ? active.dataset?.key : null;
    const scroll = $("#catb-scroll")?.scrollTop ?? 0;
    renderCatbDriver(view, { driver, sheet: () => state.sheet, edit, addTrip: () => askNewTrip(current, state.sheet) });
    const area = $("#catb-scroll");
    if (area) area.scrollTop = scroll;
    if (key) view.querySelector(`[data-key="${CSS.escape(key)}"]`)?.focus();
  }

  function emptyState() {
    return h("div", { class: "empty-note" },
      h("p", {}, `Pentru ${MONTHS[state.month - 1]} ${state.year} nu există încă curse în aplicație.`),
      h("button", { class: "btn primary", onclick: fetchMonth }, "⬇ Descarcă luna din API"));
  }

  function renderAll() {
    renderBar();
    renderSummary();
    renderNotices();
    renderDrivers();
    renderView();
  }

  function fillPlaces() {
    const names = [...COMMON_PLACES, ...(getSettings().catb?.tariffs ?? []).map((t) => t.name)];
    $("#catb-places").replaceChildren(...[...new Set(names)].map((name) => h("option", { value: name })));
  }

  // ---- actions ----

  function selectDriver(id) {
    state.driverId = id;
    renderDrivers();
    renderView();
  }

  function toggleIssues() {
    state.onlyIssues = !state.onlyIssues;
    renderDrivers();
  }

  async function openMonth(year, month) {
    await flush();
    Object.assign(state, { year, month, sheet: null, driverId: null });
    renderAll();
    busy("Se deschide luna…");
    try {
      state.sheet = await api.catbOpen(year, month, getSettings());
      state.driverId = state.sheet?.drivers[0]?.id ?? null;
    } catch (err) {
      fail(err);
    } finally {
      busy(null);
    }
    renderAll();
  }

  async function fetchMonth() {
    await flush();
    busy("Se descarcă cursele din API…");
    try {
      const sheet = await api.catbFetch(state.year, state.month, getSettings(), state.sheet);
      state.sheet = sheet;
      state.saved = await api.catbMonths();
      if (!sheet.drivers.some((d) => d.id === state.driverId)) state.driverId = sheet.drivers[0]?.id ?? null;
      state.dirty = false;
      renderAll();
      const trips = sheet.drivers.reduce((n, d) => n + d.trips.length, 0);
      toast(`${sheet.drivers.length} șoferi, ${trips} curse. Verifică staționările și avertizările.`, { kind: "ok", timeout: 5000 });
    } catch (err) {
      fail(err);
    } finally {
      busy(null);
    }
  }

  /** What the office should still look at before the file goes out (the decision stays theirs). */
  function exportWarning() {
    const over = state.sheet.drivers.filter((d) => d.totals.overMin > 0);
    const open = state.sheet.drivers.reduce((n, d) => n + d.pauses.filter(needsDecision).length, 0);
    if (!over.length && !open) return null;
    return h("div", { class: "export-warning" },
      over.length
        ? [
            h("p", {}, h("b", {}, `${over.length} ${over.length === 1 ? "șofer depășește" : "șoferi depășesc"} plafonul lunar`), `(${duration(state.sheet.capMin)} ore: 8 ore × ${state.sheet.weekdays} zile luni–vineri, cu sărbătorile). Pontajul nu ar trebui să-l depășească.`),
            h("table", { class: "data-table" },
              h("thead", {}, h("tr", {}, ["Șofer", "Ore în pontaj", "Depășire", "Propuneri"].map((t) => h("th", {}, t)))),
              h("tbody", {}, over.map((d) => h("tr", {}, h("td", { class: "left" }, d.name), h("td", {}, duration(d.totals.totalMin)), h("td", { class: "bad" }, `+${duration(d.totals.overMin)}`), h("td", {}, `${d.proposals.length}`))))),
            h("p", { class: "muted" }, "Fiecare șofer are sugestii (staționări de scurtat sau zile de scos). Tu hotărăști; Excel-ul se poate exporta și așa."),
          ]
        : null,
      open ? h("p", {}, `${open} pauze lungi sunt încă nehotărâte: nu se numără în ore.`) : null);
  }

  async function exportExcel() {
    if (!state.sheet) {
      toast("Nu e nimic de exportat: descarcă mai întâi luna din API.", { kind: "warn" });
      return;
    }
    const warning = exportWarning();
    if (warning) {
      const go = await showDialog({
        title: "Înainte de export",
        body: warning,
        wide: true,
        buttons: [{ label: "Înapoi la verificare", value: false }, { label: "Exportă oricum", kind: "primary", value: true }],
      });
      if (!go) return;
    }
    await queue; // let a pending edit finish
    await flush();
    try {
      const path = await api.catbExport(state.sheet, getSettings());
      if (path) toast(`Excel salvat: ${path.split(/[\\/]/).pop()}`, { kind: "ok", timeout: 10000, action: { label: "Deschide", run: () => api.openFile(path) } });
    } catch (err) {
      fail(err);
    }
  }

  /** The rules changed (tariffs, thresholds…): everything is recomputed. */
  async function settingsChanged() {
    fillPlaces();
    if (!state.sheet) {
      renderView();
      return;
    }
    queue = queue
      .then(async () => {
        state.sheet = await api.catbRecalculate(state.sheet, getSettings());
        markDirty();
        renderAll();
      })
      .catch(fail);
    await queue;
  }

  async function show() {
    $("#catb").hidden = false;
    fillPlaces();
    if (state.loaded) return renderAll();
    state.loaded = true;
    try {
      state.saved = await api.catbMonths();
      const [year, month] = state.saved[0] ?? [state.year, state.month];
      await openMonth(year, month);
    } catch (err) {
      fail(err);
      renderAll();
    }
  }

  function hide() {
    $("#catb").hidden = true;
    flush();
  }

  window.addEventListener("pagehide", flush);
  document.addEventListener("visibilitychange", () => document.hidden && flush());

  return { show, hide, fetchMonth, exportExcel, settingsChanged, flush };
}
