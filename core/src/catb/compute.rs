//! Rebuilds everything derived from a driver's trips and the office's edits: the trips as they count, the pauses
//! between regular trips (waiting time), the days, the totals, the estimate and the remarks.
//!
//! Rules (all numbers come from [`CatbSettings`]):
//! - A day's hours are the known driving time of its trips plus the waiting time of the pauses that start that day.
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
use super::{cap, places, tariffs, transfers};
use crate::calendar;
use crate::settings::Settings;
use crate::time::{fmt_duration, round_hours};
use crate::timesheet::month_days;

/// Recomputes the whole sheet.
pub fn recompute(sheet: &mut Sheet, settings: &Settings) {
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
    driver.days = days(driver, year, month);
    let cap_min = monthly_cap_min(year, month, settings);
    driver.totals = totals(&driver.days, cap_min);
    driver.alerts = driver_alerts(driver, cap_min);
    driver.proposals = cap::propose(driver, cap_min, settings);
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
    Effective { departure: Some(departure), arrival, from, to, minutes, regular, counts_hours, excursion, price, price_note }
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
    let mut busy: BTreeSet<NaiveDate> = BTreeSet::new();
    for trip in trips.iter().filter(|t| !t.edit.removed) {
        let Some(from) = trip.eff.departure else { continue };
        let to = trip.eff.arrival.filter(|arrival| *arrival >= from).unwrap_or(from);
        let mut day = from.date();
        while day <= to.date() {
            busy.insert(day);
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
    }
    (1..=calendar::days_in_month(year, month)).filter_map(|d| NaiveDate::from_ymd_opt(year, month, d)).filter(|day| !busy.contains(day)).collect()
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

fn days(driver: &Driver, year: i32, month: u32) -> Vec<DayRow> {
    let mut rows: BTreeMap<NaiveDate, DayRow> = BTreeMap::new();

    // A trip belongs to the day it departs, a pause to the day it starts.
    for trip in driver.trips.iter().filter(|t| !t.edit.removed && !t.eff.excursion) {
        let eff = &trip.eff;
        let Some(row) = eff.departure.and_then(|d| day_row(&mut rows, d.date(), year, month)) else { continue };
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
        let Some(row) = pause.start.and_then(|s| day_row(&mut rows, s.date(), year, month)) else { continue };
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
        row.total_min = row.trip_min + row.station_min;
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

    #[test]
    fn waiting_beyond_three_hours_adds_ten_lei_an_hour_and_counts_in_the_hours() {
        // Arrives 08:00, leaves 12:00: 4 h of waiting, 1 h beyond the included 3.
        let mut d = round_trip("2026-10-05 12:00", "2026-10-05 15:00");
        run(&mut d);
        assert_eq!((d.days[0].station_min, d.days[0].pay_station, d.days[0].pay_total), (240, 10.0, 250.0));
        assert_eq!(d.totals.total_min, 360 + 240, "the waiting is part of the hours");
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
        assert_eq!((day.trip_min, day.station_min, day.total_min), (180 + 40 + 180, 320, 720), "every minute between 05:00 and 17:00 counts once");
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
        // 23 days of two 3-hour legs and 3 hours of waiting: 9 h a day = 207 h > 176 h.
        let mut trips = Vec::new();
        for day in 1..=23 {
            trips.push(trip(&format!("a{day}"), TripKind::Eco, "Constanta", "Otopeni", &format!("2026-10-{day:02} 05:00"), Some(&format!("2026-10-{day:02} 08:00"))));
            trips.push(trip(&format!("b{day}"), TripKind::Eco, "Otopeni", "Constanta", &format!("2026-10-{day:02} 11:00"), Some(&format!("2026-10-{day:02} 14:00"))));
        }
        let mut d = driver(trips);
        run(&mut d);
        assert_eq!(d.totals.total_min, 23 * 540);
        assert_eq!(d.totals.over_min, 23 * 540 - 176 * 60);
        assert!(d.alerts.iter().any(|a| a.code == "over_cap" && a.level == Level::Alert));
        assert!(!d.proposals.is_empty());
    }
}
