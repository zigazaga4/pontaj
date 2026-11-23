//! "Reglare după Excel": the hours of an Excel file that was changed outside the app are brought back into the app.
//!
//! The file is the one the app wrote (see [`crate::pontaj_file`]); for each Categoria B driver found on its first sheet, each
//! day is compared with what the app shows for it, and the app follows the file: a day the file has other hours on gets them
//! ([`Driver::set_hours`]), a day the file left empty is taken out of the pontaj, a code of leave on a day without trips
//! becomes that leave. Everything is worked out on a copy: the caller shows the changes and keeps the copy only when the office
//! agrees. The file is read as it is, so a second reglare with the same file changes nothing.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::compute::recompute_driver;
use super::model::*;
use super::pontaj::to_timesheet;
use crate::pontaj_file::{Cell, File, Person};
use crate::settings::Settings;
use crate::time::{round_hours, round_to_half};
use crate::timesheet::{display_name, DayEntry};
use crate::{Error, Result};

/// The most hours a day can have.
const MAX_HOURS: f64 = 24.0;

/// What a day shows: nothing, hours, a code of leave, or something the app cannot read.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Empty,
    Hours(f64),
    Code(String),
    Other(String),
}

impl Shown {
    /// The day of the timesheet as the file shows it.
    fn of_entry(entry: &DayEntry) -> Self {
        match (&entry.code, entry.hours) {
            (Some(code), _) => Self::Code(code.clone()),
            (None, Some(hours)) if hours > 0.0 => Self::Hours(hours),
            _ => Self::Empty,
        }
    }

    /// A cell of the file, with hours to the half hour (the app's own precision for hours typed by hand).
    fn of_cell(cell: &Cell) -> Self {
        match cell {
            Cell::Empty => Self::Empty,
            Cell::Hours(hours) if round_to_half(*hours) <= 0.0 => Self::Empty,
            Cell::Hours(hours) => Self::Hours(round_to_half(*hours)),
            Cell::Code(code) => Self::Code(code.clone()),
            Cell::Other(text) => Self::Other(text.clone()),
        }
    }

    /// For the screen: "8", "7,5", "CO"; empty for a day without hours.
    fn text(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Hours(hours) if hours.fract() == 0.0 => format!("{hours:.0}"),
            Self::Hours(hours) => format!("{hours}").replace('.', ","),
            Self::Code(text) | Self::Other(text) => text.clone(),
        }
    }
}

/// A day whose hours differ between the app and the file, and what the app shows after the reglare.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Change {
    pub driver: String,
    pub date: Option<NaiveDate>,
    /// What the app showed before ("" = nothing) and what it shows now.
    pub app: String,
    pub excel: String,
}

