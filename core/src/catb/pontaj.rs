//! The Categoria B month as the payroll team's *foaie colectivă*: the same [`Timesheet`] the tachograph mode builds, so
//! the Excel sheet is drawn by the same code and looks the same (see [`crate::excel::pontaj_sheet`]).
//!
//! What a day holds: the hours of the day (time on the trips and time added, never the waiting between trips) rounded to
//! whole hours (the rule of the other mode); under them the start and end of every trip of the day, as the screen lists them
//! (12:00–15:00 and 16:00–19:00, never one 12:00–19:00), with as many pairs of rows as the driver's busiest day needs; the
//! night hours measured on those times. The minutes and the day they belong to are the ones of the sheet (a trip counts on
//! its work day), so the Excel shows what the screen shows. See [`trip_intervals`].

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use super::model::{DayRow, Driver, Sheet};
use crate::rules::{compute_totals, recompute_day};
use crate::settings::{IntervalMode, NightSource, Settings};
use crate::time::{fmt_duration, round_hours};
use crate::timesheet::{display_name, DayEntry, Employee, Interval, Timesheet, Totals};

/// A period that counts, on the day it belongs to: `start` and `minutes` from the midnight that opens `day`, so what happens
/// after midnight in a shift that began on `day` is past 24:00 (a trip at 03:30 the next day starts at 27:30).
struct Placed {
    day: NaiveDate,
    start: i32,
    minutes: i32,
}

fn placed(day: NaiveDate, at: NaiveDateTime, minutes: i32) -> Placed {
    Placed { day, start: (at - day.and_time(NaiveTime::MIN)).num_minutes() as i32, minutes }
}

impl Placed {
    fn interval(&self) -> Interval {
        Interval { start: self.start, end: self.start + self.minutes }
    }
}

/// The trips whose hours count: not taken out, not an excursion, with a known duration, each on its work day (see
/// [`super::model::Effective::work_day`]).
fn counted_trips(driver: &Driver) -> impl Iterator<Item = Placed> + '_ {
    driver
        .trips
        .iter()
        .filter(|t| !t.edit.removed && !t.eff.excursion && t.eff.counts_hours && t.eff.minutes > 0)
        .filter_map(|t| t.eff.departure.map(|at| placed(t.eff.work_day.unwrap_or(at.date()), at, t.eff.minutes)))
}

/// The trips of a day as the payroll sheet writes them: each one from its departure to its arrival, in the order they
/// leave (an arrival after midnight is past 24:00), exactly as the screen lists them. The time added to fill the day counts
/// in the hours of the day, not in its times; the waiting between two trips counts in neither. A day with more trips than the
/// usual two gets more rows in the sheet (see `crate::excel::pontaj_sheet`).
pub(super) fn trip_intervals(driver: &Driver, date: NaiveDate) -> Vec<Interval> {
    let mut out: Vec<Interval> = counted_trips(driver).filter(|p| p.day == date).map(|p| p.interval()).collect();
    out.sort_by_key(|i| (i.start, i.end));
    out
}

/// First start and last end (minutes from midnight) of the trips of a day, without the time added to fill it. The waiting
/// between trips is not work time, so it never widens the span.
pub(super) fn day_span(driver: &Driver, date: NaiveDate) -> Option<(i32, i32)> {
    let trips = trip_intervals(driver, date);
    Some((trips.iter().map(|b| b.start).min()?, trips.iter().map(|b| b.end).max()?))
}

/// The remark written beside the hours of a day that got time added to the time of its trips: what the trips took, what was
/// added and where, and the total. `None` when nothing was added.
pub(super) fn added_note(row: &DayRow) -> Option<String> {
    if row.fill_min <= 0 {
        return None;
    }
    let (before, after) = (row.fill_before_min, row.fill_after_min);
    let places = match (before > 0, after > 0) {
        (true, true) => format!("{} înainte de prima plecare, {} după ultima sosire", fmt_duration(before), fmt_duration(after)),
        (true, false) => format!("{} înainte de prima plecare", fmt_duration(before)),
        _ => format!("{} după ultima sosire", fmt_duration(after)),
    };
    let typed = if row.fill_manual { "; scris de birou" } else { "" };
    Some(format!(
        "Timp adăugat la timpul inițial: din cursă {}, adăugat {} ({places}{typed}), în total {}.",
        fmt_duration(row.trip_min),
        fmt_duration(row.fill_min),
        fmt_duration(row.total_min)
    ))
}

