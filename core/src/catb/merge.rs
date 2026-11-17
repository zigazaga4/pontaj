//! Puts a fresh download of the API into the month's sheet without losing what the office changed.
//!
//! The API side of a trip (times, places, car…) is replaced; its `edit`, the pause decisions, the excluded days and
//! the trips added by hand stay. A trip that vanished from the API disappears too, unless somebody edited it:
//! then it stays and a notice says so.

use std::collections::BTreeMap;

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
        passengers: row.passengers,
        ..Default::default()
    }
}

/// Copies the API fields of `row` into `trip`; true when something differed.
fn refresh(trip: &mut Trip, row: &ApiRow) -> bool {
    let fresh = from_row(row);
    let changed = (trip.kind, &trip.label, &trip.from, &trip.to, trip.departure, trip.arrival, trip.return_at, &trip.car, trip.passengers)
        != (fresh.kind, &fresh.label, &fresh.from, &fresh.to, fresh.departure, fresh.arrival, fresh.return_at, &fresh.car, fresh.passengers);
    if changed {
        *trip = Trip { edit: std::mem::take(&mut trip.edit), eff: std::mem::take(&mut trip.eff), ..fresh };
    }
    changed
}

fn is_edited(trip: &Trip) -> bool {
    trip.edit != TripEdit::default()
}

fn has_decisions(driver: &Driver) -> bool {
    !driver.pause_edits.is_empty() || !driver.excluded_days.is_empty() || driver.trips.iter().any(|t| t.manual || is_edited(t))
}

fn short(trip: &Trip) -> String {
    format!("{} {} → {} ({})", trip.departure.format("%d.%m %H:%M"), trip.from, trip.to, trip.id)
}

/// Merges the answer of the API into the sheet (the derived parts must be recomputed afterwards).
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

    for (id, rows) in &by_driver {
        let position = sheet.drivers.iter().position(|d| d.id == *id);
        let driver = match position {
            Some(index) => &mut sheet.drivers[index],
            None => {
                sheet.drivers.push(Driver { id: *id, ..Default::default() });
                sheet.drivers.last_mut().expect("just pushed")
            }
        };
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
                Some(trip) => {
                    if refresh(trip, row) && is_edited(trip) {
                        sheet.notices.push(format!("{}: API-ul a schimbat cursa {}, pe care ai modificat-o. Verifică modificările.", driver.name, short(trip)));
                    }
                }
                None => driver.trips.push(from_row(row)),
            }
        }

        // Trips the API no longer has.
        let name = driver.name.clone();
        let notices = &mut sheet.notices;
        driver.trips.retain(|trip| {
            let gone = !trip.manual && !seen.contains(&trip.id.as_str());
            if gone && is_edited(trip) {
                notices.push(format!("{name}: cursa {} nu mai există în API, dar ai modificat-o; am păstrat-o.", short(trip)));
            }
            !gone || is_edited(trip)
        });
    }

    // Drivers the API no longer has.
    let notices = &mut sheet.notices;
    sheet.drivers.retain(|driver| {
        if by_driver.contains_key(&driver.id) {
            return true;
        }
        let keep = has_decisions(driver);
        if keep {
            notices.push(format!("{}: nu mai apare în API, dar are modificări; l-am păstrat.", driver.name));
        }
        keep
    });
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
            passengers: 10,
        }
    }

    fn first_download() -> Sheet {
        let mut sheet = new_sheet(2026, 10);
        let rows = vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00"),
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
    fn edits_survive_a_second_download() {
        let mut sheet = first_download();
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.trips[0].edit.departure = parse_time("2026-10-05 06:00");
        popescu.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, note: "foaia: a stat".into(), ..Default::default() });
        popescu.excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());
        popescu.trips.push(Trip { id: "manual-1".into(), kind: TripKind::Passenger, manual: true, departure: parse_time("2026-10-05 15:00").unwrap(), ..Default::default() });

        // The API now says eco-2 leaves at 10:30 and has a third trip; Ionescu is gone.
        let rows = vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            row(1, "Popescu Ion", "eco-2", "2026-10-05 10:30", "2026-10-05 13:30"),
            row(1, "Popescu Ion", "eco-4", "2026-10-07 05:00", "2026-10-07 08:00"),
        ];
        merge_rows(&mut sheet, &rows, 0, "2026-10-31 18:00");
        recompute(&mut sheet, &Settings::default());

        assert_eq!(sheet.drivers.len(), 1, "Ionescu has no edits, so he leaves with the API");
        let popescu = &sheet.drivers[0];
        assert_eq!(popescu.trips.len(), 4, "3 from the API + the one added by hand");
        assert_eq!(popescu.trips.iter().find(|t| t.id == "eco-1").unwrap().edit.departure, parse_time("2026-10-05 06:00"));
        assert_eq!(popescu.trips.iter().find(|t| t.id == "eco-2").unwrap().departure, parse_time("2026-10-05 10:30").unwrap(), "API fields are replaced");
        assert_eq!((popescu.pause_edits.len(), popescu.excluded_days.len()), (1, 1));
        assert_eq!(sheet.fetched_at.as_deref(), Some("2026-10-31 18:00"));
    }

    #[test]
    fn a_vanished_trip_is_dropped_unless_somebody_edited_it() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap().trips[1].edit.price = Some(100.0);
        // eco-1 (not edited) and eco-2 (edited) are both gone from the API.
        let rows = vec![row(1, "Popescu Ion", "eco-9", "2026-10-08 05:00", "2026-10-08 08:00"), row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00")];
        merge_rows(&mut sheet, &rows, 2, "x");
        let ids: Vec<&str> = sheet.drivers.iter().find(|d| d.id == 1).unwrap().trips.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["eco-2", "eco-9"]);
        assert!(sheet.notices.iter().any(|n| n.contains("nu mai există în API") && n.contains("eco-2")));
        assert!(sheet.notices.iter().any(|n| n.starts_with("2 rânduri")));
        // Notices are rebuilt by each download.
        merge_rows(&mut sheet, &rows, 0, "y");
        assert!(sheet.notices.iter().all(|n| !n.starts_with("2 rânduri")));
    }

    #[test]
    fn a_change_in_an_edited_trip_is_reported() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap().trips[0].edit.arrival = parse_time("2026-10-05 09:00");
        let rows = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:15", "2026-10-05 08:15"), row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00"), row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00")];
        merge_rows(&mut sheet, &rows, 0, "z");
        assert_eq!(sheet.notices.len(), 1, "{:?}", sheet.notices);
        assert!(sheet.notices[0].contains("a schimbat cursa"));
    }

    #[test]
    fn a_driver_with_decisions_stays_even_when_the_api_forgets_him() {
        let mut sheet = first_download();
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap());
        let rows = vec![row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00")];
        merge_rows(&mut sheet, &rows, 0, "z");
        assert_eq!(sheet.drivers.len(), 2);
        assert!(sheet.notices.iter().any(|n| n.contains("Ionescu Vasile") && n.contains("l-am păstrat")));
    }
}
