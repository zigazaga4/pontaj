//! The Categoria B month as the payroll team's *foaie colectivă*: the same [`Timesheet`] the tachograph mode builds, so
//! the Excel sheet is drawn by the same code and looks the same (see [`crate::excel::pontaj_sheet`]).
//!
//! What a day holds: the hours of the day rounded to whole hours (the rule of the other mode), up to two work
//! intervals made of the REAL periods of the day (trips, waiting, added time) and the night hours measured on them. The
//! minutes and the day they belong to are the ones of the sheet (a trip counts on the day it leaves, a waiting on the
//! day it starts), so the Excel shows what the screen shows. Unlike the tachograph mode, the times are never moved to a
//! grid: see [`written_intervals`].

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use super::model::{DayRow, Driver, Sheet};
use crate::rules::{card_intervals, compute_totals, recompute_day};
use crate::settings::{NightSource, Settings};
use crate::time::round_hours;
use crate::timesheet::{display_name, DayEntry, Employee, Interval, Timesheet, Totals};

fn minute_of_day(at: NaiveDateTime) -> i32 {
    (at.hour() * 60 + at.minute()) as i32
}

/// The periods of a day that count as work: the trips that leave that day and the waiting that starts on it.
pub(super) fn work_blocks(driver: &Driver, date: NaiveDate) -> Vec<Interval> {
    let trips = driver
        .trips
        .iter()
        .filter(|t| !t.edit.removed && !t.eff.excursion && t.eff.counts_hours && t.eff.minutes > 0)
        .filter_map(|t| t.eff.departure.map(|at| (at, t.eff.minutes)));
    let waits = driver.pauses.iter().filter(|p| p.counted_min > 0).filter_map(|p| p.start.map(|at| (at, p.counted_min)));
    let mut blocks: Vec<Interval> = trips
        .chain(waits)
        .filter(|(at, _)| at.date() == date)
        .map(|(at, minutes)| Interval { start: minute_of_day(at), end: minute_of_day(at) + minutes })
        .collect();
    blocks.sort_by_key(|b| (b.start, b.end));
    blocks
}

/// First start and last end (minutes from midnight) of the periods that count on a day, without the time added to fill it.
pub(super) fn day_span(driver: &Driver, date: NaiveDate) -> Option<(i32, i32)> {
    let blocks = work_blocks(driver, date);
    Some((blocks.iter().map(|b| b.start).min()?, blocks.iter().map(|b| b.end).max()?))
}

/// The day's work as the two start/end pairs of the payroll sheet, with the times the screen shows.
///
/// The periods that count (trips, the waiting that counts, the time added) are joined where they touch, so a day of two
/// trips and the waiting between them is one interval, and a stay at home between two trips splits the day in two. The
/// sheet has two slots: more than two periods merge the closest ones. The hours of the day are whole, so the end of the
/// last interval moves by the rounding (`hours_min` − `total_min`, under an hour either way) and the intervals add up to
/// the hours written. Minutes that count but that the periods do not cover (a waiting typed longer than the gap between
/// the trips, trips that overlap) are added at the end too, so the hours typed on the screen are the hours written. Nothing
/// else is bent or invented: no break is added and no time is moved to a grid, as the tachograph mode does. Two periods
/// merged into one slot include the gap between them, and then the intervals add up to more than the hours: the times
/// stay the real ones.
pub(super) fn written_intervals(blocks: &[Interval], total_min: i32, hours_min: i32) -> Vec<Interval> {
    let mut intervals = card_intervals(blocks, 0, 2);
    let covered: i32 = intervals.iter().map(|i| i.end - i.start).sum();
    let mut delta = hours_min - total_min + (total_min - covered).max(0);
    while delta != 0 {
        let Some(last) = intervals.last_mut() else { break };
        let length = last.end - last.start;
        if delta > 0 || length + delta > 0 {
            last.end += delta;
            break;
        }
        // The last period is shorter than the cut: it disappears and the rest of the cut falls on the one before it.
        delta += length;
        intervals.pop();
    }
    intervals
}

