//! Puts a fresh answer of the API into the month's sheet: the API is right about what the API knows, the office about
//! what the office decided.
//!
//! The dispatchers correct trips in the other application for days (a wrong car, a moved hour, a cancelled trip), so every
//! download takes the API's version of a trip (by its `trip.id`): new trips come in, changed ones are replaced, ones the
//! API no longer lists leave. What the office did is never touched: the changes it typed on a trip (`Trip::edit`), trips
//! added by hand, pause decisions, excluded days, leave, completion hours. A trip or a driver the office changed stays even
//! if the API no longer lists it, and where the API now says something else than the office typed, the office's value
//! stays and a notice says so.

use std::collections::BTreeMap;

use chrono::Datelike;

use super::api::ApiRow;
use super::model::*;
use crate::calendar;
use crate::timesheet::month_days;

/// An empty sheet for a month.
pub fn new_sheet(year: i32, month: u32) -> Sheet {
    Sheet { year, month, days: month_days(year, month), weekdays: calendar::weekdays_in_month(year, month), ..Default::default() }
}

fn from_row(row: &ApiRow) -> Trip {
    Trip {
        id: row.trip_id.clone(),
        kind: row.kind,
        label: row.label.clone(),
        from: row.from.clone(),
        to: row.to.clone(),
        departure: row.departure,
        arrival: row.arrival,
        return_at: row.return_at,
        car: row.car.clone(),
        car_model: row.car_model.clone(),
        car_seats: row.car_seats,
        car_category: row.car_category.clone(),
        passengers: row.passengers,
        ..Default::default()
    }
}

/// Does the API say something else about `trip` than the sheet has (the API side of the trip, not the office's edits)?
/// The car's make and seats are not compared: a month saved before they were kept learns them without it being a correction.
fn differs(trip: &Trip, row: &ApiRow) -> bool {
    let fresh = from_row(row);
    (trip.kind, &trip.label, &trip.from, &trip.to, trip.departure, trip.arrival, trip.return_at, &trip.car, trip.passengers)
        != (fresh.kind, &fresh.label, &fresh.from, &fresh.to, fresh.departure, fresh.arrival, fresh.return_at, &fresh.car, fresh.passengers)
}

/// Does the API now say something else about a field the office typed a value for? (The office's value stays.)
fn contradicts_edit(trip: &Trip, row: &ApiRow) -> bool {
    let (edit, fresh) = (&trip.edit, from_row(row));
    (edit.departure.is_some() && trip.departure != fresh.departure)
        || (edit.arrival.is_some() && trip.arrival != fresh.arrival)
        || (edit.from.is_some() && trip.from != fresh.from)
        || (edit.to.is_some() && trip.to != fresh.to)
}

/// Takes the API's side of the trip. The office's edits are a layer on top (`Trip::edit`) and are not part of it.
fn refresh(trip: &mut Trip, row: &ApiRow) {
    let fresh = from_row(row);
    (trip.kind, trip.label, trip.from, trip.to, trip.departure, trip.arrival, trip.return_at) =
        (fresh.kind, fresh.label, fresh.from, fresh.to, fresh.departure, fresh.arrival, fresh.return_at);
    (trip.car, trip.car_model, trip.car_seats, trip.car_category, trip.passengers) = (fresh.car, fresh.car_model, fresh.car_seats, fresh.car_category, fresh.passengers);
}

fn is_edited(trip: &Trip) -> bool {
    trip.edit != TripEdit::default()
}

/// Has the office decided anything about this driver (so he must not disappear on his own)? A decision about the time
/// between two trips counts while both trips are still there.
pub(super) fn has_decisions(driver: &Driver) -> bool {
    let there = |id: &str| driver.trips.iter().any(|t| t.id == id);
    driver.pause_edits.iter().any(|p| there(&p.prev) && there(&p.next))
        || !driver.excluded_days.is_empty()
        || !driver.leaves.is_empty()
        || !driver.fills.is_empty()
        || driver.trips.iter().any(|t| t.manual || is_edited(t))
}

fn short(trip: &Trip) -> String {
    format!("{} {} → {} ({})", trip.departure.format("%d.%m %H:%M"), trip.from, trip.to, trip.id)
}