/// A day of the file the app could not follow, with the reason.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Skipped {
    pub driver: String,
    pub date: Option<NaiveDate>,
    pub excel: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Report {
    pub year: i32,
    pub month: u32,
    /// Drivers of the app found in the file.
    pub matched: usize,
    /// Drivers of the app that are not in the file (they stay as they are).
    pub not_in_file: Vec<String>,
    /// People of the file that are not drivers of Categoria B (tachograph, office...): not touched.
    pub others: usize,
    pub changes: Vec<Change>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Reconciled {
    /// The month with the file's hours in it, already recomputed.
    pub sheet: Sheet,
    pub report: Report,
}

/// The name as both sides write it: capitals, no commas, single spaces, no diacritics.
fn key(name: &str) -> String {
    display_name(name, true)
}

/// Brings one driver to what `person` says. Returns the days that could not follow, with the reason.
fn follow(driver: &mut Driver, person: &Person, year: i32, month: u32, settings: &Settings) -> BTreeMap<NaiveDate, String> {
    let mut reasons = BTreeMap::new();
    // What an earlier reglare fixed is worked out again from the file: the file is the truth for every day it has.
    driver.set_hours.clear();
    let mut hours_days: Vec<(NaiveDate, f64)> = Vec::new();
    let mut empty_days: Vec<NaiveDate> = Vec::new();

    for (index, cell) in person.days.iter().enumerate() {
        let Some(date) = NaiveDate::from_ymd_opt(year, month, index as u32 + 1) else { continue };
        match Shown::of_cell(cell) {
            Shown::Code(code) => {
                if !LEAVE_TYPES.iter().any(|(known, _)| *known == code) {
                    reasons.insert(date, format!("codul {code} nu există la Categoria B (sunt: CO, CM, CP, CFP, CC, CIM)"));
                } else if driver.free_days.contains(&date) {
                    driver.leaves.insert(date, code);
                    driver.excluded_days.retain(|day| *day != date);
                } else {
                    reasons.insert(date, "ziua are curse în aplicație, deci nu poate fi concediu".into());
                }
            }
            Shown::Hours(hours) if hours > MAX_HOURS => {
                reasons.insert(date, format!("{} ore într-o zi sunt prea multe", Shown::Hours(hours).text()));
            }
            Shown::Hours(hours) => {
                driver.leaves.remove(&date);
                driver.excluded_days.retain(|day| *day != date);
                hours_days.push((date, hours));
            }
            Shown::Empty => {
                driver.leaves.remove(&date);
                empty_days.push(date);
            }
            Shown::Other(text) => {
                reasons.insert(date, format!("„{text}” nu este nici număr de ore, nici cod de concediu"));
            }
        }
    }

    // What the rules make of each day now, with no hours fixed: only what differs from the file is fixed.
    recompute_driver(driver, year, month, settings);
    let minutes_of = |driver: &Driver, date: NaiveDate| driver.days.iter().find(|day| day.date == Some(date)).map_or(0, |day| day.total_min);
    for (date, hours) in hours_days {
        let shown = f64::from(round_hours(minutes_of(driver, date), settings.round_up_from_min));
        if shown != hours {
            driver.set_hours.insert(date, (hours * 60.0).round() as i32);
        }
    }
    for date in empty_days {
        if round_hours(minutes_of(driver, date), settings.round_up_from_min) > 0 && !driver.excluded_days.contains(&date) {
            driver.excluded_days.push(date);
        }
    }
    recompute_driver(driver, year, month, settings);
    reasons
}

/// Brings the drivers of `sheet` to the hours of the file. The month of the file must be the month of the sheet.
pub fn reconcile(sheet: &Sheet, file: &File, settings: &Settings) -> Result<Reconciled> {
    let (year, month) = (sheet.year, sheet.month);
    if (file.year, file.month) != (year, month) {
        return Err(Error::new(format!(
            "Fișierul este pe luna {:02}.{}, iar în aplicație este deschisă luna {month:02}.{year}. Deschide în aplicație luna din fișier și încearcă din nou.",
            file.month, file.year
        )));
    }
    let before = to_timesheet(sheet, settings);
    let mut out = sheet.clone();
    let mut report = Report { year, month, ..Report::default() };
    let mut reasons: BTreeMap<String, BTreeMap<NaiveDate, String>> = BTreeMap::new();
    let mut found: BTreeSet<usize> = BTreeSet::new();
    let mut matched: Vec<(String, usize)> = Vec::new();

    for driver in &mut out.drivers {
        let Some((at, person)) = file.people.iter().enumerate().find(|(at, person)| !found.contains(at) && key(&person.name) == key(&driver.name)) else {
            report.not_in_file.push(driver.name.clone());
            continue;
        };
        found.insert(at);
        matched.push((driver.name.clone(), at));
        reasons.insert(driver.name.clone(), follow(driver, person, year, month, settings));
    }
    if matched.is_empty() {
        return Err(Error::new("Niciun șofer din Categoria B nu se găsește în fișier. Este fișierul lunii corecte, exportat din aplicație?"));
    }
    report.matched = matched.len();
    report.others = file.people.len() - found.len();

    // What is left to tell: the days that changed, and the days the file asked for that the app could not follow.
    let after = to_timesheet(&out, settings);
    let entries = |timesheet: &crate::Timesheet, name: &str| timesheet.employees.iter().find(|employee| employee.name == display_name(name, settings.plain_names)).map(|employee| employee.days.clone());
    for (name, at) in &matched {
        let (Some(was), Some(now)) = (entries(&before, name), entries(&after, name)) else { continue };
        for (index, ((was, now), cell)) in was.iter().zip(&now).zip(&file.people[*at].days).enumerate() {
            let date = NaiveDate::from_ymd_opt(year, month, index as u32 + 1);
            let (was, now, asked) = (Shown::of_entry(was), Shown::of_entry(now), Shown::of_cell(cell));
            if was != now {
                report.changes.push(Change { driver: name.clone(), date, app: was.text(), excel: now.text() });
            }
            if now != asked {
                let reason = date.and_then(|date| reasons.get(name).and_then(|days| days.get(&date)).cloned()).unwrap_or_else(|| "nu s-a putut aduce ziua la valoarea din fișier".into());
                report.skipped.push(Skipped { driver: name.clone(), date, excel: asked.text(), reason });
            }
        }
    }
    Ok(Reconciled { sheet: out, report })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catb::{api::parse_rows, apply_download};
    use crate::timesheet::Timesheet;

    /// Mon 5 Oct: Popescu 11:00–14:00 (a day of 4 h); Mon 12 Oct 05:00–08:00, 11:00–14:00 (6 h on the trips, 8 h with the time added); Anghel Tue 6 Oct 05:00–08:20 (5 h).
    const BODY: &str = r#"{"data":[
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-05 11:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-12 05:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-3"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-12 11:00","duration_minutes":180},
          {"driver_id":2,"driver_name":"Anghel Dan","trip":{"type":"eco","id":"eco-4"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-06 05:00","duration_minutes":200}
        ]}"#;

    fn october() -> Sheet {
        apply_download(None, 2026, 10, &parse_rows(BODY).unwrap(), "x", &Settings::default())
    }

    /// The file the app would have written, written to disk and read back, with `change` applied to its people first (what somebody
    /// does in Excel: types other hours, a code, clears a cell).
    fn file_after(sheet: &Sheet, name: &str, change: impl FnOnce(&mut Timesheet)) -> File {
        let settings = Settings::default();
        let mut timesheet = to_timesheet(sheet, &settings);
        change(&mut timesheet);
        let path = std::env::temp_dir().join(format!("pontaj-reglare-{name}-{}.xlsx", std::process::id()));
        let mut workbook = rust_xlsxwriter::Workbook::new();
        crate::excel::pontaj_sheet(workbook.add_worksheet(), &timesheet, &settings, None).unwrap();
        workbook.save(&path).unwrap();
        let file = crate::pontaj_file::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        file
    }

    fn day<'a>(timesheet: &'a Timesheet, name: &str, day: usize) -> &'a DayEntry {
        &timesheet.employees.iter().find(|e| e.name == name).unwrap().days[day - 1]
    }

    fn edit(timesheet: &mut Timesheet, name: &str, day: usize, hours: Option<f64>, code: Option<&str>) {
        let entry = &mut timesheet.employees.iter_mut().find(|e| e.name == name).unwrap().days[day - 1];
        entry.hours = hours;
        entry.code = code.map(Into::into);
    }

    #[test]
    fn the_hours_typed_in_the_file_become_the_hours_of_the_app() {
        let settings = Settings::default();
        let sheet = october();
        let file = file_after(&sheet, "typed", |ts| {
            edit(ts, "POPESCU ION", 5, Some(8.0), None); // the app had 4
            edit(ts, "POPESCU ION", 12, Some(7.5), None); // the app had 8: lower, to the half hour
            edit(ts, "POPESCU ION", 7, Some(6.0), None); // a day without trips
            edit(ts, "ANGHEL DAN", 6, None, None); // cleared
            edit(ts, "ANGHEL DAN", 8, None, Some("CO")); // a leave on a free day
        });
        let done = reconcile(&sheet, &file, &settings).expect("the same month");
        let timesheet = to_timesheet(&done.sheet, &settings);

        assert_eq!(day(&timesheet, "POPESCU ION", 5).hours, Some(8.0));
        assert_eq!(day(&timesheet, "POPESCU ION", 12).hours, Some(7.5));
        assert_eq!(day(&timesheet, "POPESCU ION", 7).hours, Some(6.0));
        assert_eq!(day(&timesheet, "ANGHEL DAN", 6).hours, None, "the cleared day is out of the pontaj");
        assert_eq!(day(&timesheet, "ANGHEL DAN", 8).code.as_deref(), Some("CO"));

        let report = &done.report;
        assert_eq!((report.matched, report.others, report.not_in_file.len()), (2, 0, 0));
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        let listed: Vec<(String, u32, String, String)> =
            report.changes.iter().map(|c| (c.driver.clone(), chrono::Datelike::day(&c.date.unwrap()), c.app.clone(), c.excel.clone())).collect();
        assert_eq!(
            listed,
            [
                ("Anghel Dan".to_string(), 6, "5".to_string(), String::new()),
                ("Anghel Dan".to_string(), 8, String::new(), "CO".to_string()),
                ("Popescu Ion".to_string(), 5, "4".to_string(), "8".to_string()),
                ("Popescu Ion".to_string(), 7, String::new(), "6".to_string()),
                ("Popescu Ion".to_string(), 12, "8".to_string(), "7,5".to_string()),
            ]
        );
        // The days the file did not change are not in the list, and the totals follow the days.
        let popescu = done.sheet.drivers.iter().find(|d| d.name == "Popescu Ion").unwrap();
        assert_eq!(popescu.totals.total_min, (8 + 6) * 60 + 450, "8 + 6 + 7.5 hours");
        assert!(popescu.days.iter().filter(|d| d.adjusted).count() == 3);
    }

    #[test]
    fn a_second_reglare_with_the_same_file_changes_nothing() {
        let settings = Settings::default();
        let sheet = october();
        let file = file_after(&sheet, "twice", |ts| {
            edit(ts, "POPESCU ION", 5, Some(8.0), None);
            edit(ts, "ANGHEL DAN", 6, None, None);
        });
        let first = reconcile(&sheet, &file, &settings).unwrap();
        let second = reconcile(&first.sheet, &file, &settings).unwrap();
        assert!(second.report.changes.is_empty() && second.report.skipped.is_empty(), "{:?}", second.report);
        assert_eq!(second.sheet, first.sheet);
    }

    #[test]
    fn the_exported_file_of_the_reglated_month_has_the_hours_of_the_file() {
        let settings = Settings::default();
        let sheet = october();
        let file = file_after(&sheet, "roundtrip", |ts| {
            edit(ts, "POPESCU ION", 5, Some(8.0), None);
            edit(ts, "POPESCU ION", 12, Some(7.5), None);
            edit(ts, "POPESCU ION", 7, Some(6.0), None);
            edit(ts, "ANGHEL DAN", 6, None, None);
            edit(ts, "ANGHEL DAN", 8, None, Some("CO"));
        });
        let done = reconcile(&sheet, &file, &settings).unwrap();

        let path = std::env::temp_dir().join(format!("pontaj-reglare-export-{}.xlsx", std::process::id()));
        crate::catb::excel::write_xlsx(&done.sheet, &settings, &path).unwrap();
        let exported = crate::pontaj_file::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        for person in &file.people {
            let back = exported.people.iter().find(|p| p.name == person.name).expect("the person is in the new file");
            assert_eq!(back.days, person.days, "{}", person.name);
        }
    }

    #[test]
    fn a_leave_on_a_day_with_trips_and_a_code_the_app_does_not_know_are_reported_not_applied() {
        let settings = Settings::default();
        let sheet = october();
        let file = file_after(&sheet, "skipped", |ts| {
            edit(ts, "POPESCU ION", 5, None, Some("CO")); // a day with a trip
            edit(ts, "ANGHEL DAN", 8, None, Some("OLP")); // not a leave of Categoria B
            edit(ts, "ANGHEL DAN", 9, Some(30.0), None); // more than a day
        });
        let done = reconcile(&sheet, &file, &settings).unwrap();
        let timesheet = to_timesheet(&done.sheet, &settings);
        assert_eq!(day(&timesheet, "POPESCU ION", 5).hours, Some(4.0), "untouched");
        assert!(done.report.changes.is_empty(), "{:?}", done.report.changes);
        // In the order of the drivers (Anghel, then Popescu) and of the days.
        let reasons: Vec<&str> = done.report.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons[0].contains("OLP") && reasons[1].contains("prea multe") && reasons[2].contains("are curse"), "{reasons:?}");
    }

    #[test]
    fn a_file_of_another_month_is_refused_and_so_is_one_without_the_drivers() {
        let settings = Settings::default();
        let sheet = october();
        let mut file = file_after(&sheet, "month", |_| {});
        file.month = 9;
        let error = reconcile(&sheet, &file, &settings).unwrap_err().to_string();
        assert!(error.contains("09.2026") && error.contains("10.2026"), "{error}");

        let mut strangers = file_after(&sheet, "strangers", |_| {});
        strangers.people.iter_mut().for_each(|p| p.name = format!("ALTCINEVA {}", p.name));
        let error = reconcile(&sheet, &strangers, &settings).unwrap_err().to_string();
        assert!(error.contains("Niciun șofer"), "{error}");
    }

    #[test]
    fn drivers_missing_from_the_file_stay_as_they_are_and_other_people_of_the_file_are_counted() {
        let settings = Settings::default();
        let sheet = october();
        let mut file = file_after(&sheet, "missing", |ts| edit(ts, "POPESCU ION", 5, Some(8.0), None));
        file.people.retain(|p| p.name != "ANGHEL DAN");
        file.people.push(Person { name: "BIROU ANA".into(), days: Vec::new() });
        let done = reconcile(&sheet, &file, &settings).unwrap();
        assert_eq!(done.report.not_in_file, ["Anghel Dan"]);
        assert_eq!((done.report.matched, done.report.others), (1, 1));
        let anghel = done.sheet.drivers.iter().find(|d| d.name == "Anghel Dan").unwrap();
        assert_eq!(anghel.totals, sheet.drivers.iter().find(|d| d.name == "Anghel Dan").unwrap().totals);
    }
}
