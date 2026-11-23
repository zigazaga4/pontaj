//! The Categoria B month as the payroll team's *foaie colectivă*: the same [`Timesheet`] the tachograph mode builds, so
//! the Excel sheet is drawn by the same code and looks the same (see [`crate::excel::pontaj_sheet`]).
//!
//! What a day holds: the hours of the day (time on the trips and time added, never the waiting between trips) rounded to
//! whole hours (the rule of the other mode); under them the start and end of every trip of the day, as the screen lists them
//! (12:00–15:00 and 16:00–19:00, never one 12:00–19:00), with as many pairs of rows as the driver's busiest day needs; the
//! night hours measured on those times. The time added to a day is already in the times (the first departure earlier, the
//! last arrival later) and a remark on those two cells says what the time was before. The minutes and the day they belong
//! to are the ones of the sheet (a trip counts on its work day), so the Excel shows what the screen shows. See
//! [`counted_intervals`].

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use super::model::{DayRow, Driver, Sheet};
use crate::rules::{compute_totals, recompute_day};
use crate::settings::{IntervalMode, NightSource, Settings};
use crate::time::{fmt_clock, fmt_duration, round_hours};
use crate::timesheet::{display_name, DayEntry, Employee, Group, Interval, TimeNote, Timesheet, Totals};

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
/// [`super::model::Effective::work_day`]), with its place in `driver.trips`.
fn counted_trips(driver: &Driver) -> impl Iterator<Item = (usize, Placed)> + '_ {
    driver
        .trips
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.edit.removed && !t.eff.excursion && t.eff.counts_hours && t.eff.minutes > 0)
        .filter_map(|(i, t)| t.eff.departure.map(|at| (i, placed(t.eff.work_day.unwrap_or(at.date()), at, t.eff.minutes))))
}

/// The counted trips of a day in the order they leave, each with its place in `driver.trips` and its times.
fn day_trips(driver: &Driver, date: NaiveDate) -> Vec<(usize, Interval)> {
    let mut out: Vec<(usize, Interval)> = counted_trips(driver).filter(|(_, p)| p.day == date).map(|(i, p)| (i, p.interval())).collect();
    out.sort_by_key(|(_, iv)| (iv.start, iv.end));
    out
}

/// Which of the (sorted) periods of a day open and close it: the first to leave and the one that ends last. The time added to
/// the day goes before the departure of the first and after the arrival of the second (the same trip on a day with one trip).
fn edges(periods: &[Interval]) -> Option<(usize, usize)> {
    let last = periods.iter().enumerate().max_by_key(|(_, iv)| iv.end)?.0;
    Some((0, last))
}

/// The trips of a day as they really are: each one from its departure to its arrival, in the order they leave (an arrival
/// after midnight is past 24:00), exactly as the screen lists them. Without the time added to fill the day and without the
/// waiting between two trips (see [`counted_intervals`] for what is written in the sheet).
pub(super) fn trip_intervals(driver: &Driver, date: NaiveDate) -> Vec<Interval> {
    day_trips(driver, date).into_iter().map(|(_, iv)| iv).collect()
}

/// First start and last end (minutes from midnight) of the trips of a day, without the time added to fill it. The waiting
/// between trips is not work time, so it never widens the span.
pub(super) fn day_span(driver: &Driver, date: NaiveDate) -> Option<(i32, i32)> {
    let trips = trip_intervals(driver, date);
    Some((trips.iter().map(|b| b.start).min()?, trips.iter().map(|b| b.end).max()?))
}

/// The trips of a day as the payroll sheet writes them: the real times, except that the time added to the day is already in
/// them: the first departure is earlier by what was added before it, the last arrival later by what was added after it. A day
/// with more trips than the usual two gets more rows in the sheet (see `crate::excel::pontaj_sheet`).
pub(super) fn counted_intervals(driver: &Driver, date: NaiveDate, row: &DayRow) -> Vec<Interval> {
    let mut out = trip_intervals(driver, date);
    if let Some((first, last)) = edges(&out) {
        out[first].start -= row.fill_before_min;
        out[last].end += row.fill_after_min;
    }
    out
}

/// The remarks written on the times that got time added, next to them: what was added, what the time was and what it is now
/// ("S-a adăugat 0:30 la plecare: inițial era 12:00, acum e 11:30."). Empty when nothing was added.
fn time_notes(driver: &Driver, date: NaiveDate, row: &DayRow) -> Vec<TimeNote> {
    let real = trip_intervals(driver, date);
    let Some((first, last)) = edges(&real) else { return Vec::new() };
    let by_office = if row.fill_manual { " (scris de birou)" } else { "" };
    let mut notes = Vec::new();
    if row.fill_before_min > 0 {
        let text = format!(
            "S-a adăugat {} la plecare{by_office}: inițial era {}, acum e {}.",
            fmt_duration(row.fill_before_min),
            fmt_clock(real[first].start),
            fmt_clock(real[first].start - row.fill_before_min)
        );
        notes.push(TimeNote { interval: first, end: false, text });
    }
    if row.fill_after_min > 0 {
        let text = format!(
            "S-a adăugat {} la sosire{by_office}: inițial era {}, acum e {}.",
            fmt_duration(row.fill_after_min),
            fmt_clock(real[last].end),
            fmt_clock(real[last].end + row.fill_after_min)
        );
        notes.push(TimeNote { interval: last, end: true, text });
    }
    notes
}