/// "a; b; c și încă 4" — a notice must not become a wall of text.
fn listed(items: &[String]) -> String {
    const SHOWN: usize = 5;
    match items.len() {
        n if n <= SHOWN => items.join("; "),
        n => format!("{}; și încă {}", items[..SHOWN].join("; "), n - SHOWN),
    }
}

/// The notice about what the API corrected in the sheet starts like this. It stays on screen until the next correction:
/// a quiet download that finds nothing to correct does not wipe it.
pub(super) const CORRECTED_NOTICE: &str = "Din API:";

/// The notice about the office's trips that the API now disagrees with has this in it. It stays like the one above: the
/// refreshes that follow find nothing new to say, but the office should still be able to read it.
const CONFLICT_MARK: &str = "am lăsat ce ai scris tu:";

/// Notices that tell what happened once, when the API changed (the others are rebuilt from the sheet on every download).
fn is_event_notice(notice: &str) -> bool {
    notice.starts_with(CORRECTED_NOTICE) || notice.contains(CONFLICT_MARK)
}

/// Do both notices tell about the same kind of event (so that a new one replaces the old one)?
fn same_event(a: &str, b: &str) -> bool {
    a.starts_with(CORRECTED_NOTICE) == b.starts_with(CORRECTED_NOTICE) && a.contains(CONFLICT_MARK) == b.contains(CONFLICT_MARK)
}

/// What a download changed in the trips the sheet already had.
#[derive(Default)]
struct Report {
    /// Replaced by the API's version (nothing of the office's was in the way).
    corrected: Vec<String>,
    /// Trips the office edited, where the API now says something else than the office typed: the office's value stays.
    contradicted: Vec<String>,
    /// Trips the API no longer lists (cancelled, moved to another month, now on a category D car): taken out.
    removed: Vec<String>,
    /// The same, but the office had changed them, so they stay.
    kept: Vec<String>,
    /// Drivers who have no trip left in the month and nothing the office decided: taken out.
    dropped: Vec<String>,
}

impl Report {
    fn notices(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut done = Vec::new();
        if !self.corrected.is_empty() {
            done.push(format!("{} corectat{} (mașina, ora sau locul): {}", count_trips(self.corrected.len()), if self.corrected.len() == 1 { "ă" } else { "e" }, listed(&self.corrected)));
        }
        if !self.removed.is_empty() {
            done.push(format!("{} scoas{} (nu mai sunt în API pentru Categoria B): {}", count_trips(self.removed.len()), if self.removed.len() == 1 { "ă" } else { "e" }, listed(&self.removed)));
        }
        if !self.dropped.is_empty() {
            done.push(format!("fără curse în lună: {}", listed(&self.dropped)));
        }
        if !done.is_empty() {
            out.push(format!("{CORRECTED_NOTICE} {}.", done.join("; ")));
        }
        if !self.contradicted.is_empty() {
            out.push(format!("{} modificat{} de tine {} alte date în API; {CONFLICT_MARK} {}.", count_trips(self.contradicted.len()), if self.contradicted.len() == 1 { "ă" } else { "e" }, if self.contradicted.len() == 1 { "are" } else { "au" }, listed(&self.contradicted)));
        }
        if !self.kept.is_empty() {
            out.push(format!("{} modificat{} de tine nu {} în lista API pentru Categoria B (șterse, mutate în altă lună sau pe o mașină de categoria D); {}: {}.", count_trips(self.kept.len()), if self.kept.len() == 1 { "ă" } else { "e" }, if self.kept.len() == 1 { "mai este" } else { "mai sunt" }, if self.kept.len() == 1 { "am păstrat-o" } else { "le-am păstrat" }, listed(&self.kept)));
        }
        out
    }
}

/// Puts the answer of the API into the sheet (the derived parts must be recomputed afterwards). Nothing is taken out
/// when the answer has no rows: an empty answer is far more likely a hiccup of the API than a month wiped clean.
pub fn merge_rows(sheet: &mut Sheet, rows: &[ApiRow], skipped: usize, fetched_at: &str) {
    sync_rows(sheet, rows, skipped, fetched_at, !rows.is_empty());
}

