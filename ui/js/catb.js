// Categoria B — the drivers' trips from the ALT API, turned into a pontaj. This module is the controller: it loads and
// saves the month, keeps the edits in order, and draws the summary, the driver list and the chosen driver.
import { api } from "./api.js";
import { holdRow, rowIdOf } from "./anchor.js";
import { createAutosave } from "./autosave.js";
import { $, h, toast, busy } from "./dom.js";
import { showDialog } from "./dialog.js";
import { askNewTrip, chip, renderCatbDriver } from "./catb_driver.js";
import { flaggedWeeks } from "./catb_weeks.js";
import { askReconcile } from "./catb_reconcile.js";
import { monthFields } from "./month_bar.js";
import { duration, lei, MONTHS, signed } from "./time.js";

/** The month on screen is asked from the API again this often (a correction made there shows up without a click). */
const REFRESH_EVERY_MS = 10 * 60 * 1000;
const REFRESH_RETRY_MS = 2 * 60 * 1000;
const REFRESH_TICK_MS = 30 * 1000;
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
  const state = { sheet: null, year: previous.getFullYear(), month: previous.getMonth() + 1, saved: [], driverId: null, filter: "", onlyIssues: false, loaded: false, refreshError: null };
  let queue = Promise.resolve();
  let refreshing = false;
  let nextRefreshAt = 0;

  const fail = (err) => toast(String(err), { kind: "error", timeout: 0 });
  const driver = () => state.sheet?.drivers.find((d) => d.id === state.driverId);

  // ---- saving ----

  const { markDirty, flush } = createAutosave(async () => state.sheet && (await api.catbSave(state.sheet)), {
    onError: (err) => toast(`Modificările nu au putut fi salvate: ${err}`, { kind: "warn" }),
  });

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

  /**
   * The buttons of a drive made by the app: moves it by whole days (`days` < 0 = earlier; the way back is the opposite number).
   * Rust keeps the drive inside the month and recomputes the driver; when a move is not possible it says why.
   */
  function moveTrip(trip, days) {
    queue = queue
      .then(async () => {
        const current = driver();
        if (!current) return;
        accept(await api.catbMoveTrip(current, trip.id, days, state.sheet.year, state.sheet.month, getSettings()));
      })
      .catch((err) => toast(String(err), { kind: "warn", timeout: 9000 }));
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
    const saved = state.saved.some(([y, m]) => y === state.year && m === state.month);
    $("#catb-bar").replaceChildren(
      ...monthFields(state, openMonth, today),
      h("button", { class: "btn primary", title: "Aduce acum din API ultimele date ale lunii. Corecturile din API (mașină, oră, loc) înlocuiesc datele vechi; ce ai modificat tu rămâne neschimbat.", onclick: fetchMonth }, state.sheet ? "⟳ Actualizează din API" : "⬇ Descarcă din API"),
      state.sheet && h("button", { class: "btn", title: "Pentru cazul în care orele au fost schimbate direct în Excel: alegi fișierul Excel și orele din aplicație se aduc la ce scrie în el (vezi mai întâi ce se schimbă).", onclick: reconcileWithExcel }, "⇄ Reglează după Excel"),
      h("span", { class: "muted", title: "Se actualizează singură la câteva minute; ce ai modificat tu rămâne neschimbat." },
        state.sheet?.fetchedAt
          ? `Actualizat din API la ${state.sheet.fetchedAt}${state.refreshError ? " · ultima încercare n-a mers (internet?), se mai încearcă" : " · se actualizează singură"}`
          : saved ? "Luna este salvată pe acest calculator." : "Luna nu a fost descărcată încă."));
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
    const adjusted = sum((d) => d.totals.adjustMin ?? 0);
    const weekIssues = sum(flaggedWeeks);
    const limits = getSettings().catb ?? {};
    box.replaceChildren(
      h("div", { class: "title" }, h("h2", {}, `${MONTHS[sheet.month - 1]} ${sheet.year}`), h("span", {}, "Categoria B · curse din API")),
      chip("Șoferi", drivers.length),
      chip("Plafon lunar", duration(sheet.capMin), { title: `${sheet.weekdays} zile luni–vineri × 8 ore, sărbătorile incluse` }),
      chip("Total ore", duration(sum((d) => d.totals.totalMin)), {
        title: "Orele din curse, cu timpul adăugat în orele de plecare și de sosire ale zilelor (o oră pe cursă regulată, fără ca o zi să treacă de 8 ore). Staționarea nu intră aici.",
      }),
      chip("Staționare (nu intră în total)", duration(sum((d) => d.totals.stationMin)), { title: "Așteptarea dintre curse: se arată doar ca informație și se plătește după regula ei, dar nu intră în orele de lucru." }),
      ...(adjusted ? [chip("Reglări după Excel", signed(adjusted), { tone: "adjust", title: "Cât au adăugat (sau scos) zilele cu ore fixate după un Excel, față de ce ar fi dat cursele și regulile (sunt incluse în Total ore). Fiecare zi are „Anulează reglarea” în pontajul șoferului." })] : []),
      chip("Peste plafon", over, { tone: over ? "bad" : "", title: "Click: arată doar șoferii care au ceva de verificat", onclick: toggleIssues }),
      chip("Săptămâni de verificat", weekIssues, {
        tone: weekIssues ? "warn" : "",
        title: `Săptămâni de 7 zile (de la începutul lunii) cu peste ${duration(limits.weekMaxActiveMin ?? 2400)} ore active pe cursă sau cu mai puțin de ${limits.weekMinFreeDays ?? 2} zile libere. Click: arată doar șoferii care au ceva de verificat`,
        onclick: toggleIssues,
      }),
      chip("Curse fără tarif", noTariff, { tone: noTariff ? "bad" : "", title: "Cursele private/autocar cu destinații care nu sunt în lista de tarife (Reguli → Categoria B)" }),
      chip("Estimare plată", `~${lei(sum((d) => d.totals.payTotal))} lei`, { title: "Orientativ" }));
  }

  function renderNotices() {
    const notes = state.sheet?.notices ?? [];
    const box = $("#catb-notices");
    box.hidden = !notes.length;
    // One line until opened: the notes can be long and the days are what the page is for.
    box.replaceChildren(h("details", { open: noticesOpen, ontoggle: (e) => (noticesOpen = e.target.open) },
      h("summary", {}, h("b", {}, "De reținut de la ultima actualizare din API"), ` · ${notes.length} ${notes.length === 1 ? "notă" : "note"}`),
      h("ul", {}, notes.map((n) => h("li", {}, n)))));
  }
  let noticesOpen = false;

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
    // Drawing again must not take the cursor, the scroll position or the row being edited away: the row stays where it was on
    // screen (even when the edit moves it in the list) and is marked, so the eyes find it again.
    const active = document.activeElement;
    const key = view.contains(active) ? active.dataset?.key : null;
    if (view.contains(active)) workRow = rowIdOf(active) ?? workRow;
    const restore = holdRow(view, workRow, () => view.querySelector(".driver-sticky")?.offsetHeight ?? 0);
    renderCatbDriver(view, { driver, sheet: () => state.sheet, edit, resolveOverlap, moveTrip, addTrip: () => askNewTrip(current, state.sheet) });
    if (freshDriver) view.scrollTop = 0;
    else {
      const { row, jumped } = restore();
      row?.classList.add("working", ...(jumped ? ["jumped"] : []));
    }
    freshDriver = false;
    if (key) view.querySelector(`[data-key="${CSS.escape(key)}"]`)?.focus({ preventScroll: true });
  }

  /** The row (a trip, a pause, a day) the office touched last, and whether the driver on screen was just chosen. */
  let workRow = null;
  let freshDriver = false;

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
    if (id !== state.driverId) {
      workRow = null;
      freshDriver = true;
    }
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
    Object.assign(state, { year, month, sheet: null, driverId: null, refreshError: null });
    workRow = null;
    freshDriver = true;
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
    nextRefreshAt = 0;
    refreshQuietly(); // the saved month may be behind the API: look right away, without a word
  }

  // ---- the API ----

  /** Text typed into a field of the driver view and not confirmed yet (Enter, or leaving the field). */
  let unconfirmed = false;

  /** Someone is typing (or a dialog is open): redrawing now would eat what they have not confirmed yet. */
  function busyTyping() {
    const active = document.activeElement;
    const inField = Boolean(active?.matches?.("input, textarea") && $("#catb-view").contains(active));
    if (!inField) unconfirmed = false; // the field was redrawn or left: nothing is waiting to be confirmed
    return (inField && unconfirmed) || Boolean(document.querySelector(".modal-back"));
  }

  function watchTyping() {
    const view = $("#catb-view");
    view.addEventListener("input", () => (unconfirmed = true));
    for (const event of ["change", "focusout"]) view.addEventListener(event, () => (unconfirmed = false));
    // The row of the last field entered or button pressed is the one being worked on (see `renderView`).
    const touched = (e) => {
      const id = rowIdOf(e.target);
      if (!id || id === workRow) return;
      view.querySelector(".working")?.classList.remove("working", "jumped");
      workRow = id;
    };
    view.addEventListener("focusin", touched);
    view.addEventListener("pointerdown", touched, true);
  }

  const tripKeys = (sheet) => new Set(sheet ? sheet.drivers.flatMap((d) => d.trips.map((t) => `${d.id}|${t.id}`)) : []);
  const withoutStamp = (sheet) => JSON.stringify({ ...sheet, fetchedAt: null });

  /**
   * Brings the month from the API into the open sheet. The download (a few seconds, no sheet involved) runs outside the edit
   * queue; the merge runs inside it, on the sheet as it is at that moment, so an edit is never lost or overwritten. The API's
   * version replaces the sheet's data (a corrected car, hour or place; a cancelled trip); what the office changed stays.
   * Resolves with { first, added, left, changed }, or null when nothing was applied (another month was opened meanwhile, or
   * `skipIf()` said the moment is wrong).
   */
  async function syncMonth({ skipIf = () => false } = {}) {
    const { year, month } = state;
    const parsed = await api.catbDownload(year, month, getSettings());
    const turn = queue.then(async () => {
      if (state.year !== year || state.month !== month || skipIf()) return null;
      const before = state.sheet;
      const merged = await api.catbMerge(before, parsed, year, month, getSettings());
      const had = tripKeys(before);
      const has = tripKeys(merged);
      const result = {
        first: !before,
        added: [...has].filter((key) => !had.has(key)).length,
        left: [...had].filter((key) => !has.has(key)).length,
        changed: !before || withoutStamp(before) !== withoutStamp(merged),
      };
      if (result.changed) {
        state.sheet = merged;
        if (!merged.drivers.some((d) => d.id === state.driverId)) state.driverId = merged.drivers[0]?.id ?? null;
        markDirty();
        renderAll();
      } else {
        before.fetchedAt = merged.fetchedAt; // nothing new: the sheet on screen stays as it is
        renderBar();
      }
      return result;
    });
    queue = turn.catch(() => {}); // a failed merge must not block the edits behind it
    return turn;
  }

  /** What a refresh changed, in a few words (null when nothing changed). */
  function describeSync({ added, left, changed }) {
    if (!changed) return null;
    const parts = [];
    if (added) parts.push(`${added} ${added === 1 ? "cursă nouă" : "curse noi"}`);
    if (left) parts.push(`${left} ${left === 1 ? "cursă scoasă" : "curse scoase"}`);
    return parts.length ? parts.join(", ") : "corecturi la cursele existente";
  }

  /** The silent refresh of the month on screen: no overlay, and a failure is only a line in the bar (it tries again soon). */
  async function refreshQuietly() {
    if (refreshing || !state.sheet || document.hidden || $("#catb").hidden || busyTyping()) return;
    refreshing = true;
    try {
      const done = await syncMonth({ skipIf: busyTyping });
      state.refreshError = null;
      nextRefreshAt = Date.now() + (done ? REFRESH_EVERY_MS : REFRESH_TICK_MS);
      const what = done && describeSync(done);
      if (what) toast(`Actualizat din API: ${what}. Ce ai modificat tu a rămas neschimbat.${state.sheet.notices.length ? " Vezi observațiile de sub bară." : ""}`, { kind: "ok", timeout: 7000 });
      renderBar();
    } catch (err) {
      state.refreshError = String(err);
      nextRefreshAt = Date.now() + REFRESH_RETRY_MS;
      renderBar();
    } finally {
      refreshing = false;
    }
  }

  const refreshIfDue = () => Date.now() >= nextRefreshAt && refreshQuietly();

  /** The button: the same refresh, with the overlay and with the errors shown. A month never downloaded is saved right away. */
  async function fetchMonth() {
    await flush();
    busy("Se descarcă cursele din API…");
    refreshing = true;
    try {
      const done = await syncMonth();
      if (!done) return;
      await flush();
      state.saved = await api.catbMonths();
      state.refreshError = null;
      nextRefreshAt = Date.now() + REFRESH_EVERY_MS;
      renderBar();
      const note = state.sheet.notices.length ? " Vezi observațiile de sub bară." : "";
      if (done.first) {
        const trips = state.sheet.drivers.reduce((n, d) => n + d.trips.length, 0);
        toast(`${state.sheet.drivers.length} șoferi, ${trips} curse. Verifică staționările și avertizările.`, { kind: "ok", timeout: 5000 });
      } else {
        const what = describeSync(done);
        toast(`${what ? `Actualizat din API: ${what}.` : "Nimic nou în API."} Ce ai modificat tu a rămas neschimbat.${note}`, { kind: "ok", timeout: 6000 });
      }
    } catch (err) {
      fail(err);
    } finally {
      refreshing = false;
      busy(null);
    }
  }

  /**
   * "Reglează după Excel": somebody changed the hours in the Excel file without changing the app first, so the app follows the
   * file. Rust reads the file the office picks and works out the sheet with its hours; the office sees what would change and
   * only then does the sheet on screen become that one. No refresh from the API runs meanwhile (it would be merged into a sheet
   * that is about to be replaced).
   */
  async function reconcileWithExcel() {
    if (refreshing || !state.sheet) return;
    await queue; // let a pending edit finish
    await flush();
    refreshing = true;
    try {
      const done = await api.catbReconcile(state.sheet, getSettings());
      if (!done || !(await askReconcile(done.report))) return;
      state.sheet = done.sheet;
      if (!driver()) state.driverId = state.sheet.drivers[0]?.id ?? null;
      markDirty();
      renderAll();
      await flush();
      const days = done.report.changes.length;
      toast(`Orele din aplicație au fost aduse la Excel (${days} ${days === 1 ? "zi" : "zile"}). Ziua reglată se vede în pontajul fiecărui șofer, cu „Anulează reglarea”.`, { kind: "ok", timeout: 9000 });
    } catch (err) {
      fail(err);
    } finally {
      refreshing = false;
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

  /**
   * Before the one Excel file of a month goes out: the last edit is saved (the file is made from what is saved), and when this
   * is the Categoria B month being exported, the office is told what it should still look at. Resolves `false` when the
   * office goes back to check.
   */
  async function confirmExport(year, month) {
    await queue; // let a pending edit finish
    await flush();
    const warning = state.sheet && state.year === year && state.month === month ? exportWarning() : null;
    if (!warning) return true;
    const go = await showDialog({
      title: "Înainte de export",
      body: warning,
      wide: true,
      buttons: [{ label: "Înapoi la verificare", value: false }, { label: "Exportă oricum", kind: "primary", value: true }],
    });
    return Boolean(go);
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
    if (state.loaded) {
      renderAll();
      return refreshIfDue(); // back on the tab after a while: it may be due
    }
    state.loaded = true;
    watchTyping();
    setInterval(refreshIfDue, REFRESH_TICK_MS);
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
  document.addEventListener("visibilitychange", () => (document.hidden ? flush() : refreshIfDue()));

  return { show, hide, fetchMonth, confirmExport, settingsChanged, flush, month: () => ({ year: state.year, month: state.month }) };
}
