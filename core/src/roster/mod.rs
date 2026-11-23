//! The office, garage and city staff. Their hours are not read from a tachograph report or an API: they follow a plan (8 hours
//! on the working days, or shifts of 12 hours in a cycle of 2 days on and 2 days off), so a month is inferred from it and then
//! edited like any other.
//!
//! - [`StaffMember`] and [`Plan`] are what the settings keep: who the person is and how the days are decided. A cycle is tied to
//!   a date (`anchor`), so it goes on from one month to the next by itself.
//! - [`open`] gives the month of a group: what was saved (the edits of the office) laid over what the plans say.
//! - [`seed`] is the list of people the app starts with. There is no import of any file: the hours are known from the plans and go
//!   straight into the Excel export; the office only edits them in the tab of the group when something differs.

pub mod seed;

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::calendar;
use crate::month_file;
use crate::rules;
use crate::settings::Settings;
use crate::timesheet::{self, display_name, DayEntry, Employee, Group, Interval, Timesheet, Totals};
use crate::Result;

/// The groups that have a plan, in the order of the tabs.
pub const GROUPS: [Group; 3] = [Group::Office, Group::Garage, Group::City];

/// How the days of a person are decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Plan {
    /// The same times on every Monday–Friday that is not a legal holiday (8 hours in the office and the garage).
    Weekdays { start: i32, end: i32 },
    /// A cycle: the `on` shifts one after another, then `off` free days, over and over (12 hours, two days on and two off).
    /// `anchor` is a day on which the cycle starts, with its first shift; any day of the cycle works, it goes on from there in
    /// both directions. A shift that ends after midnight has an `end` above 24:00 (20:00–08:00 is 1200–1920).
    Rotation { anchor: NaiveDate, on: Vec<Interval>, off: u32 },
}

impl Plan {
    /// 08:00–16:00 on the working days.
    pub fn standard_weekdays() -> Self {
        Plan::Weekdays { start: 8 * 60, end: 16 * 60 }
    }

    /// Two shifts of 12 hours (09:00–21:00) and two free days, starting on `anchor`.
    pub fn standard_rotation(anchor: NaiveDate) -> Self {
        let shift = Interval { start: 9 * 60, end: 21 * 60 };
        Plan::Rotation { anchor, on: vec![shift.clone(), shift], off: 2 }
    }

    /// The times of the day's shift, or `None` when the person is off that day.
    pub fn shift_on(&self, date: NaiveDate) -> Option<Interval> {
        match self {
            Plan::Weekdays { start, end } => {
                let working = !calendar::is_weekend(date) && calendar::holiday_name(date).is_none();
                working.then(|| Interval { start: *start, end: *end })
            }
            Plan::Rotation { anchor, on, off } => {
                let cycle = on.len() as i64 + i64::from(*off);
                if on.is_empty() || cycle <= 0 {
                    return None;
                }
                let at = (date - *anchor).num_days().rem_euclid(cycle) as usize;
                on.get(at).cloned()
            }
        }
    }
}

/// A person of the office, the garage or the city, with the plan the hours are inferred from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaffMember {
    /// As in the pontaj (capitals, no diacritics): "POPESCU ION".
    pub name: String,
    pub function: String,
    /// [`Group::Office`], [`Group::Garage`] or [`Group::City`].
    pub group: Group,
    pub plan: Plan,
    /// What the office should check about the plan (it could not be read fully from the file it came from). Empty = nothing.
    #[serde(default)]
    pub note: String,
}

/// The key two names are the same person by: the way the pontaj writes it.
fn key(name: &str) -> String {
    display_name(name, true)
}

/// One person's month from the plan: every day with its shift as the typed times (the hours, the night and the totals follow
/// from them like for any day typed by hand; the office's edits are layered on top later).
pub fn employee(member: &StaffMember, year: i32, month: u32, plain_names: bool) -> Employee {
    let days = (1..=calendar::days_in_month(year, month))
        .map(|day| {
            let shift = NaiveDate::from_ymd_opt(year, month, day).and_then(|date| member.plan.shift_on(date));
            DayEntry { day, override_intervals: shift.map(|iv| vec![iv]), ..Default::default() }
        })
        .collect();
    Employee {
        name: display_name(&member.name, plain_names),
        function: member.function.clone(),
        location: String::new(),
        group: member.group,
        norm_hours: None,
        activities: Vec::new(),
        days,
        totals: Totals::default(),
    }
}

/// The month of one group straight from the plans: no saved edits. Employees come in alphabetical order, like the payroll file.
pub fn build_sheet(staff: &[StaffMember], group: Group, year: i32, month: u32, settings: &Settings) -> Timesheet {
    let mut employees: Vec<Employee> = Vec::new();
    for member in staff.iter().filter(|m| m.group == group) {
        let fresh = employee(member, year, month, settings.plain_names);
        if !employees.iter().any(|e| key(&e.name) == key(&fresh.name)) {
            employees.push(fresh);
        }
    }
    employees.sort_by(|a, b| a.name.cmp(&b.name));
    let mut sheet = Timesheet {
        company: String::new(),
        location: String::new(),
        year,
        month,
        working_days: 0,
        norm_hours: 0,
        days: timesheet::month_days(year, month),
        employees,
        warnings: Vec::new(),
        notices: Vec::new(),
        sources: Vec::new(),
    };
    rules::recompute(&mut sheet, settings);
    sheet
}

