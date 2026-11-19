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

const untariffed = (driver) => driver.trips.filter((t) => !t.edit.removed && t.eff.priceNote === "fără tarif").length;

/**
 * @param {{ getSettings: () => object, saveSettings: (settings: object) => Promise<void> }} host
 * Returns the functions the page needs: show/hide the mode, download, export, react to changed settings.
 */
export function createCatb({ getSettings, saveSettings }) {
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

  /** Puts a driver recomputed in Rust back into the sheet, saves and redraws. */
  function accept(fresh) {
    const at = state.sheet.drivers.findIndex((d) => d.id === fresh.id);
    if (at >= 0) state.sheet.drivers[at] = fresh;
    markDirty();
    renderAll();
  }

  /** Applies `mutate(driver)` to the chosen driver, recomputes him in Rust and redraws. Edits run one after another. */
  function edit(mutate) {
    queue = queue
      .then(async () => {
        const current = driver();
        if (!current) return;
        mutate(current);
        accept(await api.catbRecalculateDriver(current, state.sheet.year, state.sheet.month, getSettings()));
      })
      .catch(fail);
    return queue;
  }

  /** The "Rezolvă suprapunerea" button: Rust moves the times of the two trips of the pause (only those) and says what it did. */
  function resolveOverlap(pause) {
    queue = queue
      .then(async () => {
        const current = driver();
        if (!current) return;
        const done = await api.catbResolveOverlap(current, pause.prev, pause.next, state.sheet.year, state.sheet.month, getSettings());
        accept(done.driver);
        toast(`Suprapunere rezolvată. ${done.summary} Dacă scrii înapoi ora din API, revine.`, { kind: "ok", timeout: 12000 });
      })
      .catch(fail);
    return queue;
  }

  // ---- drivers taken out of the pontaj ----

  const hiddenDrivers = () => getSettings().catb?.hiddenDrivers ?? [];

  /**
   * Saves the list of drivers taken out of the pontaj (it lives in the settings, so every month obeys it) and applies it to
   * the open month: Rust moves them out of the sheet, or back into it with everything they had.
   */
  function setHiddenDrivers(change, selectId = null) {
    queue = queue
      .then(async () => {
        // The list is read when its turn comes, so two quick clicks never overwrite each other.
        const settings = getSettings();
        await saveSettings({ ...settings, catb: { ...settings.catb, hiddenDrivers: change(hiddenDrivers()) } });
        if (!state.sheet) return;
        state.sheet = await api.catbRecalculate(state.sheet, getSettings());
        if (selectId !== null && state.sheet.drivers.some((d) => d.id === selectId)) state.driverId = selectId;
        if (!driver()) state.driverId = state.sheet.drivers[0]?.id ?? null;
        markDirty();
        renderAll();
      })
      .catch(fail);
    return queue;
  }

  function removeDriver(d) {
    setHiddenDrivers((list) => [...list.filter((x) => x.id !== d.id), { id: d.id, name: d.name }]);
    toast(`${d.name} a fost scos din pontaj, în toate lunile.`, { kind: "ok", timeout: 12000, action: { label: "Anulează", run: () => restoreDriver(d.id) } });
  }

  function restoreDriver(id) {
    setHiddenDrivers((list) => list.filter((x) => x.id !== id), id);
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
      h("button", { class: "btn primary", title: "Aduce din API doar cursele și șoferii care nu sunt încă în foaie. Nimic din ce ai deja nu se schimbă.", onclick: fetchMonth }, state.sheet ? "⟳ Descarcă din nou (doar cele noi)" : "⬇ Descarcă din API"),
      h("span", { class: "muted" },
        state.sheet?.fetchedAt ? `Descărcat la ${state.sheet.fetchedAt}. O nouă descărcare adaugă doar ce e nou; ce ai deja în foaie nu se schimbă.` : saved ? "Luna este salvată pe acest calculator." : "Luna nu a fost descărcată încă."));
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
    const noTariff = sum(untariffed);
    box.replaceChildren(
      h("div", { class: "title" }, h("h2", {}, `${MONTHS[sheet.month - 1]} ${sheet.year}`), h("span", {}, "Categoria B · curse din API")),
      chip("Șoferi", drivers.length),
      chip("Plafon lunar", duration(sheet.capMin), { title: `${sheet.weekdays} zile luni–vineri × 8 ore, sărbătorile incluse` }),
      chip("Total ore", duration(sum((d) => d.totals.totalMin))),
      chip("Din care staționare", duration(sum((d) => d.totals.stationMin))),
      chip("Din care completare", duration(sum((d) => d.totals.fillMin ?? 0)), { title: "Timpul adăugat zilelor sub 8 ore (înainte de plecare și după sosire)" }),
      chip("Peste plafon", over, { tone: over ? "bad" : "", title: "Click: arată doar șoferii care au ceva de verificat", onclick: toggleIssues }),
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
        h("div", { class: "driver-list", id: "catb-driver-list" }),
        h("details", { class: "hidden-drivers", id: "catb-hidden" }, h("summary", { id: "catb-hidden-title" }), h("div", { id: "catb-hidden-list" })));
    }
    $("#catb-only-issues").checked = state.onlyIssues;
    const needle = state.filter.trim().toLowerCase();
    const list = state.sheet.drivers.filter((d) => (!needle || d.name.toLowerCase().includes(needle)) && (!state.onlyIssues || attention(d) > 0 || d.id === state.driverId));
    $("#catb-drivers-title").textContent = `Șoferi (${list.length})`;
    $("#catb-driver-list").replaceChildren(
      ...list.map((d) => {
        const flags = attention(d);
        return h("div", { class: "driver-row" },
          h("button", { class: `driver ${d.id === state.driverId ? "active" : ""} ${d.totals.overMin > 0 ? "over" : ""}`, onclick: () => selectDriver(d.id) },
            h("b", {}, d.name),
            h("small", {}, `${duration(d.totals.totalMin)} ore · ${d.totals.days} zile`,
              d.totals.overMin > 0 ? h("span", { class: "flag bad" }, ` · +${duration(d.totals.overMin)}`) : null,
              flags ? h("span", { class: `flag ${urgent(d) ? "bad" : "warn"}` }, ` · ⚠ ${flags}`) : null)),
          h("button", { class: "driver-remove", type: "button", title: `Scoate-l pe ${d.name} din pontaj, în toate lunile (îl poți adăuga înapoi)`, "aria-label": `Scoate pe ${d.name} din pontaj`, onclick: () => removeDriver(d) }, "×"));
      }));
    renderHiddenDrivers();
  }

  /** The drivers taken out of the pontaj, each with the way back. */
  function renderHiddenDrivers() {
    const out = [...hiddenDrivers()].sort((a, b) => (a.name ?? "").localeCompare(b.name ?? ""));
    $("#catb-hidden").hidden = !out.length;
    $("#catb-hidden-title").textContent = `Scoși din pontaj (${out.length})`;
    $("#catb-hidden-list").replaceChildren(
      h("p", { class: "muted" }, "Nu apar în nicio lună. Dacă îl adaugi înapoi, revine cu tot ce avea."),
      ...out.map((d) => h("div", { class: "hidden-driver" },
        h("span", {}, d.name || `Șofer #${d.id}`),
        h("button", { class: "btn small", type: "button", onclick: () => restoreDriver(d.id) }, "Adaugă înapoi"))));
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
    renderCatbDriver(view, { driver, sheet: () => state.sheet, edit, resolveOverlap, addTrip: () => askNewTrip(current, state.sheet) });
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
      // A new download only adds what the sheet does not have yet: remember what it has, to say what came in.
      const had = state.sheet ? new Set(state.sheet.drivers.flatMap((d) => d.trips.map((t) => `${d.id}|${t.id}`))) : null;
      const sheet = await api.catbFetch(state.year, state.month, getSettings(), state.sheet);
      state.sheet = sheet;
      state.saved = await api.catbMonths();
      if (!sheet.drivers.some((d) => d.id === state.driverId)) state.driverId = sheet.drivers[0]?.id ?? null;
      state.dirty = false;
      renderAll();
      const trips = sheet.drivers.reduce((n, d) => n + d.trips.length, 0);
      if (!had) {
        toast(`${sheet.drivers.length} șoferi, ${trips} curse. Verifică staționările și avertizările.`, { kind: "ok", timeout: 5000 });
      } else {
        const added = sheet.drivers.reduce((n, d) => n + d.trips.filter((t) => !had.has(`${d.id}|${t.id}`)).length, 0);
        const note = sheet.notices.length ? " Vezi observațiile de sub bară." : "";
        toast(added
          ? `${added} ${added === 1 ? "cursă nouă adăugată" : "curse noi adăugate"}. Ce aveai deja în foaie a rămas neatins.${note}`
          : `Nicio cursă nouă. Ce aveai deja în foaie a rămas neatins.${note}`, { kind: "ok", timeout: 6000 });
      }
    } catch (err) {
      fail(err);
    } finally {
      busy(null);
    }
  }

  /** What the office should still look at before the file goes out (the decision stays theirs). */
  function exportWarning() {
    const over = state.sheet.drivers.filter((d) => d.totals.overMin > 0);
    if (!over.length) return null;
    return h("div", { class: "export-warning" },
      over.length
        ? [
            h("p", {}, h("b", {}, `${over.length} ${over.length === 1 ? "șofer depășește" : "șoferi depășesc"} plafonul lunar`), `(${duration(state.sheet.capMin)} ore: 8 ore × ${state.sheet.weekdays} zile luni–vineri, cu sărbătorile). Pontajul nu ar trebui să-l depășească.`),
            h("table", { class: "data-table" },
              h("thead", {}, h("tr", {}, ["Șofer", "Ore în pontaj", "Depășire", "Propuneri"].map((t) => h("th", {}, t)))),
              h("tbody", {}, over.map((d) => h("tr", {}, h("td", { class: "left" }, d.name), h("td", {}, duration(d.totals.totalMin)), h("td", { class: "bad" }, `+${duration(d.totals.overMin)}`), h("td", {}, `${d.proposals.length}`))))),
            h("p", { class: "muted" }, "Fiecare șofer are sugestii (staționări de scurtat sau zile de scos). Tu hotărăști; Excel-ul se poate exporta și așa."),
          ]
        : null);
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
