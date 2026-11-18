//! Puts a fresh download of the API into the month's sheet and brings in only what is new.
//!
//! The office works on the sheet for days (checking and correcting), so a second download must not touch any of that:
//! a driver or a trip (by its `trip.id`) that is already in the sheet stays exactly as it is, edits, pause decisions,
//! excluded days and trips added by hand included. Only drivers and trips the sheet does not have yet are added.
//! If the API now says something else about a trip the sheet already has, or no longer has it, nothing changes: a
//! notice tells the office, who decides.

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
fn differs(trip: &Trip, row: &ApiRow) -> bool {
    let fresh = from_row(row);
    (trip.kind, &trip.label, &trip.from, &trip.to, trip.departure, trip.arrival, trip.return_at, &trip.car, trip.passengers)
        != (fresh.kind, &fresh.label, &fresh.from, &fresh.to, fresh.departure, fresh.arrival, fresh.return_at, &fresh.car, fresh.passengers)
}

/// The car's make and seats are filled in only where the sheet has none: a month saved before they were kept learns
/// them now, and nothing the sheet already has is replaced.
fn fill_car_details(trip: &mut Trip, row: &ApiRow) {
    if trip.car_model.is_empty() && trip.car_seats == 0 && trip.car_category.is_empty() {
        (trip.car_model, trip.car_seats, trip.car_category) = (row.car_model.clone(), row.car_seats, row.car_category.clone());
    }
}

fn is_edited(trip: &Trip) -> bool {
    trip.edit != TripEdit::default()
}

