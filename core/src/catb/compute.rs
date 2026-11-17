//! Rebuilds everything derived from a driver's trips and the office's edits: the trips as they count, the pauses
//! between regular trips (waiting time), the days, the totals, the estimate and the remarks.
//!
//! Rules (all numbers come from [`CatbSettings`]):
//! - A day's hours are the known driving time of its trips plus the waiting time of the pauses that start that day.
//! - Waiting is only measured between two *regular* trips (Constanța ↔ Otopeni; private/VIP trips on the same route
//!   count as regular, and so does any private/VIP "Otopeni run": a trip with the Otopeni/Bucharest end on one side and
//!   any place on the other, even outside the county, e.g. Otopeni → Năvodari, Mamaia → Otopeni, Tulcea → Otopeni).
//!   It is inferred from the API (next departure − previous arrival), never read from it.
//! - A pause shorter than `long_pause_min` counts by itself. One of that length or longer is taken as "came back as a
//!   passenger or some other way" (`Rest`: counts nothing, asks nothing); the office can still say he stayed there. Only when another kind of
//!   trip lies in between, beyond the county (`Confirm`), the pause waits for the office to say what it was
//!   (stayed, passenger, day off, excursion).
//! - A trip beyond Constanța county with no tariff is an excursion by itself (the tachograph has it): it counts nowhere
//!   here and its pause is out too. With a tariff it is a paid trip, and the pause waits for the office.
//! - The estimate is `pair_pay / 2` per regular leg, the exterior tariff for the other trips, and `station_rate` for every
//!   started hour of waiting beyond `station_included_min` (started = from `round_up_from_min`, like the firm's hours).

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};

use super::config::CatbSettings;
use super::model::*;
use super::{cap, places, tariffs};
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
    driver.trips.sort_by_key(|t| (t.eff.departure, t.id.clone()));
    driver.pauses = pauses(&driver.trips, &driver.pause_edits, settings);
    take_excursions_out(&mut driver.trips, &driver.pauses);
    driver.days = days(&driver.trips, &driver.pauses, &driver.excluded_days, year, month);
    let cap_min = monthly_cap_min(year, month, settings);
    driver.totals = totals(&driver.days, cap_min);
    driver.alerts = driver_alerts(driver, cap_min);
    driver.proposals = cap::propose(driver, cap_min, settings);
}