/// A saved month brought in line with the staff list: the people the list has are there (a new one from his plan, the others
/// with the days the office left them), the people it no longer has are gone, names and jobs are the list's.
pub fn sync(saved: Option<Timesheet>, staff: &[StaffMember], group: Group, year: i32, month: u32, settings: &Settings) -> Timesheet {
    let mut sheet = build_sheet(staff, group, year, month, settings);
    let Some(saved) = saved.filter(|s| (s.year, s.month) == (year, month)) else { return sheet };
    for employee in &mut sheet.employees {
        let kept = saved.employees.iter().find(|old| key(&old.name) == key(&employee.name)).filter(|old| old.days.len() == employee.days.len());
        if let Some(old) = kept {
            employee.days = old.days.clone();
        }
    }
    rules::recompute(&mut sheet, settings);
    sheet
}

/// Puts one person's days back as the plan says, dropping what the office typed on them.
pub fn regenerate(sheet: &mut Timesheet, name: &str, staff: &[StaffMember], settings: &Settings) {
    let Some(member) = staff.iter().find(|m| key(&m.name) == key(name)) else { return };
    let fresh = employee(member, sheet.year, sheet.month, settings.plain_names);
    if let Some(current) = sheet.employees.iter_mut().find(|e| key(&e.name) == key(name)) {
        current.days = fresh.days;
    }
    rules::recompute(sheet, settings);
}

// ---- saving ----

/// The folder of a group inside the app's roster folder.
pub fn group_key(group: Group) -> &'static str {
    match group {
        Group::Office => "birou",
        Group::Garage => "garaj",
        Group::City => "oras",
        _ => "alte",
    }
}

fn group_dir(dir: &Path, group: Group) -> PathBuf {
    dir.join(group_key(group))
}

/// Saves the month of a group.
pub fn save(dir: &Path, group: Group, sheet: &Timesheet) -> Result<()> {
    month_file::save(&group_dir(dir, group), sheet.year, sheet.month, sheet)
}

/// The month of a group as the screen shows it: what was saved, brought in line with the staff list.
pub fn open(dir: &Path, group: Group, year: i32, month: u32, settings: &Settings) -> Result<Timesheet> {
    let saved: Option<Timesheet> = month_file::load(&group_dir(dir, group), year, month)?;
    Ok(sync(saved, &settings.roster, group, year, month, settings))
}