/// [`merge_rows`], with the caller saying whether the answer is complete enough to take trips out of the sheet.
pub(super) fn sync_rows(sheet: &mut Sheet, rows: &[ApiRow], skipped: usize, fetched_at: &str, complete: bool) {
    let carried: Vec<String> = sheet.notices.iter().filter(|n| is_event_notice(n)).cloned().collect();
    sheet.notices.clear();
    sheet.fetched_at = Some(fetched_at.to_string());
    if skipped > 0 {
        sheet.notices.push(format!("{skipped} rânduri din API nu au putut fi citite (dată sau cod lipsă) și au fost ignorate."));
    }

    let mut by_driver: BTreeMap<i64, Vec<&ApiRow>> = BTreeMap::new();
    for row in rows {
        by_driver.entry(row.driver_id).or_default().push(row);
    }
    let mut report = Report::default();

    for (id, rows) in &by_driver {
        let position = sheet.drivers.iter().position(|d| d.id == *id);
        let driver = match position {
            Some(index) => &mut sheet.drivers[index],
            None => {
                sheet.drivers.push(Driver { id: *id, ..Default::default() });
                sheet.drivers.last_mut().expect("just pushed")
            }
        };
        // The name is the API's (the sheet has no way to change it by hand), so a corrected spelling arrives too.
        if let Some(name) = rows.iter().map(|r| r.driver_name.as_str()).find(|n| !n.is_empty()) {
            driver.name = name.to_string();
        }

        let mut seen: Vec<&str> = Vec::new();
        for row in rows {
            if seen.contains(&row.trip_id.as_str()) {
                sheet.notices.push(format!("{}: cursa {} apare de două ori în API; am păstrat prima.", driver.name, row.trip_id));
                continue;
            }
            seen.push(&row.trip_id);
            match driver.trips.iter_mut().find(|t| !t.manual && t.id == row.trip_id) {
                // Already in the sheet: the API's side is replaced; the office's edits are another layer and stay.
                Some(trip) => {
                    let (changed, contradicted) = (differs(trip, row), contradicts_edit(trip, row));
                    refresh(trip, row);
                    if changed {
                        let list = if contradicted { &mut report.contradicted } else { &mut report.corrected };
                        list.push(format!("{}: {}", driver.name, short(trip)));
                    }
                }
                None => driver.trips.push(from_row(row)),
            }
        }
    }

    if complete {
        // A trip the API no longer lists leaves, unless the office changed it. A driver left with no trip leaves too,
        // unless the office decided something about him.
        for driver in &mut sheet.drivers {
            let listed_ids: Vec<&str> = by_driver.get(&driver.id).map(|rows| rows.iter().map(|r| r.trip_id.as_str()).collect()).unwrap_or_default();
            let name = driver.name.clone();
            driver.trips.retain(|trip| {
                if trip.manual || trip.generated || listed_ids.contains(&trip.id.as_str()) {
                    return true;
                }
                if is_edited(trip) {
                    report.kept.push(format!("{name}: {}", short(trip)));
                    return true;
                }
                report.removed.push(format!("{name}: {}", short(trip)));
                false
            });
        }
        sheet.drivers.retain(|driver| {
            let keeps = driver.trips.iter().any(|t| !t.generated) || has_decisions(driver);
            if !keeps {
                report.dropped.push(driver.name.clone());
            }
            keeps
        });
    }

    // What happened earlier stays until something of the same kind happens again.
    let notices = report.notices();
    sheet.notices.extend(carried.into_iter().filter(|old| !notices.iter().any(|new| same_event(new, old))));
    sheet.notices.extend(notices);

    if !complete {
        if !sheet.drivers.is_empty() {
            sheet.notices.push("API-ul nu a trimis nicio cursă, așa că n-am scos nimic din foaie.".into());
        }
        return;
    }
    // A driver the API lists nothing for but who has changes of the office stays: say why he is still there.
    for driver in sheet.drivers.iter().filter(|d| !by_driver.contains_key(&d.id)) {
        sheet.notices.push(format!("{}: nu mai are curse în API pentru luna aceasta, dar are modificări; l-am păstrat.", driver.name));
    }
}

/// How many trips on each side of the month a driver keeps (the nearest ones): enough to find a regular trip even when the
/// nearest are private or excursions.
const NEIGHBOR_TRIPS: usize = 8;