/// Marks on the trips of the driver where the time added to each day sits: the trip that opens the day and the one that
/// closes it, and how many minutes were added before the first and after the last (a day taken out of the pontaj marks
/// nothing: its times are not written; neither does a day whose hours were fixed by hand: no time is added to it). The screen
/// then shows these trips with the time that counts.
pub(super) fn mark_day_edges(driver: &mut Driver) {
    for trip in &mut driver.trips {
        trip.eff.opens_day = false;
        trip.eff.closes_day = false;
        trip.eff.added_before_min = 0;
        trip.eff.added_after_min = 0;
    }
    let marks: Vec<(NaiveDate, i32, i32)> = driver
        .days
        .iter()
        .filter(|r| !r.excluded && r.leave.is_none() && !r.adjusted)
        .filter_map(|r| Some((r.date?, r.fill_before_min, r.fill_after_min)))
        .collect();
    for (date, before, after) in marks {
        let trips = day_trips(driver, date);
        let periods: Vec<Interval> = trips.iter().map(|(_, iv)| iv.clone()).collect();
        let Some((first, last)) = edges(&periods) else { continue };
        let (opens, closes) = (trips[first].0, trips[last].0);
        driver.trips[opens].eff.opens_day = true;
        driver.trips[opens].eff.added_before_min = before;
        driver.trips[closes].eff.closes_day = true;
        driver.trips[closes].eff.added_after_min = after;
    }
}