/// One day of one driver. A day taken out of the pontaj stays empty; a leave is its code. The hours are the day's total
/// (trips and time added, no waiting) rounded; the start/end times are those of its trips ([`trip_intervals`]).
fn day_entry(driver: &Driver, day: u32, date: NaiveDate, settings: &Settings) -> DayEntry {
    let mut entry = DayEntry { day, ..Default::default() };
    let row: Option<&DayRow> = driver.days.iter().find(|r| r.date == Some(date));
    if let Some(row) = row.filter(|r| !r.excluded) {
        if let Some(code) = &row.leave {
            entry.code = Some(code.clone());
        } else if row.total_min > 0 {
            entry.worked_min = row.total_min;
            entry.note = added_note(row);
            // The span of the day with the time added to it (the times written are only those of the trips).
            if let Some((first, last)) = day_span(driver, date) {
                entry.first_start = Some(first - row.fill_before_min);
                entry.last_end = Some(last + row.fill_after_min);
            }
            // A day that rounds to no hour at all writes nothing (the rules would otherwise count the intervals as typed hours).
            let trips = trip_intervals(driver, date);
            if round_hours(row.total_min, settings.round_up_from_min) > 0 {
                entry.override_intervals = Some(trips.clone()).filter(|w| !w.is_empty());
            }
            // With no trip leaving that day (only a waiting that started after midnight) no time is written either: the
            // rules build the times from the segments in the card style (see `to_timesheet`), and the segments are the trips.
            entry.segments = trips;
        }
    }
    recompute_day(&mut entry, settings);
    entry
}

fn employee(driver: &Driver, sheet: &Sheet, settings: &Settings) -> Employee {
    let days = sheet
        .days
        .iter()
        .map(|info| match NaiveDate::from_ymd_opt(sheet.year, sheet.month, info.day) {
            Some(date) => day_entry(driver, info.day, date, settings),
            None => DayEntry { day: info.day, ..Default::default() },
        })
        .collect();
    let mut employee = Employee {
        name: display_name(&driver.name, settings.plain_names),
        function: settings.default_function.clone(),
        location: String::new(),
        activities: Vec::new(),
        days,
        totals: Totals::default(),
    };
    employee.totals = compute_totals(&employee, sheet.cap_min / 60, settings.hours_per_day);
    employee
}

/// A copy of the sheet whose drivers come in the order of [`to_timesheet`], so every sheet of the workbook lists them
/// alike and the row of a driver in one is the same driver in the others.
pub(super) fn sort_drivers(sheet: &Sheet, settings: &Settings) -> Sheet {
    let mut sorted = sheet.clone();
    sorted.drivers.sort_by_key(|driver| display_name(&driver.name, settings.plain_names));
    sorted
}