/// Everybody of the office, the garage and the city for a month, for the Excel file.
pub fn open_employees(dir: &Path, year: i32, month: u32, settings: &Settings) -> Result<Vec<Employee>> {
    let mut all = Vec::new();
    for group in GROUPS {
        all.extend(open(dir, group, year, month, settings)?.employees);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn member(name: &str, group: Group, plan: Plan) -> StaffMember {
        StaffMember { name: name.into(), function: "Secretara".into(), group, plan, note: String::new() }
    }

    fn hours(sheet: &Timesheet, name: &str) -> Vec<Option<f64>> {
        sheet.employees.iter().find(|e| e.name == name).unwrap().days.iter().map(|d| d.hours).collect()
    }

    #[test]
    fn eight_hours_on_the_working_days_and_nothing_on_weekends_and_holidays() {
        let settings = Settings::default();
        let staff = [member("Popescu Ion", Group::Office, Plan::standard_weekdays())];
        let sheet = build_sheet(&staff, Group::Office, 2026, 11, &settings);
        let days = hours(&sheet, "POPESCU ION");
        assert_eq!(days[0], None, "1 November 2026 is a Sunday");
        assert_eq!(days[1], Some(8.0), "Monday");
        assert_eq!(days[6], None, "Saturday");
        assert_eq!(days[29], None, "30 November, Sf. Andrei, is a holiday");
        assert_eq!(sheet.employees[0].totals.worked, 160.0, "20 working days of 8 hours: the norm of the month");
        assert_eq!(sheet.employees[0].days[1].intervals, [Interval { start: 480, end: 960 }], "08:00-16:00");
    }

    #[test]
    fn a_cycle_of_two_days_on_and_two_off_goes_on_from_one_month_to_the_next() {
        let settings = Settings::default();
        // 12 hours on 30 Oct and 31 Oct, off on 1 and 2 Nov, on again on 3 and 4 Nov, and so on.
        let plan = Plan::Rotation { anchor: date(2026, 10, 30), on: vec![Interval { start: 540, end: 1200 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = [member("Dragan Mirel", Group::City, plan)];
        let november = build_sheet(&staff, Group::City, 2026, 11, &settings);
        let days = hours(&november, "DRAGAN MIREL");
        assert_eq!(&days[0..6], &[None, None, Some(11.0), Some(12.0), None, None], "1-6 November: off, off, 11 h, 12 h, off, off");
        assert_eq!(days[6..8], [Some(11.0), Some(12.0)]);
        let october = build_sheet(&staff, Group::City, 2026, 10, &settings);
        let days = hours(&october, "DRAGAN MIREL");
        assert_eq!(days[29..31], [Some(11.0), Some(12.0)], "the same cycle seen from October");
        assert_eq!(days[0], None, "1 October: 29 days before the anchor is position 3 of 4, a free day");
    }

    #[test]
    fn a_shift_over_midnight_is_written_on_the_day_it_starts_and_counts_its_night() {
        let settings = Settings::default();
        let plan = Plan::Rotation { anchor: date(2026, 11, 1), on: vec![Interval { start: 20 * 60, end: 32 * 60 }], off: 1 };
        let sheet = build_sheet(&[member("Nocturn Ana", Group::City, plan)], Group::City, 2026, 11, &settings);
        let first = &sheet.employees[0].days[0];
        assert_eq!(first.hours, Some(12.0));
        assert_eq!(first.intervals, [Interval { start: 1200, end: 1920 }]);
        assert_eq!(first.night_hours, Some(7.0), "00:00-07:00 of the next day, as the app counts the night");
        assert_eq!(sheet.employees[0].days[1].hours, None);
    }

    #[test]
    fn the_saved_days_stay_and_the_staff_list_decides_who_is_there() {
        let settings = Settings::default();
        let mut staff = vec![member("Ana Ionescu", Group::Garage, Plan::standard_weekdays()), member("Bogdan Vasile", Group::Garage, Plan::standard_weekdays()), member("Alt Om", Group::Office, Plan::standard_weekdays())];
        let mut saved = build_sheet(&staff, Group::Garage, 2026, 11, &settings);
        assert_eq!(saved.employees.len(), 2, "only the people of the group");
        // The office puts a leave on the 2nd for Ana and changes the 3rd to 4 hours.
        saved.employees[0].days[1].code = Some("CO".into());
        saved.employees[0].days[2].override_hours = Some(4.0);
        // Bogdan leaves the list, Cristian joins.
        staff.retain(|m| m.name != "Bogdan Vasile");
        staff.push(member("Cristian Pop", Group::Garage, Plan::standard_weekdays()));
        let sheet = sync(Some(saved), &staff, Group::Garage, 2026, 11, &settings);
        let names: Vec<&str> = sheet.employees.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["ANA IONESCU", "CRISTIAN POP"]);
        let ana = &sheet.employees[0];
        assert_eq!((ana.days[1].code.as_deref(), ana.days[2].hours), (Some("CO"), Some(4.0)), "the edits of the office stay");
        assert_eq!(sheet.employees[1].days[1].hours, Some(8.0), "the new person comes from his plan");
        assert_eq!(ana.totals.code_hours[0], 8.0, "a day of leave is a norm day in its column");
    }

    #[test]
    fn a_saved_month_of_another_month_is_not_used_and_regenerate_drops_the_edits_of_one_person() {
        let settings = Settings::default();
        let staff = [member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let mut october = build_sheet(&staff, Group::Office, 2026, 10, &settings);
        october.employees[0].days[0].override_hours = Some(2.0);
        let november = sync(Some(october), &staff, Group::Office, 2026, 11, &settings);
        assert_eq!((november.month, november.employees[0].days[1].hours), (11, Some(8.0)));

        let mut edited = build_sheet(&staff, Group::Office, 2026, 11, &settings);
        edited.employees[0].days[1].override_hours = Some(3.0);
        rules::recompute(&mut edited, &settings);
        assert_eq!(edited.employees[0].days[1].hours, Some(3.0));
        regenerate(&mut edited, "ana ionescu", &staff, &settings);
        assert_eq!(edited.employees[0].days[1].hours, Some(8.0), "back to the plan");
    }

    #[test]
    fn each_group_is_saved_in_its_own_folder() {
        let settings = Settings::default();
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays()), member("Bogdan Vasile", Group::Garage, Plan::standard_weekdays())];
        let settings = Settings { roster: staff, ..settings };
        let dir = std::env::temp_dir().join(format!("pontaj-roster-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut office = open(&dir, Group::Office, 2026, 11, &settings).unwrap();
        office.employees[0].days[1].code = Some("CM".into());
        save(&dir, Group::Office, &office).unwrap();
        let again = open(&dir, Group::Office, 2026, 11, &settings).unwrap();
        assert_eq!(again.employees[0].days[1].code.as_deref(), Some("CM"));
        assert!(dir.join("birou").join("2026-11.json").exists() && !dir.join("garaj").exists());
        let everybody = open_employees(&dir, 2026, 11, &settings).unwrap();
        assert_eq!(everybody.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["ANA IONESCU", "BOGDAN VASILE"]);
        assert_eq!(everybody.iter().map(|e| e.group).collect::<Vec<_>>(), [Group::Office, Group::Garage]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