fn effective(trip: &Trip, cfg: &CatbSettings) -> Effective {
    let departure = trip.edit.departure.unwrap_or(trip.departure);
    let arrival = trip.edit.arrival.or(trip.arrival);
    let from = trip.edit.from.clone().unwrap_or_else(|| trip.from.clone());
    let to = trip.edit.to.clone().unwrap_or_else(|| trip.to.clone());
    let minutes = arrival.map(|a| (a - departure).num_minutes()).filter(|m| *m >= 0).unwrap_or(0) as i32;
    let regular = match trip.kind {
        TripKind::Eco | TripKind::Passenger => true,
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

/// An excursion is timed from the tachograph, so it leaves Categoria B: the other trips between the two regular
/// trips of a pause marked "Excursie" (the far trip, anything done around it) count nowhere here.
fn take_excursions_out(trips: &mut [Trip], pauses: &[Pause]) {
    for pause in pauses.iter().filter(|p| p.state == PauseState::Excursion) {
        let (Some(first), Some(last)) = (trips.iter().position(|t| t.id == pause.prev), trips.iter().position(|t| t.id == pause.next)) else { continue };
        for trip in trips.iter_mut().take(last).skip(first + 1).filter(|t| !t.edit.removed && !t.eff.regular) {
            trip.eff.excursion = true;
            trip.eff.counts_hours = false;
            trip.eff.price = 0.0;
            trip.eff.price_note = EXCURSION_NOTE.into();
        }
    }
}

fn price_of(trip: &Trip, regular: bool, from: &str, to: &str, cfg: &CatbSettings) -> (f64, String) {
    if let Some(price) = trip.edit.price {
        return (price, "preț trecut manual".into());
    }
    if regular {
        return (cfg.leg_pay(), "cursă".into());
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
            pause.left = edit.left;
            pause.back = edit.back;
            pause.note = edit.note.clone();
        }
        // A far trip that has a tariff is paid work or an excursion: the office decides. One without a tariff is already an
        // excursion (see `effective`), so the pause around it is simply out. A private job in a resort is just work.
        let far_paid_trip = between.iter().any(|t| is_far(t, cfg) && !t.eff.excursion);
        let excursion_inside = between.iter().any(|t| t.eff.excursion);
        pause.kind = if gap_min < 0 {
            PauseKind::Overlap
        } else if far_paid_trip {
            PauseKind::Confirm
        } else if excursion_inside || gap_min >= cfg.long_pause_min {
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

/// Does the trip go beyond the places of an ordinary day (the regular route and Constanța county)?
fn is_far(trip: &Trip, cfg: &CatbSettings) -> bool {
    is_far_route(&trip.eff.from, &trip.eff.to, cfg)
}

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

/// How much of a pause counts as waiting.
fn counted_minutes(pause: &Pause) -> i32 {
    // An excursion is not Categoria B: nothing of it counts here, not even hours typed by hand.
    if pause.state == PauseState::Excursion {
        return 0;
    }
    if let Some(hours) = pause.hours_override {
        return (hours.max(0.0) * 60.0).round() as i32;
    }
    let gap = (pause.gap_min.max(0) - pause.busy_min).max(0);
    let by_itself = if pause.kind == PauseKind::Normal { gap } else { 0 };
    match pause.state {
        PauseState::Off | PauseState::Excursion => 0,
        PauseState::Stayed => gap,
        // He waited until he left; without that time, a normal pause counts whole and a long one not at all.
        PauseState::Passenger => match (pause.start, pause.left) {
            (Some(start), Some(left)) => ((left - start).num_minutes() as i32).clamp(0, gap),
            _ => by_itself,
        },
        PauseState::Auto => by_itself,
    }
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
    let manual_excursion = pause.state == PauseState::Excursion;
    if manual_excursion || between.iter().any(|t| t.eff.excursion) {
        let place = between.iter().find(|t| t.eff.excursion || is_far(t, cfg)).or(between.first()).map_or_else(|| pause.place_to.clone(), |t| t.eff.to.clone());
        let dates = match (pause.start, pause.end) {
            (Some(start), Some(end)) => format!(", {} – {}", start.format("%d.%m"), end.format("%d.%m")),
            _ => String::new(),
        };
        alerts.push(Alert::new("excursion_done", Level::Info, format!("Excursie ({place}{dates}): nu intră în Categoria B, se pontează din tahograf. Pauza și cursele din ea nu se numără aici.")));
        if manual_excursion {
            return alerts;
        }
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
    // A day off between two shifts is not worth a remark, even if he came back some other way.
    if differs && undecided && pause.kind != PauseKind::Rest {
        let text = format!("A ajuns în {}, iar următoarea cursă pleacă din {}: probabil s-a întors ca pasager.", pause.place_from, pause.place_to);
        let level = if pause.kind == PauseKind::Normal { Level::Warn } else { Level::Info };
        alerts.push(Alert::new("place_differs", level, format!("Loc diferit între curse. {text}")));
    }
    if pause.kind == PauseKind::Confirm && undecided {
        if let Some(trip) = between.iter().find(|t| is_far(t, cfg) && !t.eff.excursion) {
            let why = format!("între ele: cursă {} {} → {}, în afara județului Constanța", trip.kind.label().to_lowercase(), trip.eff.from, trip.eff.to);
            alerts.push(Alert::new("to_confirm", Level::Alert, format!("De confirmat ({why}): timpul dintre curse nu intră în ore până alegi ce a fost: a stat, întors ca pasager, zi liberă sau excursie (iese din Categoria B, se pontează din tahograf).")));
            alerts.push(Alert::new("excursion", Level::Info, format!("Probabilă excursie: {} → {}.", trip.eff.from, trip.eff.to)));
        }
    }
    alerts
}

/// The row of a day, created on first use. Days outside the month get no row (they belong to the other month's sheet).
fn day_row(rows: &mut BTreeMap<NaiveDate, DayRow>, date: NaiveDate, year: i32, month: u32) -> Option<&mut DayRow> {
    let inside = date.year() == year && date.month() == month;
    inside.then(|| rows.entry(date).or_insert_with(|| DayRow { date: Some(date), ..Default::default() }))
}

fn days(trips: &[Trip], pauses: &[Pause], excluded: &[NaiveDate], year: i32, month: u32) -> Vec<DayRow> {
    let mut rows: BTreeMap<NaiveDate, DayRow> = BTreeMap::new();

    // A trip belongs to the day it departs, a pause to the day it starts.
    for trip in trips.iter().filter(|t| !t.edit.removed && !t.eff.excursion) {
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
    }
    for pause in pauses {
        let Some(row) = pause.start.and_then(|s| day_row(&mut rows, s.date(), year, month)) else { continue };
        row.station_min += pause.counted_min;
        row.pay_station += pause.extra_pay;
    }

    let mut list: Vec<DayRow> = rows.into_values().collect();
    for row in &mut list {
        row.total_min = row.trip_min + row.station_min;
        row.pay_total = row.pay_legs + row.pay_exterior + row.pay_station;
        row.excluded = row.date.is_some_and(|d| excluded.contains(&d));
    }
    list
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
    fn a_long_pause_is_taken_as_a_passenger_return_and_the_office_can_still_change_it() {
        // Arrives in Bucharest at 08:00, the next trip leaves 30 h later: unpaid, and nobody is asked.
        let mut d = round_trip("2026-10-06 14:00", "2026-10-06 17:00");
        run(&mut d);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.is_empty(), "{:?}", d.pauses[0].alerts);
        // The office says he stayed there: the whole pause counts, 30 h.
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 30 * 60);
        assert_eq!(d.pauses[0].extra_pay, 270.0, "27 h beyond the included 3");
        // "Day off" counts nothing.
        d.pause_edits[0].state = PauseState::Off;
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 0);
        // Hours typed by hand win over the state.
        d.pause_edits[0].hours = Some(4.5);
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 270);
    }

    #[test]
    fn a_pause_of_exactly_the_long_limit_is_a_pause_and_one_minute_less_is_waiting() {
        // Arrives in Constanța at 21:30 and leaves from Constanța at 09:30: exactly 12 h.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-02 18:30", Some("2026-11-02 21:30")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-03 09:30", Some("2026-11-03 12:30")),
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

    #[test]
    fn a_passenger_return_counts_the_waiting_until_he_left() {
        let mut d = round_trip("2026-10-05 17:30", "2026-10-05 20:30");
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 570, "9 h 30 min of waiting, undecided");
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Passenger, left: parse_time("2026-10-05 11:00"), ..Default::default() });
        run(&mut d);
        assert_eq!(d.pauses[0].counted_min, 180, "he left at 11:00: 3 h of waiting");
        assert_eq!(d.days[0].pay_total, 240.0);
    }

    #[test]
    fn a_trip_in_between_sends_the_pause_to_the_office() {
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, "Constanta", "Tulcea", "2026-10-05 09:00", None),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 17:00", Some("2026-10-05 20:00")),
        ]);
        run(&mut d);
        assert_eq!(d.pauses.len(), 1, "the private trip is not part of the waiting pairs");
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Confirm, 0));
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "excursion" && a.text.contains("Tulcea")));
        // The private trip is paid by its destination: Tulcea 110, and has no duration to count.
        assert_eq!((d.days[0].pay_exterior, d.days[0].legs, d.days[0].trip_min), (110.0, 2, 360));
    }

    /// Arrives in Constanța at 08:00; a private trip `from → to` at 10:00 (40 min if `arrival` is given); leaves again at 14:00.
    fn with_private_trip_in_between(from: &str, to: &str, arrival: Option<&str>) -> Driver {
        driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, from, to, "2026-10-05 10:00", arrival),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 14:00", Some("2026-10-05 17:00")),
        ])
    }

    #[test]
    fn a_private_trip_inside_the_county_is_work_not_an_excursion() {
        let mut d = with_private_trip_in_between("Mamaia Nord", "Constanta", Some("2026-10-05 10:40"));
        run(&mut d);
        let pause = &d.pauses[0];
        assert_eq!(pause.kind, PauseKind::Normal, "a resort is not an excursion");
        assert!(pause.alerts.iter().all(|a| a.code != "excursion" && a.code != "to_confirm"), "{:?}", pause.alerts);
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
            // Three regular legs; the waiting is measured around it: 08:00→09:00 and 13:00→17:00.
            assert_eq!(d.pauses.len(), 2);
            assert_eq!((d.pauses[0].gap_min, d.pauses[0].kind), (60, PauseKind::Normal));
            assert_eq!((d.pauses[1].gap_min, d.pauses[1].kind, d.pauses[1].counted_min, d.pauses[1].extra_pay), (240, PauseKind::Normal, 240, 10.0));
            let day = &d.days[0];
            assert_eq!((day.legs, day.pay_legs, day.pay_exterior, day.trip_min, day.station_min), (3, 360.0, 0.0, 180 + 240 + 180, 60 + 240));
            assert_eq!(day.pay_total, 370.0);
            // Leaving from Constanța after arriving in Năvodari is the same area: no "loc diferit". Arriving in Otopeni
            // and leaving Constanța is, as before, only the first pause's problem (it arrives in Otopeni, next leaves Otopeni).
            assert!(d.pauses.iter().all(|p| p.alerts.iter().all(|a| a.code != "place_differs")), "{:?}", d.pauses);
        }
    }

    #[test]
    fn a_private_run_toward_otopeni_from_outside_the_county_is_the_same_kind_of_run() {
        // Tulcea → Otopeni: no tariff, not an excursion, not a question for the office: a leg like any other.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, "Tulcea", "Aeroport Otopeni Bucuresti", "2026-10-05 10:00", Some("2026-10-05 14:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 17:00", Some("2026-10-05 20:00")),
        ]);
        run(&mut d);
        let tulcea = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(tulcea.eff.regular && tulcea.eff.counts_hours && !tulcea.eff.excursion);
        assert_eq!((tulcea.eff.price, tulcea.eff.price_note.as_str()), (120.0, "cursă"));
        assert_eq!(d.pauses.len(), 2);
        assert!(d.pauses.iter().all(|p| p.kind == PauseKind::Normal), "{:?}", d.pauses);
        // The waiting of that day applies: 08:00 → 10:00 and 14:30 → 17:00.
        assert_eq!((d.pauses[0].counted_min, d.pauses[1].counted_min), (120, 150));
        let day = &d.days[0];
        assert_eq!((day.legs, day.pay_legs, day.pay_exterior), (3, 360.0, 0.0));
        assert!(day.alerts.iter().all(|a| a.code != "no_tariff"), "{:?}", day.alerts);
    }

    #[test]
    fn a_run_that_comes_back_to_otopeni_after_leaving_the_county_is_flagged_as_a_different_place() {
        // The private trip Otopeni → Năvodari, then the next run leaves from Otopeni: he is in Năvodari, not in Otopeni.
        let mut d = driver(vec![
            trip("private-9", TripKind::Private, "Aeroport Otopeni Bucuresti", "Navodari", "2026-10-05 05:00", Some("2026-10-05 09:00")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 12:00", Some("2026-10-05 15:00")),
        ]);
        run(&mut d);
        assert_eq!(d.pauses.len(), 1);
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "place_differs" && a.text.contains("Navodari")), "{:?}", d.pauses[0].alerts);
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
        // Nothing to confirm, nothing missing, nothing counted.
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Rest, 0));
        assert!(d.pauses[0].alerts.iter().all(|a| a.code == "excursion_done"), "{:?}", d.pauses[0].alerts);
        assert!(d.pauses[0].alerts.iter().any(|a| a.text.contains("Chitila")));
        assert!(d.days.iter().all(|day| day.alerts.is_empty()), "no 'fără tarif' for an excursion");
        assert_eq!(d.totals.pay_exterior, 0.0);
        // A price typed by hand makes it a paid trip: the office decides again.
        d.trips.iter_mut().find(|t| t.id == "private-9").unwrap().edit.price = Some(100.0);
        run(&mut d);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
        assert_eq!(d.pauses[0].kind, PauseKind::Confirm);
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "excursion" && a.text.contains("Chitila")));
        // A place the office says is normal for this firm is not far at all.
        d.trips.iter_mut().find(|t| t.id == "private-9").unwrap().edit.price = None;
        let mut settings = Settings::default();
        settings.catb.extra_local_places = vec!["Chitila".into()];
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(d.pauses[0].kind, PauseKind::Normal);
    }

    #[test]
    fn a_far_trip_with_a_tariff_is_still_paid_and_asks_the_office() {
        let mut d = with_private_trip_in_between("Constanta", "Tulcea", Some("2026-10-05 11:30"));
        run(&mut d);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
        assert_eq!(d.days[0].pay_exterior, 110.0);
        assert_eq!(d.pauses[0].kind, PauseKind::Confirm);
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
    fn an_excursion_leaves_categoria_b_completely() {
        // Back from Otopeni at 08:00; a private trip to Albac (no duration); the next regular trip 4 days later.
        // Albac has a tariff here, so the trip is paid work until the office says it was an excursion.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("private-9", TripKind::Private, "Constanta", "Albac", "2026-10-06 06:00", None),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-10 09:30", Some("2026-10-10 12:30")),
        ]);
        let mut settings = Settings::default();
        settings.catb.tariffs.push(super::tariffs::Tariff { name: "Albac".into(), amount: 150.0 });
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!((d.pauses[0].kind, d.pauses[0].counted_min), (PauseKind::Confirm, 0), "undecided: nothing counted yet");
        assert_eq!(d.days.iter().map(|day| day.pay_exterior).sum::<f64>(), 150.0);
        let before = d.totals.total_min;

        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Excursion, ..Default::default() });
        recompute_driver(&mut d, 2026, 10, &settings);
        let pause = &d.pauses[0];
        assert_eq!((pause.counted_min, pause.extra_pay), (0, 0.0), "the whole pause stays out");
        assert!(pause.alerts.iter().any(|a| a.code == "excursion_done" && a.text.contains("Albac") && a.text.contains("tahograf")), "{:?}", pause.alerts);
        let albac = d.trips.iter().find(|t| t.id == "private-9").unwrap();
        assert!(albac.eff.excursion && !albac.eff.counts_hours);
        assert_eq!((albac.eff.price, albac.eff.price_note.as_str()), (0.0, EXCURSION_NOTE));
        assert!(d.days.iter().all(|day| day.alerts.is_empty()), "an excursion trip raises no 'fără tarif'");
        assert_eq!(d.totals.total_min, before, "the excursion adds nothing");
        assert_eq!(d.totals.pay_exterior, 0.0);

        // Hours typed by hand do not bring it back in.
        d.pause_edits[0].hours = Some(20.0);
        recompute_driver(&mut d, 2026, 10, &settings);
        assert_eq!(d.pauses[0].counted_min, 0);
        // Choosing something else puts the trip back.
        d.pause_edits[0].state = PauseState::Stayed;
        recompute_driver(&mut d, 2026, 10, &settings);
        assert!(!d.trips.iter().find(|t| t.id == "private-9").unwrap().eff.excursion);
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
        // Arrives in Bucharest, the next trip leaves from Constanța: he came back some other way.
        let mut d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 05:00", Some("2026-10-05 08:00")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-10-05 15:00", Some("2026-10-05 18:00")),
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