/// Splits the rows of a download: those that leave in the month (the sheet's trips) and the others (the days around it).
pub fn split_month(rows: &[ApiRow], year: i32, month: u32) -> (Vec<ApiRow>, Vec<ApiRow>) {
    rows.iter().cloned().partition(|row| (row.departure.year(), row.departure.month()) == (year, month))
}

/// Notes, for each driver of the sheet, his nearest trips before and after the month (see [`Driver::before`]). They come
/// from the API only, so a download replaces them: the office has nothing to protect there. A driver the answer has no
/// trip around the month for keeps what he had.
pub fn set_neighbors(sheet: &mut Sheet, around: &[ApiRow]) {
    let Some(start) = chrono::NaiveDate::from_ymd_opt(sheet.year, sheet.month, 1).and_then(|d| d.and_hms_opt(0, 0, 0)) else { return };
    for driver in &mut sheet.drivers {
        let mut mine: Vec<&ApiRow> = around.iter().filter(|r| r.driver_id == driver.id).collect();
        // An answer that says nothing about the driver changes nothing: the generated trips must not vanish because the API
        // was silent once.
        if mine.is_empty() {
            continue;
        }
        mine.sort_by(|a, b| (a.departure, &a.trip_id).cmp(&(b.departure, &b.trip_id)));
        mine.dedup_by(|a, b| a.trip_id == b.trip_id);
        let (before, after): (Vec<&ApiRow>, Vec<&ApiRow>) = mine.into_iter().partition(|r| r.departure < start);
        driver.before = before.iter().rev().take(NEIGHBOR_TRIPS).rev().map(|r| from_row(r)).collect();
        driver.after = after.iter().take(NEIGHBOR_TRIPS).map(|r| from_row(r)).collect();
    }
}

/// A month saved before the days around it were kept (or never downloaded again) knows nothing of the trips just outside
/// it, but the months saved beside it do: their trips are exactly those. They go into [`Driver::before`] and
/// [`Driver::after`] (the nearest ones), so the drives between months are found without downloading again. A trip as the
/// neighbor's sheet has it wins over the one the sheet had: it carries what the office did to it (a trip taken out stays out).
pub fn borrow_neighbors(sheet: &mut Sheet, previous: Option<&Sheet>, next: Option<&Sheet>) {
    for driver in &mut sheet.drivers {
        if let Some(theirs) = previous.and_then(|s| s.drivers.iter().find(|d| d.id == driver.id)) {
            driver.before = nearest(std::mem::take(&mut driver.before), &theirs.trips, true);
        }
        if let Some(theirs) = next.and_then(|s| s.drivers.iter().find(|d| d.id == driver.id)) {
            driver.after = nearest(std::mem::take(&mut driver.after), &theirs.trips, false);
        }
    }
}

/// The driver's own context joined with the trips of the neighbor month, in time order: the last [`NEIGHBOR_TRIPS`] (`last`,
/// the days before the month) or the first ones (the days after it).
fn nearest(own: Vec<Trip>, theirs: &[Trip], last: bool) -> Vec<Trip> {
    let mut trips: Vec<Trip> = theirs.iter().filter(|t| !t.generated).cloned().collect();
    for trip in own {
        if !trips.iter().any(|t| t.id == trip.id) {
            trips.push(trip);
        }
    }
    trips.sort_by(|a, b| (a.departure, &a.id).cmp(&(b.departure, &b.id)));
    if last {
        trips.drain(..trips.len().saturating_sub(NEIGHBOR_TRIPS));
    } else {
        trips.truncate(NEIGHBOR_TRIPS);
    }
    trips
}

