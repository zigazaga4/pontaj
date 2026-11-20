//! Rebuilds everything derived from a driver's trips and the office's edits: the trips as they count, the pauses
//! between regular trips (waiting time), the days, the totals, the estimate and the remarks.
//!
//! Rules (all numbers come from [`CatbSettings`]):
//! - A day's hours are the known driving time of its trips plus the waiting time of the pauses that start that day, plus
//!   `fill_per_trip_min` (1 h) for every regular trip of the day: half before the first departure and half after the last
//!   arrival. One trip of 3 h makes a day of 4 h, the usual two make 8 h (1 h + 1 h around them). The generated drives are
//!   trips like any other. The time added never takes a day past `hours_per_day` (8 h): a day that already has 8 h gets nothing.
//! - Waiting is only measured between two *regular* trips (Constanța ↔ Otopeni; private/VIP trips on the same route
//!   count as regular, and so does any private/VIP "Otopeni run": a trip with the Otopeni/Bucharest end on one side and
//!   any place on the other, even outside the county, e.g. Otopeni → Năvodari, Mamaia → Otopeni, Tulcea → Otopeni).
//!   It is inferred from the API (next departure − previous arrival), never read from it.
//! - The time between two regular trips is either a *staționare* or a *pauză*, by where the driver is. At Otopeni (the
//!   Bucharest side) he waits: a staționare (`Normal`), which counts, unless it lasts `long_pause_min` or more. In Constanța
//!   (or anywhere else) he is at home: a pauză (`Rest`), which counts nothing, like any break of `long_pause_min` or more.
//!   The office can switch either one to the other.
//! - A trip beyond Constanța county with no tariff is an excursion by itself (the tachograph has it): it counts nowhere
//!   here and its pause is a pauză. With a tariff it is a paid trip like any other.
//! - The estimate is `pair_pay / 2` per regular leg, the exterior tariff for the other trips, and `station_rate` for every
//!   started hour of waiting beyond `station_included_min` (started = from `round_up_from_min`, like the firm's hours).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, NaiveDate};

use super::config::CatbSettings;
use super::model::*;
use super::{cap, hidden, places, tariffs, transfers, weeks};
use crate::calendar;
use crate::settings::Settings;
use crate::time::{fmt_duration, round_hours};
use crate::timesheet::month_days;

/// Recomputes the whole sheet.
pub fn recompute(sheet: &mut Sheet, settings: &Settings) {
    // Drivers taken out of the pontaj leave the sheet (and come back when they leave the list) before anything is counted.
    hidden::apply(sheet, &settings.catb.hidden_drivers);
    sheet.days = month_days(sheet.year, sheet.month);
    sheet.weekdays = calendar::weekdays_in_month(sheet.year, sheet.month);
    sheet.cap_min = monthly_cap_min(sheet.year, sheet.month, settings);
    let (year, month) = (sheet.year, sheet.month);
    for driver in &mut sheet.drivers {
        recompute_driver(driver, year, month, settings);
    }
    sheet.drivers.sort_by_cached_key(|d| d.name.to_lowercase());
}

/// The most a driver may be booked in a month: `hours_per_day` × the Mon–Fri days, legal holidays included.
pub fn monthly_cap_min(year: i32, month: u32, settings: &Settings) -> i32 {
    calendar::weekdays_in_month(year, month) as i32 * settings.hours_per_day * 60
}

/// Recomputes one driver (what the screen does after each edit).
pub fn recompute_driver(driver: &mut Driver, year: i32, month: u32, settings: &Settings) {
    let cfg = &settings.catb;
    for trip in &mut driver.trips {
        trip.eff = effective(trip, cfg);
    }
    transfers::sync(driver, year, month, settings);
    driver.trips.sort_by_key(|t| (t.eff.departure, t.id.clone()));
    driver.free_days = free_days(&driver.trips, year, month);
    migrate_pause_leaves(driver);
    driver.pauses = pauses(&driver.trips, &driver.pause_edits, settings);
    assign_work_days(driver);
    driver.days = days(driver, year, month, settings);
    super::pontaj::mark_day_edges(driver);
    let cap_min = monthly_cap_min(year, month, settings);
    driver.totals = totals(&driver.days, cap_min);
    driver.weeks = weeks::build(driver, year, month, cfg);
    driver.alerts = driver_alerts(driver, cap_min);
    driver.proposals = cap::propose(driver, cap_min);
}

pub(super) fn effective(trip: &Trip, cfg: &CatbSettings) -> Effective {
    let departure = trip.edit.departure.unwrap_or(trip.departure);
    let arrival = trip.edit.arrival.or(trip.arrival);
    let from = trip.edit.from.clone().unwrap_or_else(|| trip.from.clone());
    let to = trip.edit.to.clone().unwrap_or_else(|| trip.to.clone());
    let minutes = arrival.map(|a| (a - departure).num_minutes()).filter(|m| *m >= 0).unwrap_or(0) as i32;
    let regular = match trip.kind {
        TripKind::Eco | TripKind::Passenger | TripKind::Transfer => true,
        TripKind::Private | TripKind::Vip => cfg.main_route_private_regular && places::is_regular_route(&from, &to),
        TripKind::Bus => false,
    };
    let (mut price, mut price_note) = price_of(trip, regular, &from, &to, cfg);
    let mut counts_hours = regular || cfg.count_other_trips;
    // A trip beyond the county with no tariff (and no price typed by hand) is an excursion: the tachograph has it, so it
    // leaves Categoria B by itself. Adding a tariff for the place (Rules) or typing a price brings it back as a paid trip.
    let excursion = !regular && price_note == NO_TARIFF && is_far_route(&from, &to, cfg);
    if excursion {
        counts_hours = false;
        price = 0.0;
        price_note = EXCURSION_NOTE.into();
    }
    Effective { departure: Some(departure), arrival, from, to, minutes, regular, counts_hours, excursion, price, price_note, work_day: Some(departure.date()), ..Default::default() }
}

/// Is the pause part of one shift with the trips on both sides of it? A staționare (waiting that counts) that goes over at
/// most one midnight is: the driver did not go home between the two trips.
fn same_shift(pause: &Pause) -> bool {
    let (Some(start), Some(end)) = (pause.start, pause.end) else { return false };
    is_station(pause) && (end.date() - start.date()).num_days() <= 1
}

/// Is the trip a leg toward Bucharest/Otopeni (it leaves from somewhere else)?
fn goes_out(eff: &Effective) -> bool {
    places::is_bucharest(&eff.to) && !places::is_bucharest(&eff.from)
}

/// Is the trip a leg back from Bucharest/Otopeni (it arrives somewhere else)?
fn comes_back(eff: &Effective) -> bool {
    places::is_bucharest(&eff.from) && !places::is_bucharest(&eff.to)
}

/// Does the trip after the pause count on the day of the trip before it? Yes when it is the way back of a trip to
/// Bucharest/Otopeni: the way back is always put on the day of the way there, even when by the clock it is the next day
/// (09:00–12:00 and then 01:00–04:00), as long as it leaves on that day or the next one. How long the wait was and what the
/// office chose for it (staționare or pauză) do not matter. Or when the wait is a staționare over at most one midnight (a
/// trip inside Bucharest between the two, for example).
fn same_outing(pause: &Pause, day_before: NaiveDate) -> bool {
    let way_back = pause.round_trip && pause.end.is_some_and(|end| (end.date() - day_before).num_days() <= 1);
    way_back || same_shift(pause)
}

/// Gives every trip and every pause the day it counts on. The trips are sorted by departure and a pause always comes after
/// the trip before it, so one pass in order is enough: a trip that is the way back of the trip before it (or follows a
/// staționare going over midnight) takes the day of that trip (the accountants want "left 14:30, arrived 17:30, left again
/// 03:30, arrived 06:30" on the one day the outing started), the others their own departure day. A pause counts on the day
/// of the trip before it when it is part of the outing, else on the day it starts.
fn assign_work_days(driver: &mut Driver) {
    let mut counted_on: std::collections::HashMap<String, NaiveDate> = std::collections::HashMap::new();
    let after: std::collections::HashMap<&str, &Pause> = driver.pauses.iter().map(|p| (p.next.as_str(), p)).collect();
    for trip in &mut driver.trips {
        let own = trip.eff.departure.map(|at| at.date());
        let shared = after.get(trip.id.as_str()).and_then(|pause| counted_on.get(&pause.prev).copied().filter(|day| same_outing(pause, *day)));
        trip.eff.work_day = shared.or(own);
        if let Some(day) = trip.eff.work_day {
            counted_on.insert(trip.id.clone(), day);
        }
    }
    for pause in &mut driver.pauses {
        let shared = counted_on.get(&pause.prev).copied().filter(|day| same_outing(pause, *day));
        pause.day = shared.or(pause.start.map(|start| start.date()));
    }
}

/// Note shown when a trip has no tariff: the office types the amount by hand.
pub const NO_TARIFF: &str = "fără tarif";

/// Note of a trip that belongs to an excursion.
pub const EXCURSION_NOTE: &str = "excursie: se pontează din tahograf";