/// Has the office decided anything about this driver (so he must not disappear on his own)?
pub(super) fn has_decisions(driver: &Driver) -> bool {
    !driver.pause_edits.is_empty() || !driver.excluded_days.is_empty() || !driver.leaves.is_empty() || !driver.fills.is_empty() || driver.trips.iter().any(|t| t.manual || is_edited(t))
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

/// Adds what is new in the answer of the API to the sheet and leaves the rest untouched (the derived parts must be
/// recomputed afterwards).
pub fn merge_rows(sheet: &mut Sheet, rows: &[ApiRow], skipped: usize, fetched_at: &str) {
    sheet.notices.clear();
    sheet.fetched_at = Some(fetched_at.to_string());
    if skipped > 0 {
        sheet.notices.push(format!("{skipped} rânduri din API nu au putut fi citite (dată sau cod lipsă) și au fost ignorate."));
    }

    let mut by_driver: BTreeMap<i64, Vec<&ApiRow>> = BTreeMap::new();
    for row in rows {
        by_driver.entry(row.driver_id).or_default().push(row);
    }
    // Trips already in the sheet that the API now describes differently / no longer has.
    let (mut changed, mut gone): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());

    for (id, rows) in &by_driver {
        let position = sheet.drivers.iter().position(|d| d.id == *id);
        let driver = match position {
            Some(index) => &mut sheet.drivers[index],
            None => {
                sheet.drivers.push(Driver { id: *id, ..Default::default() });
                sheet.drivers.last_mut().expect("just pushed")
            }
        };
        // A name the sheet already has stays (the office may have corrected it).
        if driver.name.is_empty() {
            if let Some(name) = rows.iter().map(|r| r.driver_name.as_str()).find(|n| !n.is_empty()) {
                driver.name = name.to_string();
            }
        }

        let mut seen: Vec<&str> = Vec::new();
        for row in rows {
            if seen.contains(&row.trip_id.as_str()) {
                sheet.notices.push(format!("{}: cursa {} apare de două ori în API; am păstrat prima.", driver.name, row.trip_id));
                continue;
            }
            seen.push(&row.trip_id);
            match driver.trips.iter_mut().find(|t| !t.manual && t.id == row.trip_id) {
                // Already in the sheet: untouched, only reported when the API now says something else.
                Some(trip) => {
                    fill_car_details(trip, row);
                    if differs(trip, row) {
                        changed.push(format!("{}: {}", driver.name, short(trip)));
                    }
                }
                None => driver.trips.push(from_row(row)),
            }
        }

        for trip in driver.trips.iter().filter(|t| !t.manual && !t.generated && !seen.contains(&t.id.as_str())) {
            gone.push(format!("{}: {}", driver.name, short(trip)));
        }
    }

    if !changed.is_empty() {
        let what = if changed.len() == 1 { "are alte date în API; am lăsat-o neatinsă" } else { "au alte date în API; le-am lăsat neatinse" };
        sheet.notices.push(format!("{} din foaie {what}: {}.", count_trips(changed.len()), listed(&changed)));
    }
    if !gone.is_empty() {
        let what = if gone.len() == 1 { "nu mai apare în API; am păstrat-o" } else { "nu mai apar în API; le-am păstrat" };
        sheet.notices.push(format!("{} din foaie {what}: {}.", count_trips(gone.len()), listed(&gone)));
    }

    // Drivers the API no longer lists stay too, with everything they have.
    let notices = &mut sheet.notices;
    for driver in sheet.drivers.iter().filter(|d| !by_driver.contains_key(&d.id)) {
        notices.push(if has_decisions(driver) {
            format!("{}: nu mai apare în API, dar are modificări; l-am păstrat.", driver.name)
        } else {
            format!("{}: nu mai apare în API; l-am păstrat cu cursele pe care le avea.", driver.name)
        });
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

fn count_trips(count: usize) -> String {
    if count == 1 { "1 cursă".into() } else { format!("{count} curse") }
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::super::compute::recompute;
    use super::*;
    use crate::settings::Settings;

    fn row(driver: i64, name: &str, id: &str, departure: &str, arrival: &str) -> ApiRow {
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
    fn back(mut row: ApiRow) -> ApiRow {
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
    fn a_second_download_brings_in_only_the_new_trips_and_touches_nothing_else() {
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
        for old in &before.drivers {
            let now = sheet.drivers.iter().find(|d| d.id == old.id).unwrap();
            assert_eq!(now.name, old.name, "a corrected name stays");
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
    fn a_trip_the_api_no_longer_has_stays_and_a_notice_says_so() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap().trips[1].edit.price = Some(100.0);
        // eco-1 and eco-2 are both gone from the API (edited or not), eco-9 is new.
        let rows = vec![row(1, "Popescu Ion", "eco-9", "2026-10-08 05:00", "2026-10-08 08:00"), row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00")];
        merge_rows(&mut sheet, &rows, 2, "x");
        let ids: Vec<&str> = sheet.drivers.iter().find(|d| d.id == 1).unwrap().trips.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["eco-1", "eco-2", "eco-9"]);
        assert!(sheet.notices.iter().any(|n| n.starts_with("2 curse din foaie nu mai apar în API") && n.contains("eco-1") && n.contains("eco-2")), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().any(|n| n.starts_with("2 rânduri")));
        // Notices are rebuilt by each download.
        merge_rows(&mut sheet, &rows, 0, "y");
        assert!(sheet.notices.iter().all(|n| !n.starts_with("2 rânduri")));
    }

    #[test]
    fn a_trip_the_api_now_describes_differently_is_left_as_it_is_and_reported() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap().trips[0].edit.arrival = parse_time("2026-10-05 09:00");
        let rows = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:15", "2026-10-05 08:15"), back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")), row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00")];
        merge_rows(&mut sheet, &rows, 0, "z");
        assert_eq!(sheet.notices.len(), 1, "{:?}", sheet.notices);
        assert!(sheet.notices[0].starts_with("1 cursă din foaie are alte date în API") && sheet.notices[0].contains("eco-1"), "{:?}", sheet.notices);
        let trip = &sheet.drivers.iter().find(|d| d.id == 1).unwrap().trips[0];
        assert_eq!(trip.departure, parse_time("2026-10-05 05:00").unwrap(), "the sheet keeps what it had");
        assert_eq!(trip.edit.arrival, parse_time("2026-10-05 09:00"));
    }

    #[test]
    fn the_days_around_the_month_are_neighbors_and_not_trips() {
        use super::super::api::Parsed;
        let mut coach = row(1, "Badea Lucian", "eco-1", "2026-09-29 05:00", "2026-09-29 08:00");
        (coach.car_model, coach.car_seats) = ("TOURISMO".into(), 55);
        let rows = vec![
            coach,
            back(row(1, "Badea Lucian", "eco-30", "2026-10-31 11:00", "2026-10-31 14:00")),
            back(row(1, "Badea Lucian", "eco-31", "2026-11-01 05:30", "2026-11-01 08:30")),
            row(1, "Badea Lucian", "eco-32", "2026-11-03 05:00", "2026-11-03 08:00"),
            back(row(1, "Badea Lucian", "eco-33", "2026-12-20 05:00", "2026-12-20 08:00")),
            row(2, "Doar Context", "eco-40", "2026-10-30 05:00", "2026-10-30 08:00"),
        ];
        let sheet = super::super::apply_download(None, 2026, 11, &Parsed { rows, ..Default::default() }, "x", &Settings::default());
        // Driver 1 drove a coach in September (outside the month): that does not make him category D. Driver 2 has no trip
        // in November, so he is not in the sheet at all.
        assert_eq!(sheet.drivers.len(), 1);
        let badea = &sheet.drivers[0];
        let ids = |trips: &[Trip]| trips.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&badea.before), ["eco-1", "eco-30"]);
        assert_eq!(ids(&badea.after), ["eco-33"]);
        // The sheet's trips are those of November, and the drive that explains how he got to Otopeni on the 1st is there.
        assert_eq!(ids(&badea.trips), ["gen-pre-eco-31", "eco-31", "eco-32"]);
        assert!(sheet.notices.is_empty(), "{:?}", sheet.notices);

        // A download with no trip in the month says so, and the same data twice changes nothing.
        let empty = super::super::apply_download(None, 2026, 8, &Parsed { rows: vec![row(1, "X", "eco-9", "2026-09-29 05:00", "2026-09-29 08:00")], ..Default::default() }, "x", &Settings::default());
        assert!(empty.notices.iter().any(|n| n.contains("nicio cursă")), "{:?}", empty.notices);
        let again = super::super::apply_download(Some(sheet.clone()), 2026, 11, &Parsed { rows: vec![], ..Default::default() }, "y", &Settings::default());
        assert_eq!(again.drivers[0].trips, sheet.drivers[0].trips, "generated trips are not 'gone from the API'");
        assert!(again.notices.iter().all(|n| !n.contains("din foaie")), "{:?}", again.notices);
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
    fn a_driver_the_api_forgets_stays() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap());
        let rows = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00")];
        merge_rows(&mut sheet, &rows, 0, "z");
        assert_eq!(sheet.drivers.len(), 2);
        assert!(sheet.notices.iter().any(|n| n.contains("Ionescu Vasile") && n.contains("dar are modificări; l-am păstrat")));
        // Without any change by the office he stays too, with a plainer notice.
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.clear();
        merge_rows(&mut sheet, &rows, 0, "w");
        assert_eq!(sheet.drivers.len(), 2);
        assert!(sheet.notices.iter().any(|n| n.contains("Ionescu Vasile: nu mai apare în API; l-am păstrat")), "{:?}", sheet.notices);
    }
}
