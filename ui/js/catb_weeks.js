// Categoria B — the weeks of one driver: blocks of 7 days counted from the 1st of the month, each with the time the driver was
// active on trips and the days without work. A week over the limit of hours, or without the free days it needs, is marked, and
// every worked day has a button that turns it into a free day (takes it out of the pontaj). The numbers and the remarks come
// from Rust (`driver.weeks`); this module only draws them and reports the click.
import { h } from "./dom.js";
import { duration, shortDate, weekdayName } from "./time.js";

const HOW = "Ziua nu mai e în pontaj (fără ore și fără plată) și se numără ca zi liberă. Cursele ei rămân în listă și o poți pune la loc oricând.";

/** What each kind of day says about itself, and the button it may have: [text, button label, button title]. */
const DAY_STATES = {
  worked: (day) => [duration(day.tripMin), "Fă liberă", `${duration(day.tripMin)} în cursă. ${HOW}`],
  out: (day) => ["scoasă", "Pune la loc", `Ziua (${duration(day.tripMin)} în cursă) e scoasă din pontaj și se numără ca zi liberă. Apasă ca să aibă din nou ore.`],
  tachograph: () => ["tahograf", null, "A lucrat, dar excursia e pe tahograf: ziua nu are ore aici ca să le scoți."],
  free: (day) => [day.leave ?? "liberă", null, day.leave ? `Zi liberă, cu concediu (${day.leave})` : "Zi fără nicio cursă în pontaj"],
};

/** One day of a week: its name, what it is, and (worked / taken out) the button that switches it. */
function dayCard(day, { info, onToggle }) {
  const [text, button, title] = DAY_STATES[day.state](day);
  return h("div", { class: `week-day ${day.state} ${info(day.date)?.weekend ? "off" : ""} ${info(day.date)?.holiday ? "holiday" : ""}`, title: info(day.date)?.holiday ?? title },
    h("div", { class: "week-day-line" }, h("b", {}, `${weekdayName(day.date).slice(0, 3)} ${shortDate(day.date)}`), h("small", {}, text)),
    button ? h("button", { class: `btn small ${day.state === "worked" ? "" : "ghost"}`, type: "button", title, onclick: () => onToggle(day.date) }, button) : null);
}

/** A figure of the week; `tone` "bad" paints it as a problem. */
const stat = (text, tone, title) => h("span", { class: `week-stat ${tone}`, title }, text);

/**
 * One week. A week to look at (over the hours, short of free days) or with a day taken out shows its days, each with its
 * button; any other week is a single line (the days of a quiet week are in the list below).
 */
function weekBlock(week, ctx) {
  const bad = week.tooLong || week.fewFree;
  const expanded = bad || week.days.some((day) => day.state === "out");
  const hours = week.limitActiveMin > 0 ? `${duration(week.activeMin)} / ${duration(week.limitActiveMin)}` : duration(week.activeMin);
  const free = week.full && week.needFreeDays > 0 ? `${week.freeCount} / ${week.needFreeDays}` : String(week.freeCount);
  const head = h("div", { class: "week-head" },
    h("b", {}, `${shortDate(week.from)} – ${shortDate(week.to)}`, week.full ? null : h("small", { class: "muted" }, ` · ${week.days.length} zile`)),
    stat(`${hours} ore în cursă`, week.tooLong ? "bad" : "", "Doar timpul în cursă: fără staționare și fără orele adăugate zilelor; zilele scoase din pontaj nu se numără. Limita se schimbă în Reguli → Categoria B."),
    stat(`${free} zile libere`, week.fewFree ? "bad" : "", week.full ? "Zile fără ore în pontaj (concediile și zilele scoase din pontaj sunt libere). Numărul cerut se schimbă în Reguli → Categoria B." : "Săptămâna e mai scurtă de 7 zile (se termină luna): zilele libere nu se verifică aici."));
  return h("div", { class: `week ${bad ? "bad" : ""} ${expanded ? "" : "compact"}` }, head, expanded ? h("div", { class: "week-days" }, week.days.map((day) => dayCard(day, ctx))) : null);
}

/** Panels the office opened or closed by hand (driver id → open), so a redraw after an edit does not undo it. */
const chosen = new Map();

/**
 * The weeks of `driver`. `ctx`: { info(date) → calendar entry of the day (weekend, holiday), onToggle(date) → called when the
 * button of a worked day (make it free) or of a taken-out day (put it back) is pressed }. Open by itself when a week has
 * something to look at.
 */
export function weeksPanel(driver, ctx) {
  const weeks = driver.weeks ?? [];
  if (!weeks.length) return null;
  const flagged = weeks.filter((w) => w.tooLong || w.fewFree).length;
  const open = chosen.get(driver.id) ?? flagged > 0;
  const panel = h("details", { class: `weeks ${flagged ? "flagged" : ""}`, open, ontoggle: (e) => chosen.set(driver.id, e.target.open) },
    h("summary", {},
      h("b", {}, "Săptămâni"),
      h("span", { class: flagged ? "flag warn" : "muted" }, flagged ? ` · ${flagged} de verificat` : " · în regulă"),
      h("small", { class: "muted" }, " (câte 7 zile de la începutul lunii)")),
    h("div", { class: "weeks-body" }, weeks.map((week) => weekBlock(week, ctx))));
  return panel;
}