/// One day of one driver. A day taken out of the pontaj stays empty; a leave is its code. The hours are the day's total
/// (trips and time added, no waiting) rounded; the start/end times are those of its trips with the time added already in
/// them ([`counted_intervals`]), and the times that changed carry a remark.
fn day_entry(driver: &Driver, day: u32, date: NaiveDate, settings: &Settings) -> DayEntry {
    let mut entry = DayEntry { day, ..Default::default() };
    let row: Option<&DayRow> = driver.days.iter().find(|r| r.date == Some(date));
    if let Some(row) = row.filter(|r| !r.excluded) {
        if let Some(code) = &row.leave {
            entry.code = Some(code.clone());
        } else if row.total_min > 0 {
            entry.worked_min = row.total_min;
            // A day fixed by hand has exactly the hours typed for it (whole or half hours), not the rounding of its minutes.
            if row.adjusted {
                entry.override_hours = Some(f64::from(row.total_min) / 60.0);
            }
            let trips = counted_intervals(driver, date, row);
            entry.first_start = trips.iter().map(|b| b.start).min();
            entry.last_end = trips.iter().map(|b| b.end).max();
            // A day that rounds to no hour at all writes nothing (the rules would otherwise count the intervals as typed hours).
            if round_hours(row.total_min, settings.round_up_from_min) > 0 {
                entry.override_intervals = Some(trips.clone()).filter(|w| !w.is_empty());
                entry.time_notes = time_notes(driver, date, row);
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
        group: Group::CatB,
        norm_hours: Some(sheet.cap_min / 60),
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
        assert_eq!(twelfth.intervals, [iv(4 * 60, 8 * 60), iv(11 * 60, 15 * 60)], "the hour added is in the first start and the last end");
        // 3 h 20 min + the hour added for the trip = 4 h 20 min, which is written as 5 (20 minutes count as a full hour).
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 6).hours, Some(5.0));
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 7).hours, None, "a day without trips stays empty");
    }

    #[test]
    fn the_time_added_to_a_day_is_in_its_times_with_a_remark_on_each_changed_time() {
        // Mon 5 Oct 11:00–14:00 is the only trip of the day: an hour is added (half before, half after). The departure is
        // earlier and the arrival later, so the times add up to the hours of the day.
        let timesheet = to_timesheet(&october(), &Settings::default());
        let day = entry(&timesheet, "POPESCU ION", 5);
        assert_eq!(day.hours, Some(4.0));
        assert_eq!((day.first_start, day.last_end), (Some(10 * 60 + 30), Some(14 * 60 + 30)));
        assert_eq!(day.intervals, [iv(10 * 60 + 30, 14 * 60 + 30)], "the trip with the time added: 10:30–14:30, 4 h");
        // A remark sits on each time that changed and says what it was and what it is now.
        let texts: Vec<(usize, bool, &str)> = day.time_notes.iter().map(|n| (n.interval, n.end, n.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                (0, false, "S-a adăugat 0:30 la plecare: inițial era 11:00, acum e 10:30."),
                (0, true, "S-a adăugat 0:30 la sosire: inițial era 14:00, acum e 14:30."),
            ]
        );
        // Without the setting the day keeps the hours and the times of the trip.
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        let plain = to_timesheet(&apply_download(None, 2026, 10, &parse_rows(OCTOBER_BODY).unwrap(), "x", &settings), &settings);
        let day = entry(&plain, "POPESCU ION", 5);
        assert_eq!((day.hours, day.first_start, day.last_end), (Some(3.0), Some(11 * 60), Some(14 * 60)));
        assert_eq!(day.intervals, [iv(11 * 60, 14 * 60)]);
        assert!(day.time_notes.is_empty(), "nothing was added, nothing to say");
    }

    #[test]
    fn two_trips_get_the_hour_each_on_the_first_departure_and_the_last_arrival() {
        // 05:00–08:00 and 11:00–14:00: two hours are added, one before the first departure and one after the last arrival.
        let mut sheet = october();
        let driver = sheet.drivers.iter_mut().find(|d| d.name.starts_with("Popescu")).unwrap();
        let day = driver.days.iter().find(|r| r.date == NaiveDate::from_ymd_opt(2026, 10, 12)).unwrap();
        assert_eq!((day.fill_before_min, day.fill_after_min, day.total_min), (60, 60, 480));
        let marked: Vec<(&str, bool, bool, i32, i32)> = driver
            .trips
            .iter()
            .filter(|t| t.eff.work_day == NaiveDate::from_ymd_opt(2026, 10, 12))
            .map(|t| (t.id.as_str(), t.eff.opens_day, t.eff.closes_day, t.eff.added_before_min, t.eff.added_after_min))
            .collect();
        assert_eq!(marked, [("eco-2", true, false, 60, 0), ("eco-3", false, true, 0, 60)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let twelfth = entry(&timesheet, "POPESCU ION", 12);
        assert_eq!(twelfth.intervals, [iv(4 * 60, 8 * 60), iv(11 * 60, 15 * 60)], "04:00–08:00 and 11:00–15:00: 8 h");
        let at: Vec<(usize, bool)> = twelfth.time_notes.iter().map(|n| (n.interval, n.end)).collect();
        assert_eq!(at, [(0, false), (1, true)], "a remark on the first start and on the last end only");
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
        // Left at 12 and arrived at 15, left again at 16 and arrived at 19: two periods, never one 12:00–19:00. The two hours
        // added are in the first departure (an hour earlier) and in the last arrival (an hour later).
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-04 12:00", 180), (OTP, "Constanta", "2026-10-04 16:00", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 4);
        assert_eq!(day.intervals, [iv(11 * 60, 15 * 60), iv(16 * 60, 20 * 60)]);
        assert_eq!(day.hours, Some(8.0), "6 h of trips and 2 h added (the hour of waiting between them is not counted)");
    }

    #[test]
    fn a_day_with_the_waiting_between_two_trips_writes_the_two_trips_and_not_the_waiting() {
        // 08:30–11:30, waiting at Otopeni for 3 h 45 min, 15:15–18:15: the waiting is neither in the hours nor in the times.
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-05 08:30", 180), (OTP, "Constanta", "2026-10-05 15:15", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 5);
        assert_eq!(day.hours, Some(8.0), "6 h on the trips and 2 h added; with the 3:45 of waiting it would be 9:45 → 10");
        assert_eq!(day.intervals, [iv(7 * 60 + 30, 11 * 60 + 30), iv(15 * 60 + 15, 19 * 60 + 15)], "an hour added before the first departure and after the last arrival");
        // The span is that of the times written.
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
        // 22:30 → 01:30 with half an hour added on each side: 22:00 → 02:00, one interval. The night (00:00–07:00 by default)
        // is the two hours after midnight, the time added after the arrival included: written as 2 (no minimum by default).
        let sheet = driver_sheet(&[(OTP, "Constanta", "2026-10-07 22:30", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 7);
        assert_eq!(day.intervals, [iv(22 * 60, 26 * 60)]);
        assert_eq!((day.hours, day.night_min, day.night_hours), (Some(4.0), 120, Some(2.0)));
        // A minimum of 3 hours chosen by the office leaves them out.
        let settings = Settings { night_threshold_min: 180, ..Settings::default() };
        let day = entry(&to_timesheet(&sheet, &settings), "TEST SOFER", 7).clone();
        assert_eq!((day.night_min, day.night_hours), (120, None));
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