/// The month's sheet as a timesheet, drivers in alphabetical order like the payroll file (a stable sort on the name the
/// sheet shows: [`sort_drivers`] gives the same order to the other sheets of the workbook). The norm is the monthly
/// ceiling of Categoria B (8 hours × the Monday–Friday days, holidays included).
pub(super) fn to_timesheet(sheet: &Sheet, settings: &Settings) -> Timesheet {
    // No tachograph report here: the night is always measured on the written times, and the times are never made up on a
    // grid (the card style only takes the segments of the day, which are its trips).
    let rules = Settings { night_source: NightSource::Intervals, interval_mode: IntervalMode::Card, ..settings.clone() };
    let mut employees: Vec<Employee> = sheet.drivers.iter().map(|driver| employee(driver, sheet, &rules)).collect();
    employees.sort_by(|a, b| a.name.cmp(&b.name));
    Timesheet {
        company: String::new(),
        location: String::new(),
        year: sheet.year,
        month: sheet.month,
        working_days: sheet.weekdays,
        norm_hours: sheet.cap_min / 60,
        days: sheet.days.clone(),
        employees,
        warnings: Vec::new(),
        notices: Vec::new(),
        sources: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catb::{api::parse_rows, apply_download};

    /// Mon 5 Oct: Otopeni 11:00 → Constanța 14:00 (3 h); Mon 12 Oct 05:00 → 08:00, wait until 11:00, 11:00 → 14:00.
    const OCTOBER_BODY: &str = r#"{"data":[
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-05 11:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-12 05:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-3"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-12 11:00","duration_minutes":180},
          {"driver_id":2,"driver_name":"Anghel Dan","trip":{"type":"eco","id":"eco-4"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-06 05:00","duration_minutes":200}
        ]}"#;

    fn october() -> Sheet {
        apply_download(None, 2026, 10, &parse_rows(OCTOBER_BODY).unwrap(), "x", &Settings::default())
    }

    fn entry<'a>(timesheet: &'a Timesheet, name: &str, day: u32) -> &'a DayEntry {
        &timesheet.employees.iter().find(|e| e.name == name).unwrap().days[day as usize - 1]
    }

    #[test]
    fn the_days_become_rounded_hours_with_intervals_and_the_drivers_are_sorted() {
        let timesheet = to_timesheet(&october(), &Settings::default());
        assert_eq!(timesheet.employees.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["ANGHEL DAN", "POPESCU ION"]);
        assert_eq!(timesheet.norm_hours, 22 * 8, "22 weekdays in October 2026");

        // One trip of 3 h: an hour is added for it (30 min before the departure, 30 min after the arrival), 4 h.
        assert_eq!(entry(&timesheet, "POPESCU ION", 5).hours, Some(4.0));
        // 05:00–08:00 and 11:00–14:00: 6 h on the trips and 2 h added for the two trips make 8 h. The 3 h of waiting in
        // between are not work time, so they are in neither the hours nor the times.
        let twelfth = entry(&timesheet, "POPESCU ION", 12);
        assert_eq!(twelfth.hours, Some(8.0));
        assert_eq!(twelfth.intervals, [iv(5 * 60, 8 * 60), iv(11 * 60, 14 * 60)]);
        // 3 h 20 min + the hour added for the trip = 4 h 20 min, which is written as 5 (20 minutes count as a full hour).
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 6).hours, Some(5.0));
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 7).hours, None, "a day without trips stays empty");
    }

    #[test]
    fn the_time_added_to_a_day_counts_in_the_hours_not_in_the_times() {
        // Mon 5 Oct 11:00–14:00 is the only trip of the day: an hour is added (half before, half after) to the hours, the
        // times stay those of the trip.
        let timesheet = to_timesheet(&october(), &Settings::default());
        let day = entry(&timesheet, "POPESCU ION", 5);
        assert_eq!(day.hours, Some(4.0));
        assert_eq!((day.first_start, day.last_end), (Some(10 * 60 + 30), Some(14 * 60 + 30)), "the span of the day with the time added");
        assert_eq!(day.intervals, [iv(11 * 60, 14 * 60)], "the trip, from its departure to its arrival");
        // The hours carry a remark that says what was added to the time of the trip, so nobody wonders why 4 h and 3 h of trip.
        let note = day.note.as_deref().expect("a remark beside the hours of a day with time added");
        assert_eq!(note, "Timp adăugat la timpul inițial: din cursă 3:00, adăugat 1:00 (0:30 înainte de prima plecare, 0:30 după ultima sosire), în total 4:00.");
        // Without the setting the day keeps the hours of the trip, and the same times.
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        let plain = to_timesheet(&apply_download(None, 2026, 10, &parse_rows(OCTOBER_BODY).unwrap(), "x", &settings), &settings);
        let day = entry(&plain, "POPESCU ION", 5);
        assert_eq!((day.hours, day.first_start, day.last_end), (Some(3.0), Some(11 * 60), Some(14 * 60)));
        assert_eq!(day.intervals, [iv(11 * 60, 14 * 60)]);
        assert_eq!(day.note, None, "nothing was added, nothing to say");
    }

    fn iv(start: i32, end: i32) -> Interval {
        Interval { start, end }
    }

    /// One driver with the given eco trips (`from`, `to`, departure, minutes), through the whole path of the screen.
    fn driver_sheet(trips: &[(&str, &str, &str, i32)]) -> Sheet {
        let rows: Vec<String> = trips
            .iter()
            .enumerate()
            .map(|(i, (from, to, at, minutes))| {
                format!(r#"{{"driver_id":9,"driver_name":"Test Sofer","trip":{{"type":"eco","id":"eco-{i}"}},"route":{{"from":"{from}","to":"{to}"}},"departure":"{at}","duration_minutes":{minutes}}}"#)
            })
            .collect();
        let body = format!(r#"{{"data":[{}]}}"#, rows.join(","));
        apply_download(None, 2026, 10, &parse_rows(&body).unwrap(), "x", &Settings::default())
    }

    const OTP: &str = "Aeroport Otopeni Bucuresti";

    #[test]
    fn every_trip_is_written_from_its_departure_to_its_arrival() {
        // The user's example: left at 12 and arrived at 15, left again at 16 and arrived at 19. The sheet says 12:00–15:00
        // and 16:00–19:00, never 12:00–19:00.
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-04 12:00", 180), (OTP, "Constanta", "2026-10-04 16:00", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 4);
        assert_eq!(day.intervals, [iv(12 * 60, 15 * 60), iv(16 * 60, 19 * 60)]);
        assert_eq!(day.hours, Some(8.0), "6 h of trips and 2 h added (the hour of waiting between them is not counted)");
    }

    #[test]
    fn a_day_with_the_waiting_between_two_trips_writes_the_two_trips_and_not_the_waiting() {
        // 08:30–11:30, waiting at Otopeni for 3 h 45 min, 15:15–18:15: the waiting is neither in the hours nor in the times.
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-05 08:30", 180), (OTP, "Constanta", "2026-10-05 15:15", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 5);
        assert_eq!(day.hours, Some(8.0), "6 h on the trips and 2 h added; with the 3:45 of waiting it would be 9:45 → 10");
        assert_eq!(day.intervals, [iv(8 * 60 + 30, 11 * 60 + 30), iv(15 * 60 + 15, 18 * 60 + 15)]);
        // The span is that of the trips with the time added on each side (an hour before and an hour after).
        assert_eq!((day.first_start, day.last_end), (Some(7 * 60 + 30), Some(19 * 60 + 15)));
    }

    #[test]
    fn a_day_with_three_trips_writes_three_pairs_of_times() {
        // 05:00–08:00 to Otopeni, 09:00–12:00 back (the hour of waiting does not count), then home, 18:00–21:00 to Otopeni again.
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-06 05:00", 180), (OTP, "Constanta", "2026-10-06 09:00", 180), ("Constanta", OTP, "2026-10-06 18:00", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 6);
        assert_eq!(day.hours, Some(9.0), "9 h on the trips, already more than a full day: nothing is added");
        assert_eq!(day.intervals, [iv(5 * 60, 8 * 60), iv(9 * 60, 12 * 60), iv(18 * 60, 21 * 60)]);
    }

    #[test]
    fn a_trip_after_midnight_is_written_past_24_hours_and_the_night_follows_the_written_times() {
        // 22:30 → 01:30: one interval, the night (00:00–07:00 by default) is the hour and a half after midnight; under the
        // threshold of 3 hours nothing is written for it.
        let sheet = driver_sheet(&[(OTP, "Constanta", "2026-10-07 22:30", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 7);
        assert_eq!(day.intervals, [iv(22 * 60 + 30, 25 * 60 + 30)]);
        assert_eq!((day.hours, day.night_min, day.night_hours), (Some(4.0), 90, None));
        // With no threshold the hour and a half is written as 2 (30 minutes count as an hour).
        let mut settings = Settings::default();
        settings.night_threshold_min = 0;
        let day = entry(&to_timesheet(&sheet, &settings), "TEST SOFER", 7).clone();
        assert_eq!(day.night_hours, Some(2.0));
    }

    #[test]
    fn a_leave_is_its_code_and_a_day_taken_out_stays_empty() {
        let mut sheet = october();
        let popescu = sheet.drivers.iter_mut().find(|d| d.name == "Popescu Ion").unwrap();
        popescu.leaves.insert(NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(), "CC".into());
        popescu.leaves.insert(NaiveDate::from_ymd_opt(2026, 10, 7).unwrap(), "CIM".into());
        popescu.excluded_days.push(NaiveDate::from_ymd_opt(2026, 10, 5).unwrap());
        super::super::recompute(&mut sheet, &Settings::default());

        let timesheet = to_timesheet(&sheet, &Settings::default());
        assert_eq!(entry(&timesheet, "POPESCU ION", 6).code.as_deref(), Some("CC"));
        assert_eq!(entry(&timesheet, "POPESCU ION", 7).code.as_deref(), Some("CIM"));
        let fifth = entry(&timesheet, "POPESCU ION", 5);
        assert!(fifth.hours.is_none() && fifth.intervals.is_empty(), "the day taken out of the pontaj is empty");

        let totals = &timesheet.employees.iter().find(|e| e.name == "POPESCU ION").unwrap().totals;
        assert_eq!(totals.worked, 8.0, "only the 12th is left");
        let columns: Vec<&str> = crate::timesheet::CODES.iter().map(|(code, _)| *code).collect();
        let hours_of = |code: &str| totals.code_hours[columns.iter().position(|c| *c == code).unwrap()];
        assert_eq!((hours_of("CC"), hours_of("CIM")), (8.0, 8.0), "a day of leave is a norm day (8 h) in its column");
    }

    #[test]
    fn the_sheet_is_written_in_the_layout_of_the_payroll_file() {
        let timesheet = to_timesheet(&october(), &Settings::default());
        let dir = std::env::temp_dir().join(format!("pontaj-catb-foaie-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut workbook = rust_xlsxwriter::Workbook::new();
        crate::excel::pontaj_sheet(workbook.add_worksheet(), &timesheet, &Settings::default(), None).unwrap();
        workbook.save(dir.join("foaie.xlsx")).unwrap();
        assert!(std::fs::metadata(dir.join("foaie.xlsx")).unwrap().len() > 2000);
        std::fs::remove_dir_all(&dir).ok();
    }
}