/// One day of one driver. A day taken out of the pontaj stays empty; a leave is its code. The time added to fill the day
/// goes before the first start and after the last end, so the written intervals show it.
fn day_entry(driver: &Driver, day: u32, date: NaiveDate, settings: &Settings) -> DayEntry {
    let mut entry = DayEntry { day, ..Default::default() };
    let row: Option<&DayRow> = driver.days.iter().find(|r| r.date == Some(date));
    if let Some(row) = row.filter(|r| !r.excluded) {
        if let Some(code) = &row.leave {
            entry.code = Some(code.clone());
        } else if row.total_min > 0 {
            let mut blocks = work_blocks(driver, date);
            entry.worked_min = row.total_min;
            let first = blocks.iter().map(|b| b.start).min();
            let last = blocks.iter().map(|b| b.end).max();
            if let Some(start) = first.filter(|_| row.fill_before_min > 0) {
                blocks.push(Interval { start: start - row.fill_before_min, end: start });
            }
            if let Some(end) = last.filter(|_| row.fill_after_min > 0) {
                blocks.push(Interval { start: end, end: end + row.fill_after_min });
            }
            blocks.sort_by_key(|b| (b.start, b.end));
            entry.first_start = first.map(|start| start - row.fill_before_min);
            entry.last_end = last.map(|end| end + row.fill_after_min);
            // A day that rounds to no hour at all writes nothing (the rules would otherwise count the intervals as typed hours).
            let hours_min = round_hours(row.total_min, settings.round_up_from_min) * 60;
            if hours_min > 0 {
                let written = written_intervals(&blocks, row.total_min, hours_min);
                entry.override_intervals = Some(written).filter(|w| !w.is_empty());
            }
            entry.segments = blocks;
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
    // No tachograph report here: the night is always measured on the written intervals.
    let rules = Settings { night_source: NightSource::Intervals, ..settings.clone() };
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
        // 05:00–08:00 and 11:00–14:00 plus the 3 h of waiting in between: 9 h, more than a full day, so nothing is added.
        let twelfth = entry(&timesheet, "POPESCU ION", 12);
        assert_eq!(twelfth.hours, Some(9.0));
        assert_eq!(twelfth.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 9 * 60, "the intervals add up to the hours");
        // 3 h 20 min + the hour added for the trip = 4 h 20 min, which is written as 5 (20 minutes count as a full hour).
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 6).hours, Some(5.0));
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 7).hours, None, "a day without trips stays empty");
    }

    #[test]
    fn the_time_added_to_a_day_moves_the_written_start_and_end() {
        // Mon 5 Oct 11:00–14:00 is the only trip of the day: half an hour before and half an hour after.
        let timesheet = to_timesheet(&october(), &Settings::default());
        let day = entry(&timesheet, "POPESCU ION", 5);
        assert_eq!((day.first_start, day.last_end), (Some(10 * 60 + 30), Some(14 * 60 + 30)), "10:30–14:30 instead of 11:00–14:00");
        // The day starts at 10:30 and its two intervals add up to the 4 hours.
        assert_eq!(day.intervals.first().unwrap().start, 10 * 60 + 30);
        assert_eq!(day.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 4 * 60);
        // Without the setting the day keeps the hours of the trip.
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        let plain = to_timesheet(&apply_download(None, 2026, 10, &parse_rows(OCTOBER_BODY).unwrap(), "x", &settings), &settings);
        let day = entry(&plain, "POPESCU ION", 5);
        assert_eq!((day.hours, day.first_start, day.last_end), (Some(3.0), Some(11 * 60), Some(14 * 60)));
    }

    fn iv(start: i32, end: i32) -> Interval {
        Interval { start, end }
    }

    #[test]
    fn the_written_intervals_are_the_real_periods_and_add_up_to_the_hours() {
        let h = |h: i32, m: i32| h * 60 + m;
        // Trips 08:30–11:30 and 15:15–18:15 with the waiting between them: one period of 9:45, written as 10 hours (+15).
        let day = [iv(h(8, 30), h(11, 30)), iv(h(15, 15), h(18, 15)), iv(h(11, 30), h(15, 15))];
        assert_eq!(written_intervals(&day, 9 * 60 + 45, 10 * 60), [iv(h(8, 30), h(18, 30))], "only the rounding moves the end (15 minutes)");
        // 8:10 of work is written as 8 hours: the end moves back 10 minutes.
        assert_eq!(written_intervals(&[iv(h(5, 0), h(13, 10))], 8 * 60 + 10, 8 * 60), [iv(h(5, 0), h(13, 0))]);
        // A stay at home between two periods keeps them apart.
        let split = [iv(h(5, 0), h(8, 0)), iv(h(15, 0), h(18, 0))];
        assert_eq!(written_intervals(&split, 6 * 60, 6 * 60), split);
        // Three periods do not fit in two slots: the two closest merge, so the hour between them is inside the first
        // interval. The times stay the real ones (the intervals add up to 10 hours, the day is 9): nothing is bent.
        let three = [iv(h(5, 0), h(8, 0)), iv(h(9, 0), h(12, 0)), iv(h(18, 0), h(21, 0))];
        assert_eq!(written_intervals(&three, 9 * 60, 9 * 60), [iv(h(5, 0), h(12, 0)), iv(h(18, 0), h(21, 0))]);
        // Minutes that count but are not on the clock (a waiting typed longer than the gap, overlapping trips) are added at
        // the end, so the hours of the screen are the hours written: 13:00–13:15 gap, 2:30 typed, trips 10:00–13:00 and 13:15–16:15.
        let typed = [iv(h(10, 0), h(13, 0)), iv(h(13, 15), h(16, 15)), iv(h(13, 0), h(15, 30))];
        assert_eq!(written_intervals(&typed, 8 * 60 + 30, 9 * 60), [iv(h(10, 0), h(19, 0))]);
        // Nothing to write without periods.
        assert!(written_intervals(&[], 30, 60).is_empty());
        // A cut longer than the last period removes it and falls on the one before.
        assert_eq!(written_intervals(&[iv(h(5, 0), h(10, 0)), iv(h(20, 0), h(20, 20))], 320, 290), [iv(h(5, 0), h(9, 50))]);
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
    fn a_day_with_the_waiting_between_two_trips_is_written_from_its_real_start_to_its_real_end() {
        // The case the Excel got wrong: 08:30–11:30, waiting at Otopeni, 15:15–18:15 (the app says the day ends at 18:15).
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-05 08:30", 180), (OTP, "Constanta", "2026-10-05 15:15", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 5);
        assert_eq!(day.hours, Some(10.0), "9:45 of work, 20 minutes or more count as an hour");
        assert_eq!(day.intervals, [iv(8 * 60 + 30, 18 * 60 + 30)], "not 08:30–13:30 and 14:30–19:30");
        assert_eq!((day.first_start, day.last_end), (Some(8 * 60 + 30), Some(18 * 60 + 15)));
    }

    #[test]
    fn days_with_three_trips_and_a_stay_at_home_in_between_keep_both_periods() {
        // 05:00–08:00 to Otopeni, 09:00–12:00 back (an hour of waiting counts), then home, 18:00–21:00 to Otopeni again.
        let sheet = driver_sheet(&[("Constanta", OTP, "2026-10-06 05:00", 180), (OTP, "Constanta", "2026-10-06 09:00", 180), ("Constanta", OTP, "2026-10-06 18:00", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 6);
        assert_eq!(day.hours, Some(10.0));
        assert_eq!(day.intervals, [iv(5 * 60, 12 * 60), iv(18 * 60, 21 * 60)]);
        assert_eq!(day.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 600);
    }

    #[test]
    fn a_trip_after_midnight_is_written_past_24_hours_and_the_night_follows_the_written_times() {
        // 22:30 → 01:30: one interval, the night (00:00–07:00 by default) is the hour and a half after midnight; under the
        // threshold of 3 hours nothing is written for it.
        let sheet = driver_sheet(&[(OTP, "Constanta", "2026-10-07 22:30", 180)]);
        let timesheet = to_timesheet(&sheet, &Settings::default());
        let day = entry(&timesheet, "TEST SOFER", 7);
        assert_eq!(day.intervals.first().map(|i| i.start), Some(22 * 60 + 30 - 30), "half an hour added before the departure");
        assert!(day.intervals.iter().all(|i| i.end > i.start));
        assert_eq!(day.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), day.hours.map_or(0, |h| (h * 60.0) as i32));
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
        assert_eq!(totals.worked, 9.0, "only the 12th is left");
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
