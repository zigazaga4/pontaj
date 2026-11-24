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

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::calendar;
use crate::leave::{touches, LeavePlan};
use crate::store;
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
        applied_leaves: Vec::new(),
        days,
        totals: Totals::default(),
    }
}

/// Puts the planned leaves ([`LeavePlan`]) that touch the month on the days the person would work: those days carry the code of the
/// leave instead of the shift. Days off stay empty, and a day that already has a code keeps it. Each plan is applied once per
/// person and month (the person remembers it in `applied_leaves`), so a day cleared by hand does not get its leave back.
fn apply_leaves(employee: &mut Employee, member: &StaffMember, year: i32, month: u32, plans: &[LeavePlan]) {
    for plan in plans.iter().filter(|p| p.group == member.group && p.has_known_code() && p.is_for(member.group, &member.name, None)) {
        let days = plan.days_in(year, month);
        if days.is_empty() || employee.applied_leaves.contains(&plan.id) {
            continue;
        }
        for date in days.into_iter().filter(|date| member.plan.shift_on(*date).is_some()) {
            let Some(day) = employee.days.get_mut(date.day0() as usize) else { continue };
            if day.code.is_none() {
                *day = DayEntry { day: day.day, code: Some(plan.code.clone()), ..Default::default() };
            }
        }
        employee.applied_leaves.push(plan.id.clone());
    }
}

/// [`apply_leaves`] for everybody of the sheet who is on the staff list.
fn apply_plans(sheet: &mut Timesheet, staff: &[StaffMember], group: Group, plans: &[LeavePlan]) {
    let (year, month) = (sheet.year, sheet.month);
    for employee in &mut sheet.employees {
        if let Some(member) = staff.iter().find(|m| m.group == group && key(&m.name) == key(&employee.name)) {
            apply_leaves(employee, member, year, month, plans);
        }
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
    apply_plans(&mut sheet, staff, group, &settings.leaves);
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
            employee.applied_leaves = old.applied_leaves.clone();
        }
    }
    // The plans the saved days have not taken yet (a leave written down after the month was last saved).
    apply_plans(&mut sheet, staff, group, &settings.leaves);
    rules::recompute(&mut sheet, settings);
    sheet
}