fn price_of(trip: &Trip, regular: bool, from: &str, to: &str, cfg: &CatbSettings) -> (f64, String) {
    if let Some(price) = trip.edit.price {
        return (price, "preț trecut manual".into());
    }
    if regular {
        return (cfg.leg_pay(), if trip.generated { "deplasare adăugată de aplicație" } else { "cursă" }.into());
    }
    // The exterior place is the end of the trip that is not on the regular route.
    let outside: Vec<&str> = [to, from].into_iter().filter(|p| !p.is_empty() && !places::is_main_route(p)).collect();
    if outside.is_empty() {
        return (0.0, "pe ruta principală".into());
    }
    match outside.iter().find_map(|p| tariffs::find(&cfg.tariffs, p)) {
        Some(tariff) => (tariff.amount, format!("exterior: {}", tariff.name)),
        None => (0.0, NO_TARIFF.into()),
    }
}

fn pauses(trips: &[Trip], edits: &[PauseEdit], settings: &Settings) -> Vec<Pause> {
    let cfg = &settings.catb;
    let regular: Vec<usize> = (0..trips.len()).filter(|&i| !trips[i].edit.removed && trips[i].eff.regular).collect();
    let mut out = Vec::new();
    for pair in regular.windows(2) {
        let (a, b) = (&trips[pair[0]], &trips[pair[1]]);
        let (Some(start), Some(end)) = (a.eff.arrival, b.eff.departure) else { continue };
        // Trips sorted by departure: whatever lies between the two indexes happened between the two regular trips.
        let between: Vec<&Trip> = trips[pair[0] + 1..pair[1]].iter().filter(|t| !t.edit.removed && !t.eff.regular).collect();
        let gap_min = (end - start).num_minutes() as i32;
        let busy_min = busy_minutes(start, end, &between);
        let mut pause = Pause { prev: a.id.clone(), next: b.id.clone(), place_from: a.eff.to.clone(), place_to: b.eff.from.clone(), start: Some(start), end: Some(end), gap_min, busy_min, ..Default::default() };
        if let Some(edit) = edits.iter().find(|e| e.prev == a.id && e.next == b.id) {
            pause.state = edit.state;
            pause.hours_override = edit.hours;
            pause.note = edit.note.clone();
        }
        // He waits (staționare) only at Otopeni; in Constanța he is at home (pauză). A far trip without a tariff is already an
        // excursion (see `effective`), so the pause around it is out too.
        let excursion_inside = between.iter().any(|t| t.eff.excursion);
        // A drive made by the app that the office moved to another day stays where it was put (see `Trip::moved_days`).
        pause.round_trip = goes_out(&a.eff) && comes_back(&b.eff) && !excursion_inside && a.moved_days == 0 && b.moved_days == 0;
        pause.kind = if gap_min < 0 {
            PauseKind::Overlap
        } else if excursion_inside || gap_min >= cfg.long_pause_min || !places::is_bucharest(&pause.place_from) {
            PauseKind::Rest
        } else {
            PauseKind::Normal
        };
        pause.counted_min = counted_minutes(&pause);
        pause.extra_pay = station_pay(pause.counted_min, settings);
        pause.alerts = pause_alerts(&pause, &between, settings);
        out.push(pause);
    }
    out
}

/// The days of the month on which the driver has no trip at all (a trip that runs past midnight takes both days; a
/// removed trip takes none): the days the office can mark as leave.
fn free_days(trips: &[Trip], year: i32, month: u32) -> Vec<NaiveDate> {
    let busy: BTreeSet<NaiveDate> = trips.iter().filter(|t| !t.edit.removed).flat_map(trip_days).collect();
    (1..=calendar::days_in_month(year, month)).filter_map(|d| NaiveDate::from_ymd_opt(year, month, d)).filter(|day| !busy.contains(day)).collect()
}

/// The calendar days a trip takes, from its departure to its arrival (a trip that runs past midnight takes both days; one
/// with no known arrival takes the day it leaves).
pub(super) fn trip_days(trip: &Trip) -> Vec<NaiveDate> {
    let Some(from) = trip.eff.departure else { return Vec::new() };
    let to = trip.eff.arrival.filter(|arrival| *arrival >= from).unwrap_or(from);
    from.date().iter_days().take_while(|day| *day <= to.date()).collect()
}

/// Months saved by 0.3.11–0.3.12 chose a leave for a whole pause: it becomes a leave on each free working day of that pause.
fn migrate_pause_leaves(driver: &mut Driver) {
    if driver.pause_edits.iter().all(|edit| edit.leave.is_none()) {
        return;
    }
    let Driver { trips, pause_edits, free_days, leaves, .. } = driver;
    for edit in pause_edits.iter_mut() {
        let Some(code) = edit.leave.take() else { continue };
        let from = trips.iter().find(|t| t.id == edit.prev).and_then(|t| t.eff.arrival);
        let to = trips.iter().find(|t| t.id == edit.next).and_then(|t| t.eff.departure);
        let (Some(from), Some(to)) = (from, to) else { continue };
        if !LEAVE_TYPES.iter().any(|(known, _)| *known == code) {
            continue;
        }
        for day in free_days.iter().filter(|d| **d > from.date() && **d < to.date() && !calendar::is_weekend(**d) && calendar::holiday_name(**d).is_none()) {
            leaves.entry(*day).or_insert_with(|| code.clone());
        }
    }
    pause_edits.retain(|edit| edit.state != PauseState::Auto || edit.hours.is_some() || !edit.note.is_empty());
}

/// Does the route go beyond the places of an ordinary day (the regular route and Constanța county)?
fn is_far_route(from: &str, to: &str, cfg: &CatbSettings) -> bool {
    [from, to].into_iter().any(|place| !place.trim().is_empty() && !places::is_local(place, &cfg.extra_local_places))
}

/// Minutes of `start..end` taken by the known driving time of other trips (overlaps counted once). They are already
/// in the hours as trip time, so they must not be counted a second time as waiting.
fn busy_minutes(start: chrono::NaiveDateTime, end: chrono::NaiveDateTime, between: &[&Trip]) -> i32 {
    let mut spans: Vec<(chrono::NaiveDateTime, chrono::NaiveDateTime)> = between
        .iter()
        .filter(|t| t.eff.counts_hours)
        .filter_map(|t| Some((t.eff.departure?.max(start), t.eff.arrival?.min(end))))
        .filter(|(from, to)| to > from)
        .collect();
    spans.sort();
    let mut total = 0;
    let mut covered_until = start;
    for (from, to) in spans {
        let from = from.max(covered_until);
        if to > from {
            total += (to - from).num_minutes() as i32;
            covered_until = to;
        }
    }
    total
}

/// Is the pause a staționare (waiting that counts)? The office's choice wins, otherwise the rules decide.
pub(super) fn is_station(pause: &Pause) -> bool {
    match pause.state {
        PauseState::Stayed => true,
        PauseState::Off => false,
        PauseState::Auto => pause.kind == PauseKind::Normal,
    }
}

/// How much of a pause counts as waiting: nothing for a pauză, otherwise the hours typed by hand or the whole wait.
fn counted_minutes(pause: &Pause) -> i32 {
    if !is_station(pause) {
        return 0;
    }
    if let Some(hours) = pause.hours_override {
        return (hours.max(0.0) * 60.0).round() as i32;
    }
    (pause.gap_min.max(0) - pause.busy_min).max(0)
}

/// The amount added for waiting beyond the included time.
pub(super) fn station_pay(counted_min: i32, settings: &Settings) -> f64 {
    let extra = counted_min - settings.catb.station_included_min;
    if extra <= 0 { 0.0 } else { round_hours(extra, settings.round_up_from_min) as f64 * settings.catb.station_rate }
}

fn pause_alerts(pause: &Pause, between: &[&Trip], settings: &Settings) -> Vec<Alert> {
    let cfg = &settings.catb;
    let mut alerts = Vec::new();
    if pause.kind == PauseKind::Overlap {
        alerts.push(Alert::new("overlap", Level::Alert, format!("Cursele se suprapun cu {}.", fmt_duration(-pause.gap_min))));
        return alerts;
    }
    if between.iter().any(|t| t.eff.excursion) {
        let place = between.iter().find(|t| t.eff.excursion).or(between.first()).map_or_else(|| pause.place_to.clone(), |t| t.eff.to.clone());
        let dates = match (pause.start, pause.end) {
            (Some(start), Some(end)) => format!(", {} – {}", start.format("%d.%m"), end.format("%d.%m")),
            _ => String::new(),
        };
        alerts.push(Alert::new("excursion_done", Level::Info, format!("Excursie ({place}{dates}): nu intră în Categoria B, se pontează din tahograf. Pauza și cursele din ea nu se numără aici.")));
    }
    if pause.counted_min >= cfg.station_warn_min {
        alerts.push(Alert::new(
            "station_long",
            Level::Warn,
            format!(
                "Staționare de {} (de la {} în sus): probabil a plecat și s-a întors mai târziu. Verifică foaia șoferului.",
                fmt_duration(pause.counted_min),
                fmt_duration(cfg.station_warn_min)
            ),
        ));
    }
    let undecided = pause.state == PauseState::Auto && pause.hours_override.is_none();
    let differs = !places::same_area(&pause.place_from, &pause.place_to, &cfg.extra_local_places);
    // A pauză (he is at home, or a day off between shifts) is not worth a remark, even if he came back some other way.
    if differs && undecided && pause.kind == PauseKind::Normal {
        let text = format!("A ajuns în {}, iar următoarea cursă pleacă din {}: probabil s-a întors ca pasager.", pause.place_from, pause.place_to);
        alerts.push(Alert::new("place_differs", Level::Warn, format!("Loc diferit între curse. {text}")));
    }
    alerts
}