fn count_trips(count: usize) -> String {
    if count == 1 { "1 cursă".into() } else { format!("{count} curse") }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::api::parse_time;
    use super::super::compute::recompute;
    use super::*;
    use crate::settings::Settings;

    pub(in crate::catb) fn row(driver: i64, name: &str, id: &str, departure: &str, arrival: &str) -> ApiRow {
        ApiRow {
            driver_id: driver,
            driver_name: name.into(),
            trip_id: id.into(),
            kind: TripKind::Eco,
            label: String::new(),
            from: "Constanta".into(),
            to: "Aeroport Otopeni Bucuresti".into(),
            departure: parse_time(departure).unwrap(),
            arrival: parse_time(arrival),
            return_at: None,
            car: "CT-22-XDD".into(),
            car_model: "VITO".into(),
            car_seats: 8,
            car_category: "Microbuz 8 locuri".into(),
            passengers: 10,
        }
    }

    /// The same trip the other way round (Otopeni → Constanța): a driver's trips follow each other.
    pub(in crate::catb) fn back(mut row: ApiRow) -> ApiRow {
        std::mem::swap(&mut row.from, &mut row.to);
        row
    }

    fn first_download() -> Sheet {
        let mut sheet = new_sheet(2026, 10);
        let rows = vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")),
            row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00"),
        ];
        merge_rows(&mut sheet, &rows, 0, "2026-10-31 10:00");
        recompute(&mut sheet, &Settings::default());
        sheet
    }

    #[test]
    fn a_first_download_makes_a_driver_per_id() {
        let sheet = first_download();
        assert_eq!(sheet.drivers.len(), 2);
        assert_eq!(sheet.drivers[0].name, "Ionescu Vasile", "sorted by name");
        assert_eq!(sheet.drivers[1].trips.len(), 2);
        assert_eq!(sheet.fetched_at.as_deref(), Some("2026-10-31 10:00"));
        assert_eq!(sheet.cap_min, 176 * 60);
    }

    #[test]
    fn a_second_download_brings_in_the_new_trips_and_never_touches_what_the_office_did() {
        let mut sheet = first_download();
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.trips[0].edit.departure = parse_time("2026-10-05 06:00");
        popescu.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, note: "foaia: a stat".into(), ..Default::default() });
        popescu.excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());
        popescu.trips.push(Trip { id: "manual-1".into(), kind: TripKind::Passenger, manual: true, departure: parse_time("2026-10-05 15:00").unwrap(), ..Default::default() });
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().name = "Ionescu V. (corectat)".into();
        recompute(&mut sheet, &Settings::default());
        let before = sheet.clone();

        // The API now has a new trip for Popescu and a new driver, and nothing else.
        let rows = vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")),
            row(1, "Popescu Ion", "eco-4", "2026-10-07 05:00", "2026-10-07 08:00"),
            row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00"),
            row(3, "Nou Gheorghe", "eco-5", "2026-10-08 05:00", "2026-10-08 08:00"),
        ];
        merge_rows(&mut sheet, &rows, 0, "2026-10-31 18:00");
        recompute(&mut sheet, &Settings::default());

        assert!(sheet.notices.is_empty(), "nothing differs, so nothing to report: {:?}", sheet.notices);
        assert_eq!(sheet.drivers.len(), 3);
        assert_eq!(sheet.drivers.iter().find(|d| d.id == 2).unwrap().name, "Ionescu Vasile", "the name is the API's: a corrected spelling arrives");
        for old in &before.drivers {
            let now = sheet.drivers.iter().find(|d| d.id == old.id).unwrap();
            assert_eq!((&now.pause_edits, &now.excluded_days), (&old.pause_edits, &old.excluded_days));
            for trip in &old.trips {
                assert_eq!(now.trips.iter().find(|t| t.id == trip.id), Some(trip), "{} is as it was", trip.id);
            }
        }
        let popescu = sheet.drivers.iter().find(|d| d.id == 1).unwrap();
        assert_eq!(popescu.trips.len(), before.drivers.iter().find(|d| d.id == 1).unwrap().trips.len() + 1, "only eco-4 is new");
        assert!(popescu.trips.iter().any(|t| t.id == "eco-4"));
        assert_eq!(sheet.drivers.iter().find(|d| d.id == 3).unwrap().trips.len(), 1);
        assert_eq!(sheet.fetched_at.as_deref(), Some("2026-10-31 18:00"));
    }

    #[test]
    fn downloading_the_same_data_again_changes_nothing() {
        let mut sheet = first_download();
        let rows = vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")),
            row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00"),
        ];
        let before = sheet.drivers.clone();
        merge_rows(&mut sheet, &rows, 0, "later");
        assert_eq!(sheet.drivers, before);
        assert!(sheet.notices.is_empty());
    }

    #[test]
    fn a_trip_the_api_no_longer_lists_leaves_unless_the_office_changed_it() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap().trips[1].edit.price = Some(100.0);
        // eco-1 and eco-2 are both gone from the API; the office changed eco-2. eco-9 is new.
        let rows = vec![row(1, "Popescu Ion", "eco-9", "2026-10-08 05:00", "2026-10-08 08:00"), row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00")];
        merge_rows(&mut sheet, &rows, 2, "x");
        let ids: Vec<&str> = sheet.drivers.iter().find(|d| d.id == 1).unwrap().trips.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["eco-2", "eco-9"], "eco-1 is out, eco-2 stays because of the office's price");
        assert!(sheet.notices.iter().any(|n| n.starts_with("Din API: 1 cursă scoasă") && n.contains("eco-1") && !n.contains("eco-2")), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.contains("modificată de tine nu mai este în lista API") && n.contains("eco-2") && n.contains("am păstrat-o")), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.starts_with("2 rânduri")));
        // Notices are rebuilt by each download; the line about what the API corrected stays until the next correction.
        merge_rows(&mut sheet, &rows, 0, "y");
        assert!(sheet.notices.iter().all(|n| !n.starts_with("2 rânduri")));
        assert!(sheet.notices.iter().any(|n| n.starts_with("Din API: 1 cursă scoasă") && n.contains("eco-1")), "{:?}", sheet.notices);
    }

    #[test]
    fn the_api_replaces_what_it_corrected_and_the_office_keeps_what_it_typed() {
        let mut sheet = first_download();
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.trips[0].edit.departure = parse_time("2026-10-05 06:00"); // eco-1: the office moved the hour
        // The dispatchers changed eco-1's hour again, eco-2's car and eco-3's arrival.
        let mut moved = row(1, "Popescu Ion", "eco-1", "2026-10-05 05:30", "2026-10-05 08:30");
        moved.car = "CT-22-XDD".into();
        let mut other_car = back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00"));
        (other_car.car, other_car.car_model, other_car.car_seats) = ("B-704-ALT".into(), "VITO".into(), 7);
        let longer = row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:30");
        merge_rows(&mut sheet, &[moved, other_car, longer], 0, "later");
        recompute(&mut sheet, &Settings::default());

        let popescu = sheet.drivers.iter().find(|d| d.id == 1).unwrap();
        let (eco1, eco2) = (&popescu.trips[0], &popescu.trips[1]);
        assert_eq!((eco1.id.as_str(), eco1.edit.departure), ("eco-1", parse_time("2026-10-05 06:00")), "the office's hour is kept");
        assert_eq!(eco1.eff.departure, parse_time("2026-10-05 06:00"));
        assert_eq!(eco1.departure, parse_time("2026-10-05 05:30").unwrap(), "while the API's side is up to date");
        assert_eq!((eco2.car.as_str(), eco2.car_model.as_str(), eco2.car_seats), ("B-704-ALT", "VITO", 7), "a corrected car replaces the old one");
        let ionescu = sheet.drivers.iter().find(|d| d.id == 2).unwrap();
        assert_eq!(ionescu.trips[0].arrival, parse_time("2026-10-06 08:30"));
        assert!(sheet.notices.iter().any(|n| n.starts_with("Din API: 2 curse corectate") && n.contains("eco-2") && n.contains("eco-3") && !n.contains("eco-1")), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.starts_with("1 cursă modificată de tine are alte date în API") && n.contains("eco-1")), "{:?}", sheet.notices);

        // The same answer again: nothing new, so the sheet is the same and both lines (the corrections, the office's trip) stay.
        let again = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:30", "2026-10-05 08:30"), { let mut r = back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")); (r.car, r.car_model, r.car_seats) = ("B-704-ALT".into(), "VITO".into(), 7); r }, row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:30")];
        let (before, notices) = (sheet.drivers.clone(), sheet.notices.clone());
        merge_rows(&mut sheet, &again, 0, "even later");
        assert_eq!(sheet.drivers.iter().map(|d| &d.trips).collect::<Vec<_>>(), before.iter().map(|d| &d.trips).collect::<Vec<_>>());
        assert_eq!(sheet.notices, notices, "a refresh that finds nothing new says nothing new");
        assert!(sheet.notices.iter().any(|n| n.starts_with("Din API: 2 curse corectate")), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.starts_with("1 cursă modificată de tine are alte date în API") && n.contains("eco-1")), "{:?}", sheet.notices);

        // A new disagreement replaces the old line instead of piling up.
        let mut again_moved = again.clone();
        again_moved[0].departure = parse_time("2026-10-05 05:45").unwrap();
        merge_rows(&mut sheet, &again_moved, 0, "much later");
        assert_eq!(sheet.notices.iter().filter(|n| n.contains("alte date în API")).count(), 1, "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.contains("05:45")), "{:?}", sheet.notices);
    }

    #[test]
    fn an_empty_answer_takes_nothing_out_of_the_sheet() {
        let mut sheet = first_download();
        let before = sheet.drivers.clone();
        merge_rows(&mut sheet, &[], 0, "hiccup");
        assert_eq!(sheet.drivers, before);
        assert!(sheet.notices.iter().any(|n| n.contains("n-am scos nimic")), "{:?}", sheet.notices);
    }

    #[test]
    fn the_days_around_the_month_are_neighbors_and_not_trips() {
        use super::super::api::Parsed;
        let mut coach = row(1, "Testescu Ion", "eco-1", "2026-09-29 05:00", "2026-09-29 08:00");
        (coach.car_model, coach.car_seats) = ("TOURISMO".into(), 55);
        let rows = vec![
            coach,
            back(row(1, "Testescu Ion", "eco-30", "2026-10-31 11:00", "2026-10-31 14:00")),
            back(row(1, "Testescu Ion", "eco-31", "2026-11-01 05:30", "2026-11-01 08:30")),
            row(1, "Testescu Ion", "eco-32", "2026-11-03 05:00", "2026-11-03 08:00"),
            back(row(1, "Testescu Ion", "eco-33", "2026-12-20 05:00", "2026-12-20 08:00")),
            row(2, "Doar Context", "eco-40", "2026-10-30 05:00", "2026-10-30 08:00"),
        ];
        let sheet = super::super::apply_download(None, 2026, 11, &Parsed { rows, ..Default::default() }, "x", &Settings::default());
        // Driver 1 drove a coach in September (outside the month): a category D trip is no neighbor for Categoria B, and it
        // does not take him out of the sheet. Driver 2 has no trip in November, so he is not in the sheet at all.
        assert_eq!(sheet.drivers.len(), 1);
        let testescu = &sheet.drivers[0];
        let ids = |trips: &[Trip]| trips.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&testescu.before), ["eco-30"]);
        assert_eq!(ids(&testescu.after), ["eco-33"]);
        // The sheet's trips are those of November, and the drive that explains how he got to Otopeni on the 1st is there.
        assert_eq!(ids(&testescu.trips), ["gen-pre-eco-31", "eco-31", "eco-32"]);
        assert!(sheet.notices.is_empty(), "{:?}", sheet.notices);

        // A download with no trip in the month says so, and the same data twice changes nothing.
        let empty = super::super::apply_download(None, 2026, 8, &Parsed { rows: vec![row(1, "X", "eco-9", "2026-09-29 05:00", "2026-09-29 08:00")], ..Default::default() }, "x", &Settings::default());
        assert!(empty.notices.iter().any(|n| n.contains("nicio cursă")), "{:?}", empty.notices);
        let again = super::super::apply_download(Some(sheet.clone()), 2026, 11, &Parsed { rows: vec![], ..Default::default() }, "y", &Settings::default());
        assert_eq!(again.drivers[0].trips, sheet.drivers[0].trips, "generated trips are not 'gone from the API'");
        assert!(again.notices.iter().all(|n| !n.starts_with("Din API:")), "an empty answer corrects and removes nothing: {:?}", again.notices);
    }

    #[test]
    fn when_the_api_refuses_the_days_around_the_month_the_old_neighbors_stay_and_the_office_is_told() {
        use super::super::api::Parsed;
        let mut sheet = first_download();
        sheet.drivers[1].before = vec![back(row(1, "Popescu Ion", "eco-0", "2026-09-30 11:00", "2026-09-30 14:00"))].iter().map(from_row).collect();
        let parsed = Parsed { rows: vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00")], without_neighbors: true, ..Default::default() };
        let sheet = super::super::apply_download(Some(sheet), 2026, 10, &parsed, "z", &Settings::default());
        assert_eq!(sheet.drivers.iter().find(|d| d.id == 1).unwrap().before.len(), 1);
        assert!(sheet.notices.iter().any(|n| n.contains("deplasările dintre luni nu se generează")), "{:?}", sheet.notices);
    }

    #[test]
    fn a_saved_month_borrows_the_days_around_it_from_the_months_saved_beside_it() {
        use super::super::api::Parsed;
        let settings = Settings::default();
        let download = |month: u32, rows: Vec<ApiRow>| super::super::apply_download(None, 2026, month, &Parsed { rows, ..Default::default() }, "x", &settings);
        // The last trip of October ends in Constanța; the first of November leaves from Otopeni (nobody logged the drive).
        let october = download(10, vec![back(row(1, "Testescu Ion", "eco-30", "2026-10-31 11:00", "2026-10-31 14:00"))]);
        let november = download(11, vec![back(row(1, "Testescu Ion", "eco-31", "2026-11-01 05:30", "2026-11-01 08:30"))]);
        let made = |sheet: &Sheet| sheet.drivers[0].trips.iter().filter(|t| t.generated).map(|t| t.id.clone()).collect::<Vec<_>>();
        assert!(made(&november).is_empty(), "saved without the days around it: nothing is known about where he was");

        // Opened with October saved beside it, November finds the drive; October (whose next trip is November's) does not
        // count it again — a drive belongs to the month it starts in.
        let mut opened = november.clone();
        borrow_neighbors(&mut opened, Some(&october), None);
        recompute(&mut opened, &settings);
        assert_eq!(made(&opened), ["gen-pre-eco-31"]);
        let mut opened_october = october.clone();
        borrow_neighbors(&mut opened_october, None, Some(&november));
        recompute(&mut opened_october, &settings);
        assert_eq!(opened_october.drivers[0].after.len(), 1);
        assert!(made(&opened_october).is_empty(), "{:?}", made(&opened_october));

        // What the office did to the trip in October is respected: a trip taken out there is not the one he arrived with.
        let mut taken_out = october.clone();
        taken_out.drivers[0].trips[0].edit.removed = true;
        let mut opened = november.clone();
        borrow_neighbors(&mut opened, Some(&taken_out), None);
        recompute(&mut opened, &settings);
        assert!(made(&opened).is_empty(), "{:?}", made(&opened));

        // A driver the neighbor does not have, and no neighbor at all, change nothing.
        let mut alone = download(11, vec![back(row(7, "Alt Șofer", "eco-90", "2026-11-02 05:30", "2026-11-02 08:30"))]);
        let copy = alone.clone();
        borrow_neighbors(&mut alone, Some(&october), None);
        borrow_neighbors(&mut alone, None, None);
        assert_eq!(alone, copy);
    }

    #[test]
    fn a_driver_the_api_forgets_leaves_unless_the_office_decided_something_about_him() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap());
        let rows = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00")];
        merge_rows(&mut sheet, &rows, 0, "z");
        assert_eq!(sheet.drivers.len(), 2);
        assert!(sheet.drivers.iter().find(|d| d.id == 2).unwrap().trips.is_empty(), "his trip is gone from the API");
        assert!(sheet.notices.iter().any(|n| n.contains("Ionescu Vasile: nu mai are curse în API") && n.contains("dar are modificări; l-am păstrat")), "{:?}", sheet.notices);
        // Without any change by the office he leaves the list.
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.clear();
        merge_rows(&mut sheet, &rows, 0, "w");
        assert_eq!(sheet.drivers.len(), 1);
        assert!(sheet.notices.iter().any(|n| n.starts_with("Din API:") && n.contains("fără curse în lună: Ionescu Vasile")), "{:?}", sheet.notices);
    }

    #[test]
    fn a_decision_about_a_pause_counts_only_while_both_trips_are_there() {
        let mut sheet = first_download();
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        assert!(has_decisions(popescu));
        popescu.trips.retain(|t| t.id != "eco-2");
        assert!(!has_decisions(popescu), "the pause it was about is gone");
    }
}