/// Puts one person's days back as the plan says, dropping what the office typed on them.
pub fn regenerate(sheet: &mut Timesheet, name: &str, staff: &[StaffMember], settings: &Settings) {
    let Some(member) = staff.iter().find(|m| key(&m.name) == key(name)) else { return };
    let fresh = employee(member, sheet.year, sheet.month, settings.plain_names);
    if let Some(current) = sheet.employees.iter_mut().find(|e| key(&e.name) == key(name)) {
        current.days = fresh.days;
        // The leaves written down in the list of leaves stay: they are not something typed on the days.
        current.applied_leaves.clear();
        apply_leaves(current, member, sheet.year, sheet.month, &settings.leaves);
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
    store::save(&group_dir(dir, group), sheet.year, sheet.month, sheet)
}

/// The month of a group as the screen shows it: what was saved, brought in line with the staff list.
pub fn open(dir: &Path, group: Group, year: i32, month: u32, settings: &Settings) -> Result<Timesheet> {
    let saved: Option<Timesheet> = store::load(&group_dir(dir, group), year, month)?;
    Ok(sync(saved, &settings.roster, group, year, month, settings))
}

/// Takes a planned leave back out of the saved months of its group: the days that still carry the plan’s code go back to the
/// person’s plan. What the office changed on other days stays. Months never saved hold nothing of it (it is applied in memory
/// when they are opened) and the saved ones are written only when they changed.
pub fn strip_plan(dir: &Path, plan: &LeavePlan, settings: &Settings) -> Result<()> {
    let folder = group_dir(dir, plan.group);
    let Some(member) = settings.roster.iter().find(|m| m.group == plan.group && plan.is_for(m.group, &m.name, None)) else { return Ok(()) };
    for (year, month) in store::list(&folder).into_iter().filter(|(year, month)| touches(plan, *year, *month)) {
        let Some(mut sheet) = store::load::<Timesheet>(&folder, year, month)? else { continue };
        let fresh = employee(member, year, month, settings.plain_names);
        let Some(current) = sheet.employees.iter_mut().find(|e| key(&e.name) == key(&member.name)) else { continue };
        if !current.applied_leaves.contains(&plan.id) {
            continue;
        }
        for date in plan.days_in(year, month) {
            let at = date.day0() as usize;
            if current.days.get(at).is_some_and(|day| day.code.as_deref() == Some(plan.code.as_str())) {
                if let Some(planned) = fresh.days.get(at) {
                    current.days[at] = planned.clone();
                }
            }
        }
        current.applied_leaves.retain(|id| *id != plan.id);
        rules::recompute(&mut sheet, settings);
        save(dir, plan.group, &sheet)?;
    }
    Ok(())
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

    fn leave(id: &str, name: &str, group: Group, code: &str, from: u32, to: u32) -> LeavePlan {
        LeavePlan { id: id.into(), group, name: name.into(), driver_id: None, code: code.into(), from: date(2026, 12, from), to: date(2026, 12, to) }
    }

    fn codes(sheet: &Timesheet, name: &str) -> Vec<(u32, String)> {
        let employee = sheet.employees.iter().find(|e| e.name == name).unwrap();
        employee.days.iter().filter_map(|d| d.code.clone().map(|c| (d.day, c))).collect()
    }

    #[test]
    fn a_planned_leave_takes_only_the_days_the_person_would_work() {
        // 14 December 2026 is a Monday: Mon 14 – Sun 20.
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 20)], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        let days: Vec<u32> = codes(&sheet, "ANA IONESCU").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [14, 15, 16, 17, 18], "no leave on Saturday and Sunday: she does not work them");
        let ana = &sheet.employees[0];
        assert_eq!(ana.totals.code_hours[0], 40.0, "five days of leave are five norm days in the CO column");
        assert_eq!((ana.days[13].hours, ana.days[13].intervals.len()), (None, 0), "the day has no hours and no times of its own");
        assert_eq!(ana.days[20].hours, Some(8.0), "the week after is as the plan says");
        assert_eq!(ana.applied_leaves, ["L1"]);
        // A leave of another group, another person or a month that is not this one changes nothing.
        let other = Settings { leaves: vec![leave("L2", "Ana Ionescu", Group::Garage, "CO", 14, 20), leave("L3", "Alt Om", Group::Office, "CO", 14, 20)], ..settings.clone() };
        assert!(codes(&build_sheet(&staff, Group::Office, 2026, 12, &other), "ANA IONESCU").is_empty());
        assert!(codes(&build_sheet(&staff, Group::Office, 2026, 11, &settings), "ANA IONESCU").is_empty());
    }

    #[test]
    fn on_a_cycle_the_leave_takes_the_shifts_and_not_the_free_days() {
        // Two shifts of 12 hours from 1 December, then two days off: on 1, 2, 5, 6, 9, 10, 13, 14…
        let plan = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = vec![member("Dragan Mirel", Group::City, plan)];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Dragan Mirel", Group::City, "CM", 4, 10)], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::City, 2026, 12, &settings);
        let days: Vec<u32> = codes(&sheet, "DRAGAN MIREL").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [5, 6, 9, 10]);
    }

    #[test]
    fn a_plan_is_applied_once_so_a_day_cleared_by_hand_stays_cleared() {
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 18)], ..Settings::default() };
        let dir = std::env::temp_dir().join(format!("pontaj-roster-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut sheet = open(&dir, Group::Office, 2026, 12, &settings).unwrap();
        assert_eq!(codes(&sheet, "ANA IONESCU").len(), 5, "applied when the month is opened, though nothing was saved yet");
        // The office clears the 16th and types 4 hours on it; the month is saved.
        sheet.employees[0].days[15].code = None;
        sheet.employees[0].days[15].override_hours = Some(4.0);
        rules::recompute(&mut sheet, &settings);
        save(&dir, Group::Office, &sheet).unwrap();
        let again = open(&dir, Group::Office, 2026, 12, &settings).unwrap();
        let days: Vec<u32> = codes(&again, "ANA IONESCU").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [14, 15, 17, 18]);
        assert_eq!(again.employees[0].days[15].hours, Some(4.0));
        // A plan written down after the month was saved is applied when it is opened next.
        let later = Settings { leaves: vec![leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 18), leave("L2", "Ana Ionescu", Group::Office, "CM", 21, 22)], ..settings.clone() };
        let with_second = open(&dir, Group::Office, 2026, 12, &later).unwrap();
        assert_eq!(codes(&with_second, "ANA IONESCU").iter().filter(|(_, c)| c == "CM").map(|(d, _)| *d).collect::<Vec<_>>(), [21, 22]);
        assert_eq!(with_second.employees[0].days[15].hours, Some(4.0), "the first plan did not come back on the 16th");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_person_put_back_on_his_plan_keeps_his_planned_leaves() {
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 18)], ..Settings::default() };
        let mut sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        sheet.employees[0].days[2].override_hours = Some(3.0);
        regenerate(&mut sheet, "Ana Ionescu", &staff, &settings);
        assert_eq!(sheet.employees[0].days[2].hours, Some(8.0), "the typed hours are gone");
        assert_eq!(codes(&sheet, "ANA IONESCU").len(), 5, "the leave stays");
    }

    #[test]
    fn taking_a_plan_back_gives_its_days_back_to_the_plan_of_the_person() {
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let plan = leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 18);
        let settings = Settings { roster: staff.clone(), leaves: vec![plan.clone()], ..Settings::default() };
        let dir = std::env::temp_dir().join(format!("pontaj-roster-strip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut sheet = open(&dir, Group::Office, 2026, 12, &settings).unwrap();
        sheet.employees[0].days[15].code = Some("CM".into()); // the office changed the 16th to another leave
        rules::recompute(&mut sheet, &settings);
        save(&dir, Group::Office, &sheet).unwrap();
        strip_plan(&dir, &plan, &settings).unwrap();
        let without = Settings { leaves: Vec::new(), ..settings.clone() };
        let back = open(&dir, Group::Office, 2026, 12, &without).unwrap();
        assert_eq!(codes(&back, "ANA IONESCU"), [(16, "CM".to_string())], "only what the plan put there is gone");
        assert_eq!(back.employees[0].days[13].hours, Some(8.0), "the 14th follows the plan again");
        assert!(back.employees[0].applied_leaves.is_empty());
        std::fs::remove_dir_all(&dir).ok();
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