/// The row of a day, created on first use. Days outside the month get no row (they belong to the other month's sheet).
fn day_row(rows: &mut BTreeMap<NaiveDate, DayRow>, date: NaiveDate, year: i32, month: u32) -> Option<&mut DayRow> {
    let inside = date.year() == year && date.month() == month;
    inside.then(|| rows.entry(date).or_insert_with(|| DayRow { date: Some(date), ..Default::default() }))
}

/// How the time added to a day is split: half before the first departure, half after the last arrival (in whole 5 minutes).
/// The part that does not fit before the first departure (the day starts too close to midnight) goes after the last arrival.
pub(super) fn split_fill(total: i32, room_before: i32) -> (i32, i32) {
    let total = total.max(0);
    let before = (total / 2 / 5 * 5).min(room_before.max(0));
    (before, total - before)
}

/// What the rules add to a day with `legs` regular trips and `work_min` of time on trips (waiting is not work time):
/// `fill_per_trip_min` for every trip, but never past a full day (`hours_per_day`): one trip of 3 h makes 4 h, two trips of
/// 3 h make 8 h, and a day that is already full (or more) gets nothing.
fn auto_fill_min(legs: i32, work_min: i32, settings: &Settings) -> i32 {
    let cfg = &settings.catb;
    if !cfg.fill_day {
        return 0;
    }
    (legs.max(0) * cfg.fill_per_trip_min.max(0)).min(settings.hours_per_day * 60 - work_min).max(0)
}

/// The time added to a day with `legs` regular trips and `work_min` of time on trips: the rules' split (when the setting is
/// on), with whatever the office typed for either side on top. Nothing is added to a day without trips.
fn fill_of(driver: &Driver, date: NaiveDate, legs: i32, work_min: i32, settings: &Settings) -> (i32, i32, bool) {
    if work_min <= 0 {
        return (0, 0, false);
    }
    let first_start = super::pontaj::day_span(driver, date).map_or(0, |(start, _)| start);
    let (auto_before, auto_after) = split_fill(auto_fill_min(legs, work_min, settings), first_start);
    let edit = driver.fills.get(&date);
    // Typed time before the first departure cannot start before midnight (that is the day before).
    let before = edit.and_then(|e| e.before).map_or(auto_before, |m| m.clamp(0, first_start.max(0)));
    let after = edit.and_then(|e| e.after).map_or(auto_after, |m| m.max(0));
    (before, after, edit.is_some_and(|e| e.before.is_some() || e.after.is_some()))
}

fn days(driver: &Driver, year: i32, month: u32, settings: &Settings) -> Vec<DayRow> {
    let mut rows: BTreeMap<NaiveDate, DayRow> = BTreeMap::new();

    // A trip belongs to its work day (the day it departs, or the day of the shift it continues), a pause to its own.
    for trip in driver.trips.iter().filter(|t| !t.edit.removed && !t.eff.excursion) {
        let eff = &trip.eff;
        let Some(row) = eff.work_day.and_then(|day| day_row(&mut rows, day, year, month)) else { continue };
        row.trips += 1;
        if eff.counts_hours {
            row.trip_min += eff.minutes;
        }
        if eff.regular {
            row.legs += 1;
            row.pay_legs += eff.price;
        } else {
            row.pay_exterior += eff.price;
        }
        if eff.price_note == NO_TARIFF {
            row.alerts.push(Alert::new("no_tariff", Level::Alert, format!("Cursă fără tarif: {} → {}. Trece prețul în dreptul cursei.", eff.from, eff.to)));
        }
        if trip.generated {
            let (Some(start), Some(end)) = (eff.departure, eff.arrival) else { continue };
            let text = format!(
                "Deplasare adăugată de aplicație (nu apare în API): {} → {}, {}–{}. Se numără ca o cursă regulată; o poți schimba sau scoate din listă.",
                eff.from,
                eff.to,
                start.format("%H:%M"),
                end.format("%H:%M")
            );
            row.alerts.push(Alert::new("transfer", Level::Info, text));
        }
    }
    for pause in &driver.pauses {
        let Some(row) = pause.day.and_then(|day| day_row(&mut rows, day, year, month)) else { continue };
        row.station_min += pause.counted_min;
        row.pay_station += pause.extra_pay;
    }
    // A leave is a day without any trip that the office marked.
    for (date, code) in driver.leaves.iter().filter(|(date, code)| driver.free_days.contains(date) && is_leave_code(code)) {
        if let Some(row) = day_row(&mut rows, *date, year, month) {
            row.leave = Some(code.clone());
        }
    }

    let mut list: Vec<DayRow> = rows.into_values().collect();
    for row in &mut list {
        if let Some(date) = row.date {
            (row.fill_before_min, row.fill_after_min, row.fill_manual) = fill_of(driver, date, row.legs, row.trip_min, settings);
        }
        row.fill_min = row.fill_before_min + row.fill_after_min;
        // The waiting between trips is shown (`station_min`) and paid by its own rule, but it is not work time: the hours of a
        // day are the time on the trips and the time added to them.
        row.total_min = row.trip_min + row.fill_min;
        // A day is not shortened when its trips alone pass a full day, but the office is told.
        let work_min = row.trip_min;
        if work_min > settings.hours_per_day * 60 {
            let text = format!(
                "Ziua are {} de curse, peste cele {} ore ale unei zile. Nu se completează nimic, dar verifică foaia șoferului: poate a plecat sau s-a întors altfel.",
                fmt_duration(work_min),
                settings.hours_per_day
            );
            row.alerts.push(Alert::new("long_day", Level::Warn, text));
        }
        row.pay_total = row.pay_legs + row.pay_exterior + row.pay_station;
        row.excluded = row.date.is_some_and(|d| driver.excluded_days.contains(&d));
    }
    list
}

fn is_leave_code(code: &str) -> bool {
    LEAVE_TYPES.iter().any(|(known, _)| *known == code)
}

fn totals(days: &[DayRow], cap_min: i32) -> Totals {
    let mut totals = Totals::default();
    for day in days.iter().filter(|d| !d.excluded) {
        totals.days += i32::from(day.trips > 0 || day.total_min > 0);
        totals.legs += day.legs;
        totals.trip_min += day.trip_min;
        totals.station_min += day.station_min;
        totals.fill_min += day.fill_min;
        totals.total_min += day.total_min;
        totals.pay_legs += day.pay_legs;
        totals.pay_exterior += day.pay_exterior;
        totals.pay_station += day.pay_station;
        totals.pay_total += day.pay_total;
        if let Some(code) = &day.leave {
            *totals.leave_days.entry(code.clone()).or_default() += 1;
        }
    }
    totals.over_min = (totals.total_min - cap_min).max(0);
    totals
}

