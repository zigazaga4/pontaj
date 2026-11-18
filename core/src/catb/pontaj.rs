//! The Categoria B month as the payroll team's *foaie colectivă*: the same [`Timesheet`] the tachograph mode builds, so
//! the Excel sheet is drawn by the same code and looks the same (see [`crate::excel::pontaj_sheet`]).
//!
//! What a day holds follows the rules of the other mode: the hours of the day rounded to whole hours, two work
//! intervals, the night hours from those intervals. The minutes and the day they belong to are the ones of the sheet
//! (a trip counts on the day it leaves, a waiting on the day it starts), so the Excel shows what the screen shows.

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use super::model::{DayRow, Driver, Sheet};
use crate::rules::{compute_totals, recompute_day};
use crate::settings::{NightSource, Settings};
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

        // One trip of 3 h: 2 h 30 min are added before the departure and 2 h 30 min after the arrival, 8 h.
        assert_eq!(entry(&timesheet, "POPESCU ION", 5).hours, Some(8.0));
        // 05:00–08:00 and 11:00–14:00 plus the 3 h of waiting in between: 9 h, more than a full day, so nothing is added.
        let twelfth = entry(&timesheet, "POPESCU ION", 12);
        assert_eq!(twelfth.hours, Some(9.0));
        assert_eq!(twelfth.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 9 * 60, "the intervals add up to the hours");
        // 3 h 20 min + 4 h 40 min added = 8 h.
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 6).hours, Some(8.0));
        assert_eq!(entry(&timesheet, "ANGHEL DAN", 7).hours, None, "a day without trips stays empty");
    }

    #[test]
    fn the_time_added_to_a_day_moves_the_written_start_and_end() {
        // Mon 5 Oct 11:00–14:00 is the only trip of the day: 2 h 30 min before and 2 h 30 min after.
        let timesheet = to_timesheet(&october(), &Settings::default());
        let day = entry(&timesheet, "POPESCU ION", 5);
        assert_eq!((day.first_start, day.last_end), (Some(8 * 60 + 30), Some(16 * 60 + 30)), "08:30–16:30 instead of 11:00–14:00");
        // The day starts at 08:30 and its two intervals add up to the 8 hours (with no room for a break inside the day, the
        // second interval starts after one, like in the tachograph mode).
        assert_eq!(day.intervals.first().unwrap().start, 8 * 60 + 30);
        assert_eq!(day.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 8 * 60);
        // Without the setting the day keeps the hours of the trip.
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        let plain = to_timesheet(&apply_download(None, 2026, 10, &parse_rows(OCTOBER_BODY).unwrap(), "x", &settings), &settings);
        let day = entry(&plain, "POPESCU ION", 5);
        assert_eq!((day.hours, day.first_start, day.last_end), (Some(3.0), Some(11 * 60), Some(14 * 60)));
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
