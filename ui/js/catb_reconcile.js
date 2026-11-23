// "Reglează după Excel", the part the office sees: what the app would change to follow the Excel file, before anything is
// changed. The file is read and compared in Rust (`catb_reconcile`); this only shows the report and asks for the go-ahead.
import { h } from "./dom.js";
import { showDialog } from "./dialog.js";
import { shortDate, weekdayName } from "./time.js";

const MONTH_NAMES = ["ianuarie", "februarie", "martie", "aprilie", "mai", "iunie", "iulie", "august", "septembrie", "octombrie", "noiembrie", "decembrie"];

/** The items of `list` in groups by `keyOf(item)`, in the order the keys first appear. */
function groupBy(list, keyOf) {
  const groups = new Map();
  for (const item of list) {
    const key = keyOf(item);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(item);
  }
  return [...groups];
}

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
const dayText = (date) => (date ? `${weekdayName(date).slice(0, 3)} ${shortDate(date)}` : "");
const hoursText = (text) => (text === "" ? "—" : text);

/**
 * One table of days grouped by driver: a line with the name, then a line for each day.
 * `columns` = [heading, cell(item), left?] (`left`: text that is aligned to the left and may wrap, like a reason).
 */
function groupedTable(items, columns) {
  const rows = groupBy(items, (item) => item.driver).flatMap(([driver, days]) => [
    h("tr", { class: "group" }, h("td", { colspan: columns.length + 1 }, h("b", {}, driver), h("span", { class: "muted" }, ` · ${plural(days.length, "zi", "zile")}`))),
    ...days.map((item) => h("tr", {}, h("td", { class: "left" }, dayText(item.date)), columns.map(([, cell, left]) => h("td", { class: left ? "left" : "" }, cell(item))))),
  ]);
  return h("table", { class: "data-table reconcile-table" },
    h("thead", {}, h("tr", {}, ["Ziua", ...columns.map(([heading]) => heading)].map((t) => h("th", {}, t)))),
    h("tbody", {}, rows));
}

function reportBody(report) {
  const parts = [
    h("p", {}, `Excel-ul este pe ${MONTH_NAMES[report.month - 1]} ${report.year}. ${plural(report.matched, "șofer din aplicație a fost găsit", "șoferi din aplicație au fost găsiți")} în el.`),
  ];
  // The trips come first: the hours of the days below follow from them.
  if (report.trips.length) {
    parts.push(
      h("p", {}, h("b", {}, plural(report.trips.length, "cursă își schimbă", "curse își schimbă")), " plecarea, sosirea sau locurile în aplicație, ca să fie ca în Excel (foaia „Detaliu”):"),
      groupedTable(report.trips, [["Cursa", (t) => t.kind, true], ["Acum în aplicație", (t) => (t.app === "" ? "—" : t.app), true], ["În Excel", (t) => h("b", {}, t.excel), true]]));
  }
  if (report.changes.length) {
    parts.push(
      h("p", {}, h("b", {}, `${plural(report.changes.length, "zi se schimbă", "zile se schimbă")}`), " în aplicație, ca să arate ca în Excel (acolo unde Excel-ul are mai puține ore decât curse, ziua rămâne cu cursele și primește o reglare negativă):"),
      groupedTable(report.changes, [["Acum în aplicație", (c) => hoursText(c.app)], ["În Excel", (c) => h("b", {}, hoursText(c.excel))]]));
  } else if (!report.trips.length) {
    parts.push(h("p", {}, h("b", {}, "Orele și cursele din aplicație sunt deja la fel ca în Excel."), " Nu se schimbă nimic."));
  }
  if (report.skipped.length) {
    parts.push(
      h("p", {}, h("b", {}, plural(report.skipped.length, "zi nu se poate aduce", "zile nu se pot aduce")), " la ce scrie în Excel; ele rămân ca în aplicație:"),
      groupedTable(report.skipped, [["În Excel", (s) => s.excel], ["De ce nu", (s) => s.reason, true]]));
  }
  if (report.tripsSkipped.length) {
    parts.push(
      h("p", {}, h("b", {}, plural(report.tripsSkipped.length, "observație", "observații")), " despre curse: ce din Excel nu se potrivește cu aplicația (o cursă din Excel fără pereche în aplicație, sau una din aplicație fără pereche în Excel, rămâne cum este):"),
      groupedTable(report.tripsSkipped, [["În Excel", (s) => s.excel, true], ["Observație", (s) => s.reason, true]]));
  }
  if (report.notInFile.length) {
    parts.push(h("p", { class: "muted" }, `${plural(report.notInFile.length, "șofer din aplicație nu este", "șoferi din aplicație nu sunt")} în Excel, deci orele lor rămân neschimbate: ${report.notInFile.join(", ")}.`));
  }
  if (report.others) {
    parts.push(h("p", { class: "muted" }, `${plural(report.others, "persoană din Excel nu este", "persoane din Excel nu sunt")} șofer la Categoria B (tahograf, birou…): nu se ating.`));
  }
  return h("div", { class: "reconcile" }, parts);
}

/**
 * Shows what following the Excel would change and asks. Resolves `true` when the office accepts, `false` otherwise.
 * @param {object} report the `report` of `catb_reconcile`
 */
export async function askReconcile(report) {
  const nothing = !report.changes.length && !report.trips.length;
  const go = await showDialog({
    title: "Reglează orele după Excel",
    body: reportBody(report),
    wide: true,
    buttons: nothing
      ? [{ label: "Închide", kind: "primary", value: false }]
      : [{ label: "Renunță", value: false }, { label: `Aduce ${[report.changes.length && plural(report.changes.length, "zi", "zile"), report.trips.length && plural(report.trips.length, "cursă", "curse")].filter(Boolean).join(" și ")} la Excel`, kind: "primary", value: true }],
  });
  return Boolean(go);
}