fn driver_alerts(driver: &Driver, cap_min: i32) -> Vec<Alert> {
    let mut alerts = Vec::new();
    if driver.totals.over_min > 0 {
        alerts.push(Alert::new(
            "over_cap",
            Level::Alert,
            format!(
                "Depășește plafonul lunar cu {}: {} ore în pontaj, plafon {} ore.",
                fmt_duration(driver.totals.over_min),
                fmt_duration(driver.totals.total_min),
                fmt_duration(cap_min)
            ),
        ));
    }
    // A leave on a day that has a trip now (the API brought one after the office had chosen the leave) counts nowhere.
    let blocked: Vec<String> = driver.leaves.keys().filter(|date| !driver.free_days.contains(date)).map(|date| date.format("%d.%m").to_string()).collect();
    if !blocked.is_empty() {
        alerts.push(Alert::new("leave_blocked", Level::Warn, format!("Ai ales concediu pe {}, dar în {} există acum o cursă, deci concediul nu se numără. Scoate concediul sau cursa.", blocked.join(", "), if blocked.len() == 1 { "ziua aceea" } else { "zilele acelea" })));
    }
    alerts.extend(weeks::alerts(&driver.weeks));
    alerts
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::*;

    fn trip(id: &str, kind: TripKind, from: &str, to: &str, departure: &str, arrival: Option<&str>) -> Trip {
        Trip {
            id: id.into(),
            kind,
            from: from.into(),
            to: to.into(),
            departure: parse_time(departure).unwrap(),
            arrival: arrival.and_then(parse_time),
            ..Default::default()
        }
    }

    fn driver(trips: Vec<Trip>) -> Driver {
        Driver { id: 1, name: "TEST".into(), trips, ..Default::default() }
    }

    fn run(driver: &mut Driver) {
        recompute_driver(driver, 2026, 10, &Settings::default());
    }

    /// Constanța → Otopeni at 05:00 (3 h), back at `back_departure`.
    fn round_trip(back_departure: &str, back_arrival: &str) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", back_departure, Some(back_arrival)),
        ])
    }

    #[test]
    fn a_normal_day_is_two_legs_and_240_lei() {
        let mut d = round_trip("2026-10-05 10:00", "2026-10-05 13:00");
        run(&mut d);
        let day = &d.days[0];
        assert_eq!((day.legs, day.trip_min, day.station_min, day.total_min), (2, 360, 120, 480));
        assert_eq!((day.pay_legs, day.pay_station, day.pay_total), (240.0, 0.0, 240.0), "2 h of waiting are inside the 3 included hours");
        assert!(d.pauses[0].alerts.is_empty());
    }

    fn filled(d: &Driver) -> (i32, i32, i32) {
        (d.days[0].fill_before_min, d.days[0].fill_after_min, d.days[0].total_min)
    }

    fn date(text: &str) -> NaiveDate {
        parse_time(&format!("{text} 00:00")).unwrap().date()
    }

    /// Mon 5 Oct: Constanța → Otopeni 14:30–17:30; back from Otopeni at `back` (the next day or later), arriving 3 h later.
    fn evening_then_back(back: &str, back_arrival: &str) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 14:30", Some("2026-10-05 17:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", back, Some(back_arrival)),
        ])
    }

    #[test]
    fn the_trip_back_after_a_wait_over_midnight_counts_on_the_day_of_the_arrival() {
        // 14:30–17:30, the driver waits 10 h at Otopeni, leaves at 03:30 and arrives at 06:30 on the 6th: one shift, the
        // accountants write all of it on the 5th.
        let mut d = evening_then_back("2026-10-06 03:30", "2026-10-06 06:30");
        run(&mut d);
        assert_eq!(d.trips[1].eff.work_day, Some(date("2026-10-05")), "the trip leaves on the 6th");
        assert_eq!(d.pauses[0].day, Some(date("2026-10-05")));
        assert_eq!(d.days.len(), 1, "the 6th has no day of its own");
        let day = &d.days[0];
        assert_eq!(day.date, Some(date("2026-10-05")));
        assert_eq!((day.legs, day.trip_min, day.station_min, day.total_min), (2, 360, 600, 480), "6 h of trips and 2 h added; the 10 h of waiting are not hours");
        // The times of the day: the second trip starts after 24:00 (03:30 = 27:30).
        let trips = super::super::pontaj::trip_intervals(&d, date("2026-10-05"));
        assert_eq!(trips.iter().map(|i| (i.start, i.end)).collect::<Vec<_>>(), [(14 * 60 + 30, 17 * 60 + 30), (27 * 60 + 30, 30 * 60 + 30)]);
        assert!(super::super::pontaj::trip_intervals(&d, date("2026-10-06")).is_empty());
        // The 6th is not a day off: he drove that morning (no leave can be put on it).
        assert!(!d.free_days.contains(&date("2026-10-06")));
    }

    #[test]
    fn the_way_back_counts_on_the_day_of_the_way_there_whatever_the_wait_was() {
        // The accountants' rule: the trip that comes back is always on the day of the trip that left, even when by the clock
        // it is the next day. Here: 14:30–17:30, then 01:00–04:00 after a wait of 7 h 30 min.
        let mut d = evening_then_back("2026-10-06 01:00", "2026-10-06 04:00");
        run(&mut d);
        assert_eq!(d.trips[1].eff.work_day, Some(date("2026-10-05")));
        assert_eq!(d.days.iter().map(|r| r.date).collect::<Vec<_>>(), [Some(date("2026-10-05"))]);

        // A wait of 13 h is a pauză (over the long limit), and still the same day: 09:00–12:00, then 01:00–04:00.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 09:00", Some("2026-10-05 12:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-06 01:00", Some("2026-10-06 04:00")),
        ]);
        run(&mut d);
        assert_eq!(d.pauses[0].kind, PauseKind::Rest);
        assert!(d.pauses[0].round_trip);
        assert_eq!((d.trips[1].eff.work_day, d.pauses[0].day), (Some(date("2026-10-05")), Some(date("2026-10-05"))));
        assert_eq!(d.days.iter().map(|r| r.date).collect::<Vec<_>>(), [Some(date("2026-10-05"))]);
        assert_eq!((d.days[0].legs, d.days[0].trip_min, d.days[0].station_min, d.days[0].pay_total), (2, 360, 0, 240.0), "a pauză counts no waiting, the pair is still 240 lei");

        // What the office chose for the wait (staționare or pauză) does not move the trip back to another day.
        for state in [PauseState::Off, PauseState::Stayed, PauseState::Auto] {
            let mut d = evening_then_back("2026-10-06 03:30", "2026-10-06 06:30");
            d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state, ..Default::default() });
            run(&mut d);
            assert_eq!(d.trips[1].eff.work_day, Some(date("2026-10-05")), "{state:?}");
            assert_eq!(d.days.iter().map(|r| r.date).collect::<Vec<_>>(), [Some(date("2026-10-05"))], "{state:?}");
        }
        // … nor the hours typed for it, nor a note.
        let mut d = evening_then_back("2026-10-06 03:30", "2026-10-06 06:30");
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Off, hours: Some(0.0), note: "a dormit".into(), ..Default::default() });
        run(&mut d);
        assert_eq!(d.trips[1].eff.work_day, Some(date("2026-10-05")));
    }

    #[test]
    fn a_trip_that_is_not_a_way_back_keeps_its_own_day() {
        // Two ways there in a row: the second one is not a way back.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 09:00", Some("2026-10-05 12:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-06 01:00", Some("2026-10-06 04:00")),
        ]);
        run(&mut d);
        // (The app adds the drive back from Otopeni between them; that one is the way back of the first trip.)
        let second = d.pauses.iter().find(|p| p.next == "eco-2").unwrap();
        assert!(!second.round_trip);
        assert_eq!(d.trips.iter().find(|t| t.id == "eco-2").and_then(|t| t.eff.work_day), Some(date("2026-10-06")));

        // A way back two days later is not "the next day": it counts where it leaves.
        let mut d = evening_then_back("2026-10-07 01:00", "2026-10-07 04:00");
        run(&mut d);
        assert!(d.pauses[0].round_trip);
        assert_eq!(d.trips[1].eff.work_day, Some(date("2026-10-07")));

        // An excursion between the two: the driver went elsewhere, so each trip stays on its own day.
        let mut d = evening_then_back("2026-10-06 18:00", "2026-10-06 21:00");
        d.trips.push(trip("priv-1", TripKind::Private, "Constanta", "Brasov", "2026-10-06 06:00", Some("2026-10-06 12:00")));
        run(&mut d);
        assert!(d.trips.iter().find(|t| t.id == "priv-1").is_some_and(|t| t.eff.excursion), "a far trip with no tariff is an excursion");
        assert!(!d.pauses[0].round_trip);
        assert_eq!(d.trips.iter().find(|t| t.id == "eco-2").and_then(|t| t.eff.work_day), Some(date("2026-10-06")));
    }

    #[test]
    fn a_pair_after_a_pair_keeps_its_own_days() {
        // 14:30–17:30 there, 01:00–04:00 back (counts on the 5th); then the next pair on the 6th at 14:30 is a new day.
        let mut d = evening_then_back("2026-10-06 01:00", "2026-10-06 04:00");
        d.trips.push(trip("eco-3", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-06 14:30", Some("2026-10-06 17:30")));
        d.trips.push(trip("eco-4", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-07 01:00", Some("2026-10-07 04:00")));
        run(&mut d);
        let days: Vec<_> = d.trips.iter().map(|t| t.eff.work_day).collect();
        assert_eq!(days, [Some(date("2026-10-05")), Some(date("2026-10-05")), Some(date("2026-10-06")), Some(date("2026-10-06"))]);
    }

    #[test]
    fn a_wait_inside_the_day_changes_nothing_and_a_wait_over_two_midnights_is_not_one_shift() {
        let mut same_day = round_trip("2026-10-05 10:00", "2026-10-05 13:00");
        run(&mut same_day);
        assert_eq!(same_day.trips.iter().map(|t| t.eff.work_day).collect::<Vec<_>>(), [Some(date("2026-10-05")); 2]);
        // The office marked a 30 h wait as a staționare: it is long, the trip back is two days later and counts there.
        let mut long = evening_then_back("2026-10-07 00:00", "2026-10-07 03:00");
        long.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, hours: Some(5.0), ..Default::default() });
        run(&mut long);
        assert_eq!(long.trips[1].eff.work_day, Some(date("2026-10-07")));
    }

    #[test]
    fn a_shift_with_a_third_trip_chains_on_the_first_day() {
        // 14:30–17:30 to Otopeni, waits, 03:30–06:30 back (the 6th), then home: the 6th's afternoon is a new shift.
        let mut d = evening_then_back("2026-10-06 03:30", "2026-10-06 06:30");
        d.trips.push(trip("eco-3", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-06 14:30", Some("2026-10-06 17:30")));
        run(&mut d);
        let days: Vec<_> = d.trips.iter().map(|t| t.eff.work_day).collect();
        assert_eq!(days, [Some(date("2026-10-05")), Some(date("2026-10-05")), Some(date("2026-10-06"))]);
    }

    #[test]
    fn a_day_under_eight_hours_gets_the_missing_time_before_the_first_departure_and_after_the_last_arrival() {
        // Two legs of 3 h that meet at Otopeni: 6 h, 2 h missing → 1 h before 05:00 and 1 h after 11:00.
        let mut d = round_trip("2026-10-05 08:00", "2026-10-05 11:00");
        run(&mut d);
        assert_eq!(filled(&d), (60, 60, 480));
        assert_eq!((d.days[0].fill_min, d.days[0].trip_min, d.days[0].station_min, d.days[0].pay_total), (120, 360, 0, 240.0), "the money does not change");
        assert_eq!((d.totals.fill_min, d.totals.total_min, d.totals.trip_min), (120, 480, 360));
        assert_eq!(super::super::pontaj::day_span(&d, d.days[0].date.unwrap()), Some((5 * 60, 11 * 60)), "the span of the trips alone");
    }

    #[test]
    fn every_regular_trip_adds_an_hour_to_its_day_half_before_and_half_after() {
        // One trip of 3 h: 1 h added, 30 min on each side: a day of 4 h.
        let mut d = driver(vec![trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 09:00", Some("2026-10-05 12:00"))]);
        run(&mut d);
        assert_eq!(filled(&d), (30, 30, 4 * 60));
        // Two trips (the usual day): 2 h added, 1 h on each side: 8 h. (Checked above: `a_day_under_eight_hours…`.)
        // The hour is a setting: 2 h per trip makes the one-trip day 5 h.
        let mut settings = Settings::default();
        settings.catb.fill_per_trip_min = 120;
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(filled(&d), (60, 60, 5 * 60));
        // A trip of 1 h is a day of 2 h: what a trip adds does not depend on how long it is.
        let mut d = driver(vec![trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 09:00", Some("2026-10-05 10:00"))]);
        run(&mut d);
        assert_eq!(filled(&d), (30, 30, 2 * 60));
        // Two trips of 5 h 30 min together: 2 h added, a day of 7 h 30 min.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 08:00", Some("2026-10-05 10:30")),
        ]);
        run(&mut d);
        assert_eq!(filled(&d), (60, 60, 450));
    }

    #[test]
    fn the_hours_added_never_take_a_day_past_eight_hours() {
        // Three trips of 2 h 20 min = 7 h: 3 h would be added, but only the 1 h that is left is.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 07:20")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 07:20", Some("2026-10-05 09:40")),
            trip("eco-3", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 09:40", Some("2026-10-05 12:00")),
        ]);
        run(&mut d);
        assert_eq!(filled(&d), (30, 30, 8 * 60));
        // Two trips of 3 h are 6 h on the trips and 2 h are added to reach 8 h. The 2 h of waiting between them are not hours.
        let mut d = round_trip("2026-10-05 10:00", "2026-10-05 13:00");
        run(&mut d);
        assert_eq!(filled(&d), (60, 60, 8 * 60));
    }

    /// Constanța → Otopeni 05:00–`middle`, back `middle`–`end` (no waiting between the two trips).
    fn there_and_back(middle: &str, end: &str) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some(&format!("2026-10-05 {middle}"))),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", &format!("2026-10-05 {middle}"), Some(&format!("2026-10-05 {end}"))),
        ])
    }

    #[test]
    fn a_day_with_eight_hours_or_more_is_left_as_it_is_and_the_long_one_is_flagged() {
        // Exactly 8 h on the trips (4 + 4): nothing added, nothing said.
        let mut d = there_and_back("09:00", "13:00");
        run(&mut d);
        assert_eq!((d.days[0].fill_min, d.days[0].total_min), (0, 480));
        assert!(d.days[0].alerts.iter().all(|a| a.code != "long_day"));
        // 10 h on the trips (5 + 5): not shortened, but the office is warned.
        let mut d = there_and_back("10:00", "15:00");
        run(&mut d);
        assert_eq!((d.days[0].fill_min, d.days[0].total_min), (0, 600));
        let warning = d.days[0].alerts.iter().find(|a| a.code == "long_day").expect("a warning for a day over 8 hours");
        assert_eq!(warning.level, Level::Warn);
        assert!(warning.text.contains("10:00") && warning.text.contains("8 ore"), "{}", warning.text);
        // A long wait between short trips is not a long day: only the time on the trips counts (6 h + 2 h added).
        let mut d = round_trip("2026-10-05 12:00", "2026-10-05 15:00");
        run(&mut d);
        assert_eq!((d.days[0].station_min, d.days[0].fill_min, d.days[0].total_min), (240, 120, 480));
        assert!(d.days[0].alerts.iter().all(|a| a.code != "long_day"));
    }

    #[test]
    fn the_time_added_never_starts_before_midnight() {
        // A trip at 00:20 for 3 h: only 20 min fit before it, the rest of the hour goes after the arrival.
        let mut d = driver(vec![trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 00:20", Some("2026-10-05 03:20"))]);
        run(&mut d);
        assert_eq!(filled(&d), (20, 40, 4 * 60));
    }

    #[test]
    fn the_office_can_type_the_time_added_on_either_side() {
        let mut d = round_trip("2026-10-05 08:00", "2026-10-05 11:00");
        let date = d.trips[0].departure.date();
        d.fills.insert(date, FillEdit { before: Some(30), after: None });
        run(&mut d);
        assert_eq!(filled(&d), (30, 60, 450), "the side left empty keeps the rules' value");
        assert!(d.days[0].fill_manual);
        d.fills.insert(date, FillEdit { before: Some(0), after: Some(0) });
        run(&mut d);
        assert_eq!(filled(&d), (0, 0, 360), "typing 0 adds nothing");
        d.fills.clear();
        run(&mut d);
        assert_eq!(filled(&d), (60, 60, 480));
        assert!(!d.days[0].fill_manual);
    }

    #[test]
    fn with_the_setting_off_nothing_is_added_but_what_the_office_typed_still_counts() {
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        let mut d = round_trip("2026-10-05 08:00", "2026-10-05 11:00");
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(filled(&d), (0, 0, 360));
        let date = d.trips[0].departure.date();
        d.fills.insert(date, FillEdit { before: None, after: Some(60) });
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(filled(&d), (0, 60, 420));
    }

    #[test]
    fn a_different_hour_per_trip_or_day_length_changes_what_is_added() {
        let mut settings = Settings::default();
        settings.catb.fill_per_trip_min = 30;
        let mut d = round_trip("2026-10-05 08:00", "2026-10-05 11:00");
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(filled(&d), (30, 30, 420), "half an hour per trip: 1 h for the two");
        settings.catb.fill_per_trip_min = 120;
        settings.hours_per_day = 7;
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(filled(&d), (30, 30, 420), "a day is 7 h: 1 h is all that is left");
    }

    #[test]
    fn a_day_without_hours_gets_nothing() {
        // A trip beyond the county with no tariff is an excursion: it counts nowhere, so there is no day to fill.
        let mut d = driver(vec![trip("p-1", TripKind::Private, "Constanta", "Tulcea", "2026-10-06 09:00", None)]);
        run(&mut d);
        assert!(d.days.iter().all(|day| day.fill_min == 0 && day.total_min == 0), "{:?}", d.days);
        assert_eq!(d.totals.fill_min, 0);
    }

    #[test]
    fn waiting_beyond_three_hours_adds_ten_lei_an_hour_but_is_not_in_the_hours() {
        // Arrives 08:00, leaves 12:00: 4 h of waiting, 1 h beyond the included 3.
        let mut d = round_trip("2026-10-05 12:00", "2026-10-05 15:00");
        run(&mut d);
        assert_eq!((d.days[0].station_min, d.days[0].pay_station, d.days[0].pay_total), (240, 10.0, 250.0));
        assert_eq!((d.totals.trip_min, d.totals.station_min), (360, 240));
        assert_eq!(d.totals.total_min, 360 + 120, "the waiting is shown and paid, but it is not work time: 6 h of trips and 2 h added");
        // 5 h of waiting → 2 extra hours.
        let mut d = round_trip("2026-10-05 13:00", "2026-10-05 16:00");
        run(&mut d);
        assert_eq!(d.days[0].pay_total, 260.0);
    }

    #[test]
    fn waiting_of_six_hours_warns_that_he_probably_left_and_came_back() {
        let mut d = round_trip("2026-10-05 14:00", "2026-10-05 17:00");
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 360);
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "station_long" && a.level == Level::Warn));
        // 5 h 59 min does not.
        let mut d = round_trip("2026-10-05 13:59", "2026-10-05 16:59");
        run(&mut d);
        assert!(d.pauses[0].alerts.iter().all(|a| a.code != "station_long"));
    }

    #[test]
    fn a_long_wait_at_otopeni_is_a_pause_and_the_office_can_still_change_it() {
        // Arrives in Bucharest at 08:00, the next trip leaves 30 h later: a pauză, unpaid, and nobody is asked.
        let mut d = round_trip("2026-10-06 14:00", "2026-10-06 17:00");
        run(&mut d);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.is_empty(), "{:?}", d.pauses[0].alerts);
        // The office says it was a staționare: the whole pause counts, 30 h.
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 30 * 60);
        assert_eq!(d.pauses[0].extra_pay, 270.0, "27 h beyond the included 3");
        // Hours typed by hand replace the whole wait of a staționare.
        d.pause_edits[0].hours = Some(4.5);
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 270);
        // A pauză counts nothing, whatever hours were typed before.
        d.pause_edits[0].state = PauseState::Off;
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 0);
    }

    #[test]
    fn a_pause_of_exactly_the_long_limit_is_a_pause_and_one_minute_less_is_waiting() {
        // Arrives in Otopeni at 21:30 and leaves from Otopeni at 09:30: exactly 12 h.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-02 18:30", Some("2026-11-02 21:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-03 09:30", Some("2026-11-03 12:30")),
        ]);
        run(&mut d);
        assert_eq!((d.pauses[0].gap_min, d.pauses[0].kind, d.pauses[0].counted_min), (720, PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.iter().all(|a| a.code != "station_long"), "{:?}", d.pauses[0].alerts);
        d.trips[1].departure = parse_time("2026-11-03 09:29").unwrap();
        run(&mut d);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Normal, 719));
    }

    #[test]
    fn a_day_off_between_shifts_asks_nothing() {
        // Arrives in Constanța at 14:00 and starts again from Constanța 15 h later.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 11:00", Some("2026-10-05 14:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-06 05:00", Some("2026-10-06 08:00")),
        ]);
        run(&mut d);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.is_empty(), "{:?}", d.pauses[0].alerts);
        // Even when the next trip leaves from the other end (he came back some other way), a day off is not a remark.
        d.trips[1].from = "Aeroport Otopeni Bucuresti".into();
        d.trips[1].to = "Constanta".into();
        run(&mut d);
        assert_eq!(d.pauses[0].kind, PauseKind::Rest);
        assert!(d.pauses[0].alerts.is_empty());
        // The office can still say he waited.
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 15 * 60);
    }

    /// Arrives in Constanța on Mon 5 Oct at 14:00; the next trip leaves Mon 12 Oct at 05:00.
    fn week_off() -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 11:00", Some("2026-10-05 14:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-12 05:00", Some("2026-10-12 08:00")),
        ])
    }

    fn leave_on(d: &mut Driver, days: &[u32], code: &str) {
        for day in days {
            d.leaves.insert(NaiveDate::from_ymd_opt(2026, 10, *day).unwrap(), code.into());
        }
    }

    fn marked(d: &Driver) -> Vec<u32> {
        d.days.iter().filter(|x| x.leave.is_some()).map(|x| x.date.unwrap().day()).collect()
    }

    #[test]
    fn a_day_without_any_trip_can_be_marked_as_leave() {
        let mut d = week_off();
        run(&mut d);
        let free: Vec<u32> = d.free_days.iter().map(|x| x.day()).collect();
        assert_eq!(free.len(), 29, "October has 31 days, two of them have a trip");
        assert!(!free.contains(&5) && !free.contains(&12) && free.contains(&10), "a Saturday without a trip is a free day too");
        let hours = (d.totals.total_min, d.totals.station_min);

        leave_on(&mut d, &[6, 7, 10], "CO");
        run(&mut d);
        assert_eq!(marked(&d), [6, 7, 10]);
        assert_eq!(d.totals.leave_days.get("CO"), Some(&3));
        assert_eq!((d.totals.total_min, d.totals.station_min), hours, "a leave adds no hours");

        // Another kind of leave, and a day the office took out of the pontaj no longer counts.
        leave_on(&mut d, &[7], "CM");
        d.excluded_days.push(NaiveDate::from_ymd_opt(2026, 10, 6).unwrap());
        run(&mut d);
        assert_eq!((d.totals.leave_days.get("CO"), d.totals.leave_days.get("CM")), (Some(&1), Some(&1)));
    }

    #[test]
    fn a_leave_on_a_day_with_a_trip_or_of_an_unknown_kind_counts_nowhere() {
        let mut d = week_off();
        leave_on(&mut d, &[5], "CO");
        leave_on(&mut d, &[8], "XX");
        run(&mut d);
        assert!(marked(&d).is_empty() && d.totals.leave_days.is_empty());
        let blocked = d.alerts.iter().find(|a| a.code == "leave_blocked").expect("a leave on a day with a trip is reported");
        assert!(blocked.text.contains("05.10") && blocked.level == Level::Warn, "{blocked:?}");
    }

    #[test]
    fn what_makes_a_day_busy() {
        let mut d = week_off();
        // A coach trip abroad over two days, a night trip past midnight, and a removed trip.
        d.trips.push(trip("bus-1", TripKind::Bus, "Constanta", "Cluj", "2026-10-08 06:00", Some("2026-10-09 18:00")));
        d.trips.push(trip("eco-3", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-14 22:00", Some("2026-10-15 01:00")));
        let mut removed = trip("eco-4", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-20 05:00", Some("2026-10-20 08:00"));
        removed.edit.removed = true;
        d.trips.push(removed);
        run(&mut d);
        let busy: Vec<u32> = (1..=31).filter(|day| !d.free_days.iter().any(|x| x.day() == *day)).collect();
        assert_eq!(busy, [5, 8, 9, 12, 14, 15], "the removed trip frees the 20th");
    }

    #[test]
    fn a_leave_chosen_for_a_whole_pause_by_an_older_version_becomes_a_leave_on_each_free_working_day() {
        let mut d = week_off();
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), leave: Some("CM".into()), note: "foaia".into(), ..Default::default() });
        d.pause_edits.push(PauseEdit { prev: "x".into(), next: "y".into(), leave: Some("CO".into()), ..Default::default() });
        run(&mut d);
        assert_eq!(marked(&d), [6, 7, 8, 9], "Tuesday to Friday; the weekend between is not a leave");
        assert_eq!(d.totals.leave_days.get("CM"), Some(&4));
        assert_eq!(d.pause_edits.len(), 1, "the note stays, the empty edit goes");
        assert!(d.pause_edits[0].leave.is_none() && d.pause_edits[0].note == "foaia");
        run(&mut d);
        assert_eq!(marked(&d), [6, 7, 8, 9], "nothing moves on the next recompute");
    }

    #[test]
    fn the_time_in_constanta_is_a_pause_not_a_station() {
        // A private trip ends in Constanța at 02:00 and the next one leaves from Constanța at 12:00: he is at home.
        let mut d = driver(vec![
            trip("private-1", TripKind::Private, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-09 23:00", Some("2026-11-10 02:00")),
            trip("private-2", TripKind::Private, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-10 12:00", Some("2026-11-10 15:00")),
        ]);
        recompute_driver(&mut d, 2026, 11, &Settings::default());
        let pause = &d.pauses[0];
        assert_eq!((pause.gap_min, pause.kind, pause.counted_min, pause.extra_pay), (600, PauseKind::Rest, 0, 0.0));
        assert!(pause.alerts.is_empty(), "no 'staționare lungă' for a pause at home: {:?}", pause.alerts);
        assert_eq!(d.totals.station_min, 0);
        // The office can say he did wait there: now it is a staționare of 10 h (7 beyond the included 3).
        d.pause_edits.push(PauseEdit { prev: "private-1".into(), next: "private-2".into(), state: PauseState::Stayed, ..Default::default() });
        recompute_driver(&mut d, 2026, 11, &Settings::default());
        assert_eq!((d.pauses[0].counted_min, d.pauses[0].extra_pay), (600, 70.0));
        // And back: choosing "pauză" again counts nothing.
        d.pause_edits[0].state = PauseState::Off;
        recompute_driver(&mut d, 2026, 11, &Settings::default());
        assert_eq!((d.pauses[0].counted_min, d.pauses[0].extra_pay), (0, 0.0));
    }

    #[test]
    fn the_same_wait_at_otopeni_is_a_station() {
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-10 02:00", Some("2026-11-10 05:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-10 15:00", Some("2026-11-10 18:00")),
        ]);
        recompute_driver(&mut d, 2026, 11, &Settings::default());
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min, d.pauses[0].extra_pay), (PauseKind::Normal, 600, 70.0));
    }

    #[test]
    fn a_paid_far_trip_between_two_regular_trips_asks_nothing() {
        // Back in Otopeni at 08:00, a private Constanța → Tulcea trip (110 lei), the next run leaves Otopeni at 17:00.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, "Constanta", "Tulcea", "2026-10-05 09:00", None),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 17:00", Some("2026-10-05 20:00")),
        ]);
        run(&mut d);
        assert_eq!(d.pauses.len(), 1, "the private trip is not part of the waiting pairs");
        assert_eq!(d.pauses[0].kind, PauseKind::Normal, "he waits at Otopeni");
        assert!(d.pauses[0].alerts.iter().all(|a| a.code != "to_confirm" && a.code != "excursion"), "{:?}", d.pauses[0].alerts);
        // The private trip is paid by its destination: Tulcea 110, and has no duration to count.
        assert_eq!((d.days[0].pay_exterior, d.days[0].legs, d.days[0].trip_min), (110.0, 2, 360));
    }

    #[test]
    fn choices_saved_by_older_versions_still_open_as_a_staționare_or_a_pause() {
        // "passenger" and "excursion" were removed from the choices: they are read as a pauză; "confirm" as a pauză too.
        let state = |text: &str| serde_json::from_str::<PauseState>(&format!("\"{text}\"")).unwrap();
        assert_eq!((state("auto"), state("stayed"), state("off")), (PauseState::Auto, PauseState::Stayed, PauseState::Off));
        assert_eq!((state("passenger"), state("excursion")), (PauseState::Off, PauseState::Off));
        let kind = |text: &str| serde_json::from_str::<PauseKind>(&format!("\"{text}\"")).unwrap();
        assert_eq!((kind("normal"), kind("rest"), kind("confirm"), kind("overlap")), (PauseKind::Normal, PauseKind::Rest, PauseKind::Rest, PauseKind::Overlap));
        // A saved edit with the fields that no longer exist (when he left / came back) still loads.
        let edit: PauseEdit = serde_json::from_str(r#"{"prev":"a","next":"b","state":"passenger","hours":null,"left":"2026-10-05T11:00:00","back":null,"note":"foaia"}"#).unwrap();
        assert_eq!((edit.state, edit.note.as_str()), (PauseState::Off, "foaia"));
    }

    /// Arrives in Otopeni at 08:00; a private trip `from → to` at 10:00 (40 min if `arrival` is given); leaves again at 14:00.
    fn with_private_trip_in_between(from: &str, to: &str, arrival: Option<&str>) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, from, to, "2026-10-05 10:00", arrival),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 14:00", Some("2026-10-05 17:00")),
        ])
    }

    #[test]
    fn a_private_trip_inside_the_county_is_work_not_an_excursion() {
        let mut d = with_private_trip_in_between("Mamaia Nord", "Constanta", Some("2026-10-05 10:40"));
        run(&mut d);
        let pause = &d.pauses[0];
        assert_eq!(pause.kind, PauseKind::Normal, "a resort is not an excursion");
        assert!(pause.alerts.iter().all(|a| a.code != "excursion_done"), "{:?}", pause.alerts);
        // 6 h between the legs, 40 min of them on the private trip: that time is trip time, the rest is waiting.
        assert_eq!((pause.gap_min, pause.busy_min, pause.counted_min), (360, 40, 320));
        let day = &d.days[0];
        // Every minute between 05:00 and 17:00 counts once, as trip or as waiting; only the trips are hours (6 h 40 min),
        // and 1 h 20 min is added to reach 8 h.
        assert_eq!((day.trip_min, day.station_min, day.total_min), (180 + 40 + 180, 320, 480));
        // The office saying he stayed does not count the private trip twice either.
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 320);
    }

    #[test]
    fn a_private_trip_without_duration_takes_nothing_from_the_waiting() {
        let mut d = with_private_trip_in_between("Mamaia Nord", "Constanta", None);
        run(&mut d);
        assert_eq!((d.pauses[0].kind, d.pauses[0].busy_min, d.pauses[0].counted_min), (PauseKind::Normal, 0, 360));
    }

    /// Constanța → Otopeni 05:00–08:00, then a private trip Otopeni → Năvodari 09:00–13:00 (4 h), then back to Otopeni
    /// from Constanța at 17:00–20:00: the private trip is an Otopeni run.
    fn with_otopeni_run(kind: TripKind) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", kind, "Aeroport Otopeni Bucuresti", "Navodari", "2026-10-05 09:00", Some("2026-10-05 13:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 17:00", Some("2026-10-05 20:00")),
        ])
    }

    #[test]
    fn a_private_run_from_otopeni_to_navodari_is_a_regular_leg() {
        for kind in [TripKind::Private, TripKind::Vip] {
            let mut d = with_otopeni_run(kind);
            run(&mut d);
            let run_trip = d.trips.iter().find(|t| t.id == "private-9").unwrap();
            assert!(run_trip.eff.regular && run_trip.eff.counts_hours && !run_trip.eff.excursion, "{kind:?}");
            assert_eq!((run_trip.eff.price, run_trip.eff.price_note.as_str()), (120.0, "cursă"), "a leg, not the Năvodari tariff");
            // Three regular legs. He waits at Otopeni 08:00→09:00 (staționare); after the run he is in Năvodari, at home
            // on the Constanța side, so 13:00→17:00 is a pauză.
            assert_eq!(d.pauses.len(), 2);
            assert_eq!((d.pauses[0].gap_min, d.pauses[0].kind, d.pauses[0].counted_min), (60, PauseKind::Normal, 60));
            assert_eq!((d.pauses[1].gap_min, d.pauses[1].kind, d.pauses[1].counted_min, d.pauses[1].extra_pay), (240, PauseKind::Rest, 0, 0.0));
            let day = &d.days[0];
            assert_eq!((day.legs, day.pay_legs, day.pay_exterior, day.trip_min, day.station_min), (3, 360.0, 0.0, 180 + 240 + 180, 60));
            assert_eq!(day.pay_total, 360.0);
            // Leaving from Constanța after arriving in Năvodari is the same area: no "loc diferit".
            assert!(d.pauses.iter().all(|p| p.alerts.iter().all(|a| a.code != "place_differs")), "{:?}", d.pauses);
        }
    }

    #[test]
    fn a_private_run_toward_otopeni_from_outside_the_county_is_the_same_kind_of_run() {
        // Tulcea → Otopeni: no tariff, not an excursion, not a question for the office: a leg like any other.
        let mut d = driver(vec![
            trip("private-9", TripKind::Private, "Tulcea", "Aeroport Otopeni Bucuresti", "2026-10-05 10:00", Some("2026-10-05 14:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 17:00", Some("2026-10-05 20:00")),
        ]);
        run(&mut d);
        let tulcea = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(tulcea.eff.regular && tulcea.eff.counts_hours && !tulcea.eff.excursion);
        assert_eq!((tulcea.eff.price, tulcea.eff.price_note.as_str()), (120.0, "cursă"));
        assert_eq!(d.pauses.len(), 1);
        assert_eq!(d.pauses[0].kind, PauseKind::Normal);
        // The waiting of that day applies: 14:30 → 17:00.
        assert_eq!(d.pauses[0].counted_min, 150);
        let day = &d.days[0];
        assert_eq!((day.legs, day.pay_legs, day.pay_exterior), (2, 240.0, 0.0));
        assert!(day.alerts.iter().all(|a| a.code != "no_tariff"), "{:?}", day.alerts);
    }

    #[test]
    fn after_a_run_that_ends_in_the_county_the_drive_to_otopeni_is_added_and_the_time_before_it_is_a_pause() {
        // The private trip Otopeni → Năvodari, then the next run leaves from Otopeni: nobody logged how he got there, so the
        // app adds the drive (3 h, ending when the run leaves). The time he spent in Năvodari before it is a pause.
        let mut d = driver(vec![
            trip("private-9", TripKind::Private, "Aeroport Otopeni Bucuresti", "Navodari", "2026-10-05 05:00", Some("2026-10-05 09:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-06 12:00", Some("2026-10-06 15:00")),
        ]);
        run(&mut d);
        assert_eq!(d.trips.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["private-9", "gen-pre-eco-2", "eco-2"]);
        assert_eq!(d.pauses.len(), 2);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0), "at home in Năvodari");
        assert_eq!((d.pauses[1].gap_min, d.pauses[1].counted_min), (0, 0), "he drives straight to the next run");
        assert!(d.pauses.iter().all(|p| p.alerts.is_empty()), "{:?}", d.pauses);
    }

    #[test]
    fn arriving_in_otopeni_and_leaving_from_constanta_adds_the_way_back() {
        // He waits at Otopeni, but the next run leaves from Constanța: he came back with the car or as a passenger. The app
        // adds that drive at the moment he arrived, so the wait at Otopeni is not counted and there is no remark.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 12:00", Some("2026-10-05 15:00")),
        ]);
        run(&mut d);
        assert_eq!(d.trips.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["eco-1", "gen-ret-eco-1", "eco-2"]);
        assert_eq!((d.days[0].legs, d.days[0].trip_min, d.days[0].station_min), (3, 180 + 180 + 180, 0));
        assert!(d.pauses.iter().all(|p| p.alerts.iter().all(|a| a.code != "place_differs")), "{:?}", d.pauses);
        // With the generation off it is the remark of before: he probably went back as a passenger.
        let mut off = Settings::default();
        off.catb.generate_transfers = false;
        recompute_driver(&mut d, 2026, 10, &off);
        assert_eq!(d.trips.len(), 2);
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "place_differs" && a.level == Level::Warn), "{:?}", d.pauses[0].alerts);
    }

    #[test]
    fn the_otopeni_run_is_not_regular_when_the_setting_is_off() {
        let mut d = with_otopeni_run(TripKind::Private);
        let mut settings = Settings::default();
        settings.catb.main_route_private_regular = false;
        recompute_driver(&mut d, 2026, 10, &settings);
        let trip = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(!trip.eff.regular);
        assert_eq!(trip.eff.price_note, "exterior: Năvodari");
    }

    #[test]
    fn a_far_trip_without_a_tariff_is_an_excursion_by_itself() {
        let mut d = with_private_trip_in_between("Constanta", "Chitila", Some("2026-10-05 11:30"));
        run(&mut d);
        let chitila = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(chitila.eff.excursion && !chitila.eff.counts_hours, "no tariff, beyond the county: tachograph");
        assert_eq!((chitila.eff.price, chitila.eff.price_note.as_str()), (0.0, EXCURSION_NOTE));
        // Nothing missing, nothing counted.
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.iter().all(|a| a.code == "excursion_done"), "{:?}", d.pauses[0].alerts);
        assert!(d.pauses[0].alerts.iter().any(|a| a.text.contains("Chitila")));
        assert!(d.days.iter().all(|day| day.alerts.is_empty()), "no 'fără tarif' for an excursion");
        assert_eq!(d.totals.pay_exterior, 0.0);
        // A price typed by hand makes it a paid trip like any other.
        d.trips.iter_mut().find(|t| t.id == "private-9").unwrap().edit.price = Some(100.0);
        run(&mut d);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
        assert_eq!(d.pauses[0].kind, PauseKind::Normal);
        assert!(d.pauses[0].alerts.iter().all(|a| a.code != "excursion_done"), "{:?}", d.pauses[0].alerts);
        // A place the office says is normal for this firm is not far at all.
        d.trips.iter_mut().find(|t| t.id == "private-9").unwrap().edit.price = None;
        let mut settings = Settings::default();
        settings.catb.extra_local_places = vec!["Chitila".into()];
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(d.pauses[0].kind, PauseKind::Normal);
    }

    #[test]
    fn a_far_trip_with_a_tariff_is_a_paid_trip_like_any_other() {
        let mut d = with_private_trip_in_between("Constanta", "Tulcea", Some("2026-10-05 11:30"));
        run(&mut d);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
        assert_eq!(d.days[0].pay_exterior, 110.0);
        assert_eq!(d.pauses[0].kind, PauseKind::Normal);
    }

    #[test]
    fn a_coach_trip_abroad_alone_on_its_day_counts_nothing() {
        // Constanța → Kalambaka by coach, no regular trip around it.
        let mut d = driver(vec![trip("bus-1", TripKind::Bus, "Constanta", "Kalambaka", "2026-10-06 04:00", None)]);
        run(&mut d);
        assert!(d.trips[0].eff.excursion);
        assert!(d.days.is_empty(), "{:?}", d.days);
        assert_eq!((d.totals.total_min, d.totals.pay_exterior), (0, 0.0));
        assert!(d.alerts.is_empty());
    }

    #[test]
    fn an_excursion_trip_and_the_pause_around_it_count_nowhere() {
        // Back from Otopeni at 08:00 in Constanța; a private trip to Albac (no duration, no tariff: the tachograph has it);
        // the next regular trip 4 days later.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, "Constanta", "Albac", "2026-10-06 06:00", None),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-10 09:30", Some("2026-10-10 12:30")),
        ]);
        run(&mut d);
        let pause = &d.pauses[0];
        assert_eq!((pause.kind, pause.counted_min, pause.extra_pay), (PauseKind::Rest, 0, 0.0));
        assert!(pause.alerts.iter().any(|a| a.code == "excursion_done" && a.text.contains("Albac") && a.text.contains("tahograf")), "{:?}", pause.alerts);
        let albac = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(albac.eff.excursion && !albac.eff.counts_hours);
        assert_eq!((albac.eff.price, albac.eff.price_note.as_str()), (0.0, EXCURSION_NOTE));
        assert!(d.days.iter().all(|day| day.alerts.is_empty()), "an excursion trip raises no 'fără tarif'");
        assert_eq!(d.totals.pay_exterior, 0.0);
        // With a tariff for the place it is an ordinary paid trip.
        let mut settings = Settings::default();
        settings.catb.tariffs.push(super::tariffs::Tariff { name: "Albac".into(), amount: 150.0 });
        recompute_driver(&mut d, 2026, 10, &settings);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
        assert_eq!(d.days.iter().map(|day| day.pay_exterior).sum::<f64>(), 150.0);
    }

    #[test]
    fn private_trips_are_priced_by_destination_and_untariffed_ones_are_flagged() {
        let mut d = driver(vec![
            trip("p1", TripKind::Private, "Constanta", "Mangalia", "2026-10-05 09:00", Some("2026-10-05 10:00")),
            trip("p2", TripKind::Private, "Cernavoda", "Constanta", "2026-10-05 12:00", None),
            trip("p3", TripKind::Private, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 15:00", Some("2026-10-05 18:00")),
        ]);
        run(&mut d);
        let notes: Vec<&str> = d.trips.iter().map(|t| t.eff.price_note.as_str()).collect();
        assert_eq!(notes, ["exterior: Mangalia", NO_TARIFF, "cursă"]);
        assert_eq!((d.days[0].pay_exterior, d.days[0].pay_legs), (40.0, 120.0), "a private trip on the main route is a regular leg");
        assert!(d.days[0].alerts.iter().any(|a| a.code == "no_tariff" && a.text.contains("Cernavoda")));
        // A price typed by hand replaces the missing tariff.
        d.trips[1].edit.price = Some(80.0);
        run(&mut d);
        assert_eq!(d.days[0].pay_exterior, 120.0);
        assert!(d.days[0].alerts.is_empty());
    }

    #[test]
    fn a_different_place_between_two_trips_is_flagged() {
        // Arrives in Bucharest, the next trip leaves from far away (Tulcea): not a return to Constanța, so the app adds
        // nothing, and he is asked about: he came back some other way.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("eco-2", TripKind::Eco, "Tulcea", "Aeroport Otopeni Bucuresti", "2026-10-05 15:00", Some("2026-10-05 18:00")),
        ]);
        run(&mut d);
        let alert = d.pauses[0].alerts.iter().find(|a| a.code == "place_differs").expect("flagged");
        assert_eq!(alert.level, Level::Warn);
        assert!(alert.text.contains("pasager"));
    }

    #[test]
    fn edits_survive_and_removed_trips_count_nowhere() {
        let mut d = round_trip("2026-10-05 10:00", "2026-10-05 13:00");
        d.trips[1].edit.removed = true;
        run(&mut d);
        assert_eq!((d.days[0].legs, d.days[0].trip_min, d.pauses.len()), (1, 180, 0));
        // The office corrects the departure and the arrival of the first trip.
        d.trips[1].edit.removed = false;
        d.trips[0].edit.departure = parse_time("2026-10-05 06:00");
        d.trips[0].edit.arrival = parse_time("2026-10-05 09:30");
        run(&mut d);
        assert_eq!(d.trips[0].eff.minutes, 210);
        assert_eq!(d.pauses[0].gap_min, 30);
    }

    #[test]
    fn the_monthly_ceiling_counts_every_weekday() {
        let settings = Settings::default();
        assert_eq!(monthly_cap_min(2026, 10, &settings), 176 * 60);
        assert_eq!(monthly_cap_min(2026, 11, &settings), 168 * 60);
    }

    #[test]
    fn a_driver_over_the_ceiling_is_flagged() {
        // 23 days of two 3-hour legs (and 3 hours of waiting that are not hours), 2 h added to each: 8 h a day = 184 h > 176 h.
        let mut trips = Vec::new();
        for day in 1..=23 {
            trips.push(trip(&format!("a{day}"), TripKind::Eco, "Constanta", "Otopeni", &format!("2026-10-{day:02} 05:00"), Some(&format!("2026-10-{day:02} 08:00"))));
            trips.push(trip(&format!("b{day}"), TripKind::Eco, "Otopeni", "Constanta", &format!("2026-10-{day:02} 11:00"), Some(&format!("2026-10-{day:02} 14:00"))));
        }
        let mut d = driver(trips);
        run(&mut d);
        assert_eq!(d.totals.total_min, 23 * 480);
        assert_eq!(d.totals.over_min, 23 * 480 - 176 * 60);
        assert!(d.alerts.iter().any(|a| a.code == "over_cap" && a.level == Level::Alert));
        assert!(!d.proposals.is_empty());
    }
}
