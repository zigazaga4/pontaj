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

use chrono::{Datelike, Duration, NaiveDate};
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
    /// No plan: the person has no hours of her own (zero), the office writes them by hand when there is something to write.
    Manual,
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
            Plan::Manual => None,
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

/// A move of a person's shift cycle: from `from` on, the cycle is moved by `days` (positive = the shifts come that many days later,
/// negative = earlier). The moves of a person add up, each one from its own day, and the cycle goes on from there by itself, month
/// after month. Only a cycle ([`Plan::Rotation`]) can be moved: the working week has no cycle to move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShiftMove {
    pub from: NaiveDate,
    pub days: i32,
}

impl Default for ShiftMove {
    fn default() -> Self {
        Self { from: NaiveDate::default(), days: 0 }
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
    /// The moves of the shift cycle (see [`ShiftMove`]); none for most people.
    #[serde(default)]
    pub moves: Vec<ShiftMove>,
}

impl StaffMember {
    /// By how many days the cycle is moved on `date`: the moves that started on or before it, added up.
    pub fn offset_on(&self, date: NaiveDate) -> i64 {
        self.moves.iter().filter(|m| m.from <= date).map(|m| i64::from(m.days)).sum()
    }

    /// Is the day in the norm of the person (a day he works, or for somebody who types everything by hand a working day of the
    /// calendar)? A leave on any other day is a leave on a free day.
    pub fn works_on(&self, date: NaiveDate) -> bool {
        match self.plan {
            Plan::Manual => !calendar::is_weekend(date) && calendar::holiday_name(date).is_none(),
            Plan::Weekdays { .. } | Plan::Rotation { .. } => self.shift_on(date).is_some(),
        }
    }

    /// The times of the day's shift, or `None` when the person is off that day: the plan, with the moves of the cycle taken into account.
    pub fn shift_on(&self, date: NaiveDate) -> Option<Interval> {
        match self.plan {
            Plan::Rotation { .. } => self.plan.shift_on(date - Duration::days(self.offset_on(date))),
            Plan::Weekdays { .. } | Plan::Manual => self.plan.shift_on(date),
        }
    }
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
            let shift = NaiveDate::from_ymd_opt(year, month, day).and_then(|date| member.shift_on(date));
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
/// leave instead of the shift. Days off stay empty (unless the plan is for the free days too, see [`LeavePlan::on_free_days`]), and a day that already has a code keeps it. Each plan is applied once per
/// person and month (the person remembers it in `applied_leaves`), so a day cleared by hand does not get its leave back.
fn apply_leaves(employee: &mut Employee, member: &StaffMember, year: i32, month: u32, plans: &[LeavePlan]) {
    for plan in plans.iter().filter(|p| p.group == member.group && p.has_known_code() && p.is_for(member.group, &member.name, None)) {
        let days = plan.days_in(year, month);
        if days.is_empty() {
            continue;
        }
        if employee.applied_leaves.contains(&plan.id) {
            // A leave taken by an older version lost the times of the shift it replaced: they come back, so the day counts its hours.
            for date in days {
                let Some(day) = employee.days.get_mut(date.day0() as usize) else { continue };
                if day.code.as_deref() == Some(plan.code.as_str()) && day.override_intervals.is_none() && !day.leave_on_free_day {
                    day.override_intervals = member.shift_on(date).map(|shift| vec![shift]);
                }
            }
            continue;
        }
        for date in days.into_iter().filter(|date| plan.on_free_days || member.works_on(*date)) {
            let Some(day) = employee.days.get_mut(date.day0() as usize) else { continue };
            if day.code.is_none() {
                // The leave keeps the times of the shift it takes the place of: they say how many hours the day of leave counts.
                *day = DayEntry { day: day.day, code: Some(plan.code.clone()), override_intervals: day.override_intervals.take(), leave_on_free_day: !member.works_on(date), ..Default::default() };
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

/// What a person's day would be in a saved month if the office had never typed on it: the plan of the person, with the planned
/// leaves the month already took (`applied`) put on the days he works.
fn untouched(member: &StaffMember, year: i32, month: u32, applied: &[String], settings: &Settings) -> Employee {
    let mut fresh = employee(member, year, month, settings.plain_names);
    let taken: Vec<LeavePlan> = settings.leaves.iter().filter(|plan| applied.contains(&plan.id)).cloned().collect();
    apply_leaves(&mut fresh, member, year, month, &taken);
    fresh
}

/// Do two days hold the same typing (hours, times, code)? What the program works out from them does not count.
fn same_typing(a: &DayEntry, b: &DayEntry) -> bool {
    a.override_hours == b.override_hours && a.override_night == b.override_night && a.override_intervals == b.override_intervals && a.code == b.code
}

/// The shift cycle of a person was moved from `from` on (a move was added or taken back): the saved months bring their days from
/// that day on in line with the new cycle. `before` is the person as the saved days were made, the settings hold him as he is
/// now. Only the days nobody typed on follow the new cycle; a day the office changed stays as it is. Months that were never saved
/// need nothing (they are made from the person's plan when opened), and a saved month is written only when a day changed.
pub fn move_cycle(dir: &Path, before: &StaffMember, from: NaiveDate, settings: &Settings) -> Result<()> {
    let folder = group_dir(dir, before.group);
    let Some(after) = settings.roster.iter().find(|m| m.group == before.group && key(&m.name) == key(&before.name)) else { return Ok(()) };
    for (year, month) in store::list(&folder).into_iter().filter(|month| *month >= (from.year(), from.month())) {
        let Some(mut sheet) = store::load::<Timesheet>(&folder, year, month)? else { continue };
        let Some(current) = sheet.employees.iter_mut().find(|e| key(&e.name) == key(&after.name)) else { continue };
        let (was, now) = (untouched(before, year, month, &current.applied_leaves, settings), untouched(after, year, month, &current.applied_leaves, settings));
        let mut changed = false;
        for (at, day) in current.days.iter_mut().enumerate() {
            let on = NaiveDate::from_ymd_opt(year, month, day.day);
            let (Some(old), Some(new)) = (was.days.get(at), now.days.get(at)) else { continue };
            if on.is_some_and(|date| date >= from) && same_typing(day, old) && !same_typing(old, new) {
                *day = new.clone();
                changed = true;
            }
        }
        if changed {
            rules::recompute(&mut sheet, settings);
            save(dir, before.group, &sheet)?;
        }
    }
    Ok(())
}

/// The plans of some people changed (a migration of the settings): every saved month follows the new plan on the days nobody typed
/// on, like for a moved cycle ([`move_cycle`]). `before` is the list as it was, the settings hold the list as it is now.
pub fn follow_plans(dir: &Path, before: &[StaffMember], settings: &Settings) -> Result<()> {
    for old in before {
        let changed = settings.roster.iter().any(|now| now.group == old.group && key(&now.name) == key(&old.name) && (now.plan != old.plan || now.moves != old.moves));
        if changed {
            move_cycle(dir, old, NaiveDate::MIN, settings)?;
        }
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
        StaffMember { name: name.into(), function: "Secretara".into(), group, plan, note: String::new(), moves: Vec::new() }
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
        LeavePlan { id: id.into(), group, name: name.into(), driver_id: None, code: code.into(), from: date(2026, 12, from), to: date(2026, 12, to), on_free_days: false }
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

    /// Two shifts on, two days off, from 1 December 2026: on 1, 2, 5, 6, 9, 10, 13, 14, 17, 18, 21, 22…
    fn cycle() -> Plan {
        Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 }
    }

    fn mover(moves: &[(u32, i32)]) -> StaffMember {
        StaffMember { moves: moves.iter().map(|(day, days)| ShiftMove { from: date(2026, 12, *day), days: *days }).collect(), ..member("Dragan Mirel", Group::City, cycle()) }
    }

    fn on_days(sheet: &Timesheet) -> Vec<u32> {
        sheet.employees.iter().find(|e| e.name == "DRAGAN MIREL").unwrap().days.iter().filter(|d| d.hours.is_some()).map(|d| d.day).collect()
    }

    fn december(staff: &StaffMember) -> Vec<u32> {
        on_days(&build_sheet(std::slice::from_ref(staff), Group::City, 2026, 12, &Settings::default()))
    }

    #[test]
    fn a_move_shifts_the_cycle_from_its_day_on_and_the_cycle_goes_on_by_itself() {
        assert_eq!(december(&mover(&[]))[..8], [1, 2, 5, 6, 9, 10, 13, 14]);
        // From the 10th everything is one day later: the 10th shows what the 9th had, the 11th what the 10th had…
        assert_eq!(december(&mover(&[(10, 1)]))[..8], [1, 2, 5, 6, 9, 10, 11, 14], "before the 10th as planned, after it the cycle is one day on");
        assert_eq!(december(&mover(&[(10, -1)]))[..8], [1, 2, 5, 6, 9, 12, 13, 16], "one day earlier");
        // The moves of a person add up, each one from its own day.
        assert_eq!(mover(&[(10, 1), (20, 1)]).offset_on(date(2026, 12, 25)), 2);
        assert_eq!(mover(&[(10, 1), (20, -3)]).offset_on(date(2026, 12, 15)), 1);
        assert_eq!(mover(&[(10, 1), (20, -3)]).offset_on(date(2026, 12, 20)), -2);
        assert_eq!(mover(&[(10, 2)]).offset_on(date(2026, 12, 9)), 0, "nothing changes before the day of the move");
        // The next month goes on from the moved cycle by itself, with nobody typing anything.
        let january = build_sheet(&[mover(&[(10, 1)])], Group::City, 2027, 1, &Settings::default());
        assert_eq!(on_days(&january)[..6], [3, 4, 7, 8, 11, 12], "unmoved it is 2, 3, 6, 7, 10, 11: one day later");
    }

    #[test]
    fn a_move_changes_nothing_for_somebody_on_the_working_week() {
        let mut ana = member("Ana Ionescu", Group::Office, Plan::standard_weekdays());
        ana.moves = vec![ShiftMove { from: date(2026, 12, 7), days: 2 }];
        let settings = Settings::default();
        let moved = build_sheet(std::slice::from_ref(&ana), Group::Office, 2026, 12, &settings);
        let plain = build_sheet(&[member("Ana Ionescu", Group::Office, Plan::standard_weekdays())], Group::Office, 2026, 12, &settings);
        assert_eq!(hours(&moved, "ANA IONESCU"), hours(&plain, "ANA IONESCU"));
    }

    #[test]
    fn a_person_file_without_moves_is_read_and_the_moves_are_kept() {
        // A list saved by an older version has no `moves` on its people.
        let mut file = serde_json::to_value(member("Dragan Mirel", Group::City, cycle())).unwrap();
        assert!(file.as_object_mut().unwrap().remove("moves").is_some());
        let old: StaffMember = serde_json::from_value(file).unwrap();
        assert!(old.moves.is_empty());
        let with = mover(&[(10, 1)]);
        let back: StaffMember = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back, with);
    }

    #[test]
    fn a_saved_month_follows_the_moved_cycle_on_the_days_nobody_typed_on() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let before = mover(&[]);
        let settings = Settings { roster: vec![before.clone()], ..Settings::default() };
        // December and January are saved; the office typed 4 hours on the 14th (a shift day).
        let mut dec = open(&dir, Group::City, 2026, 12, &settings).unwrap();
        dec.employees[0].days[13].override_hours = Some(4.0);
        rules::recompute(&mut dec, &settings);
        save(&dir, Group::City, &dec).unwrap();
        save(&dir, Group::City, &open(&dir, Group::City, 2027, 1, &settings).unwrap()).unwrap();
        // The cycle is moved one day later from the 10th.
        let after = mover(&[(10, 1)]);
        let moved = Settings { roster: vec![after.clone()], ..settings.clone() };
        move_cycle(&dir, &before, date(2026, 12, 10), &moved).unwrap();
        let dec = open(&dir, Group::City, 2026, 12, &moved).unwrap();
        assert_eq!(on_days(&dec), [1, 2, 5, 6, 9, 10, 11, 14, 15, 18, 19, 22, 23, 26, 27, 30, 31], "before the 10th as it was, after it the new cycle");
        assert_eq!(dec.employees[0].days[13].hours, Some(4.0), "the 14th the office typed on stays as typed");
        assert_eq!(dec.employees[0].days[12].hours, None, "the 13th is no shift now");
        assert_eq!(on_days(&open(&dir, Group::City, 2027, 1, &moved).unwrap())[..6], [3, 4, 7, 8, 11, 12], "the next saved month too");
        // Taking the move back puts the cycle as it was (the typed day stays).
        move_cycle(&dir, &after, date(2026, 12, 10), &settings).unwrap();
        let back = open(&dir, Group::City, 2026, 12, &settings).unwrap();
        assert_eq!(on_days(&back)[..9], [1, 2, 5, 6, 9, 10, 13, 14, 17]);
        assert_eq!(back.employees[0].days[13].hours, Some(4.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_planned_leave_follows_the_moved_cycle_in_a_saved_month() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-move-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let before = mover(&[]);
        let settings = Settings { roster: vec![before.clone()], leaves: vec![leave("L1", "Dragan Mirel", Group::City, "CM", 12, 17)], ..Settings::default() };
        save(&dir, Group::City, &open(&dir, Group::City, 2026, 12, &settings).unwrap()).unwrap();
        assert_eq!(codes(&open(&dir, Group::City, 2026, 12, &settings).unwrap(), "DRAGAN MIREL").iter().map(|(d, _)| *d).collect::<Vec<_>>(), [13, 14, 17]);
        let moved = Settings { roster: vec![mover(&[(10, 1)])], ..settings.clone() };
        move_cycle(&dir, &before, date(2026, 12, 10), &moved).unwrap();
        let sheet = open(&dir, Group::City, 2026, 12, &moved).unwrap();
        assert_eq!(codes(&sheet, "DRAGAN MIREL").iter().map(|(d, _)| *d).collect::<Vec<_>>(), [14, 15], "the leave takes the shifts of the new cycle");
        assert_eq!(on_days(&sheet)[..8], [1, 2, 5, 6, 9, 10, 11, 18], "the days of leave are no worked hours");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn on_free(plan: LeavePlan) -> LeavePlan {
        LeavePlan { on_free_days: true, ..plan }
    }

    fn free_flags(sheet: &Timesheet, name: &str) -> Vec<u32> {
        sheet.employees.iter().find(|e| e.name == name).unwrap().days.iter().filter(|d| d.leave_on_free_day).map(|d| d.day).collect()
    }

    #[test]
    fn a_leave_for_the_free_days_too_takes_the_weekend_and_marks_those_days() {
        // 14 December 2026 is a Monday: Mon 14 – Sun 20.
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![on_free(leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 20))], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        let days: Vec<u32> = codes(&sheet, "ANA IONESCU").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [14, 15, 16, 17, 18, 19, 20], "Saturday and Sunday carry the leave too");
        assert_eq!(free_flags(&sheet, "ANA IONESCU"), [19, 20], "only the days she is free anyway");
        let ana = &sheet.employees[0];
        assert_eq!(ana.totals.code_hours[0], 40.0, "the five working days are in the CO column; the weekend is not in the norm");
        assert_eq!(ana.totals.overtime, 0.0);
        // The same leave without the option stays on the working days.
        let plain = Settings { leaves: vec![leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 20)], ..settings.clone() };
        assert_eq!(codes(&build_sheet(&staff, Group::Office, 2026, 12, &plain), "ANA IONESCU").len(), 5);
    }

    #[test]
    fn a_whole_month_of_leave_gives_the_norm_and_no_overtime() {
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![on_free(leave("L1", "Ana Ionescu", Group::Office, "CM", 1, 31))], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        let ana = &sheet.employees[0];
        assert_eq!(codes(&sheet, "ANA IONESCU").len(), 31, "every day of the month shows the leave");
        assert_eq!(ana.totals.code_hours[1], f64::from(sheet.norm_hours), "the CM column holds the norm of the month, not 31 days");
        assert_eq!((ana.totals.overtime, ana.totals.worked), (0.0, 0.0));
    }

    #[test]
    fn on_a_cycle_a_leave_for_the_free_days_takes_the_days_off_too() {
        // On 1, 2, 5, 6, 9, 10, 13, 14…: the days off are 3, 4, 7, 8, 11, 12.
        let plan = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = vec![member("Dragan Mirel", Group::City, plan)];
        let settings = Settings { roster: staff.clone(), leaves: vec![on_free(leave("L1", "Dragan Mirel", Group::City, "CM", 4, 10))], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::City, 2026, 12, &settings);
        let days: Vec<u32> = codes(&sheet, "DRAGAN MIREL").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(free_flags(&sheet, "DRAGAN MIREL"), [4, 7, 8]);
        assert_eq!(sheet.employees[0].totals.code_hours[1], 48.0, "the four shifts of the leave count 12 hours each in the CM column");
    }

    #[test]
    fn a_leave_counts_the_hours_of_the_shift_it_took_the_place_of() {
        // A cycle of 12-hour shifts, a person of four hours a day and a person of eight.
        let cycle = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = vec![
            member("Dragan Mirel", Group::City, cycle),
            member("Ana Ionescu", Group::City, Plan::Weekdays { start: 480, end: 720 }),
            member("Dan Popa", Group::City, Plan::standard_weekdays()),
        ];
        let leaves = vec![
            leave("L1", "Dragan Mirel", Group::City, "CO", 9, 14),
            leave("L2", "Ana Ionescu", Group::City, "CO", 14, 18),
            leave("L3", "Dan Popa", Group::City, "CO", 14, 18),
        ];
        let settings = Settings { roster: staff.clone(), leaves, ..Settings::default() };
        let sheet = build_sheet(&staff, Group::City, 2026, 12, &settings);
        let totals = |name: &str| sheet.employees.iter().find(|e| e.name == name).unwrap().totals.clone();
        assert_eq!(totals("DRAGAN MIREL").code_hours[0], 48.0, "four shifts of 12 hours");
        assert_eq!(totals("ANA IONESCU").code_hours[0], 20.0, "five days of 4 hours");
        assert_eq!(totals("DAN POPA").code_hours[0], 40.0, "five days of 8 hours");
        // The plan does not change with the leave: the hours worked and the hours of leave give what the person would have worked.
        let dragan = totals("DRAGAN MIREL");
        assert_eq!(dragan.worked + dragan.code_hours[0], 192.0);
    }

    #[test]
    fn a_leave_taken_by_an_older_version_gets_the_times_of_its_shift_back_when_the_month_is_opened() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-old-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cycle = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let settings = Settings { roster: vec![member("Dragan Mirel", Group::City, cycle)], leaves: vec![leave("L1", "Dragan Mirel", Group::City, "CM", 9, 14)], ..Settings::default() };
        let mut sheet = open(&dir, Group::City, 2026, 12, &settings).unwrap();
        // The month as the older version saved it: the days of leave without the times of their shift.
        for day in sheet.employees[0].days.iter_mut().filter(|d| d.code.is_some()) {
            day.override_intervals = None;
        }
        rules::recompute(&mut sheet, &settings);
        assert_eq!(sheet.employees[0].totals.code_hours[1], 32.0, "counted as norm days");
        save(&dir, Group::City, &sheet).unwrap();
        let again = open(&dir, Group::City, 2026, 12, &settings).unwrap();
        assert_eq!(again.employees[0].totals.code_hours[1], 48.0, "now as shifts of 12 hours");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_whole_month_of_leave_on_a_cycle_is_the_hours_of_every_shift_and_no_ore_libere() {
        let cycle = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = vec![member("Dragan Mirel", Group::City, cycle)];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Dragan Mirel", Group::City, "CO", 1, 31)], ..Settings::default() };
        let t = build_sheet(&staff, Group::City, 2026, 12, &settings).employees[0].totals.clone();
        assert_eq!((t.worked, t.code_hours[0], t.overtime), (0.0, 192.0, 0.0), "sixteen shifts of 12 hours, no overtime from them");
        assert_eq!(t.code_hours[timesheet::CODES.iter().position(|(c, _)| *c == timesheet::OLP).unwrap()], 0.0, "nothing is left over as OLP");
    }

    #[test]
    fn a_leave_is_not_an_overtime_and_not_an_absence() {
        // Somebody on 12-hour shifts worked 144 hours: under the norm of 168, so no overtime, whatever the leave adds.
        let cycle = Plan::Rotation { anchor: date(2026, 12, 1), on: vec![Interval { start: 540, end: 1260 }, Interval { start: 540, end: 1260 }], off: 2 };
        let staff = vec![member("Dragan Mirel", Group::City, cycle)];
        let settings = Settings { roster: staff.clone(), leaves: vec![leave("L1", "Dragan Mirel", Group::City, "CM", 9, 14)], ..Settings::default() };
        let t = build_sheet(&staff, Group::City, 2026, 12, &settings).employees[0].totals.clone();
        assert_eq!((t.worked, t.overtime), (144.0, 0.0));
        // An absence that is not a leave (N) still counts toward the norm, as before.
        let mut sheet = build_sheet(&staff, Group::City, 2026, 12, &Settings { roster: staff.clone(), ..Settings::default() });
        assert_eq!(sheet.employees[0].totals.overtime, 24.0, "without any leave the shifts of the plan are 24 hours over the norm");
        sheet.employees[0].days[0].code = Some("N".into());
        rules::recompute(&mut sheet, &settings);
        assert_eq!(sheet.employees[0].totals.overtime, 24.0, "the N day counts its 12 hours of shift toward the norm, as before: 180 + 12 - 168");
    }

    #[test]
    fn the_mark_of_a_free_day_goes_when_the_code_goes() {
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![on_free(leave("L1", "Ana Ionescu", Group::Office, "CO", 19, 20))], ..Settings::default() };
        let mut sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        assert_eq!(free_flags(&sheet, "ANA IONESCU"), [19, 20]);
        sheet.employees[0].days[18].code = None;
        sheet.employees[0].days[19].code = Some("cm".into());
        rules::recompute(&mut sheet, &settings);
        assert_eq!(free_flags(&sheet, "ANA IONESCU"), [20], "the cleared day is free of the mark; the day changed to another leave keeps it");
        assert_eq!((sheet.employees[0].totals.code_hours[0], sheet.employees[0].totals.code_hours[1]), (0.0, 0.0));
    }

    #[test]
    fn taking_a_leave_for_the_free_days_back_gives_the_plan_of_every_day_back() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-free-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let plan = on_free(leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 20));
        let settings = Settings { roster: staff, leaves: vec![plan.clone()], ..Settings::default() };
        save(&dir, Group::Office, &open(&dir, Group::Office, 2026, 12, &settings).unwrap()).unwrap();
        strip_plan(&dir, &plan, &Settings { leaves: Vec::new(), ..settings.clone() }).unwrap();
        let sheet = open(&dir, Group::Office, 2026, 12, &Settings { leaves: Vec::new(), ..settings }).unwrap();
        assert!(codes(&sheet, "ANA IONESCU").is_empty() && free_flags(&sheet, "ANA IONESCU").is_empty());
        assert_eq!(sheet.employees[0].days[13].hours, Some(8.0), "Monday is her shift again");
        assert_eq!(sheet.employees[0].days[18].hours, None, "Saturday is free again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_excel_leaves_the_free_days_out_of_the_hours_of_the_code() {
        use calamine::{open_workbook, Data, Reader, Xlsx};
        let staff = vec![member("Ana Ionescu", Group::Office, Plan::standard_weekdays())];
        let settings = Settings { roster: staff.clone(), leaves: vec![on_free(leave("L1", "Ana Ionescu", Group::Office, "CO", 14, 20))], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        let parts = crate::combined::Parts { year: 2026, month: 12, tachograph: None, catb: None, staff: sheet.employees };
        let path = std::env::temp_dir().join(format!("pontaj-roster-free-leave-{}.xlsx", std::process::id()));
        crate::combined::write_xlsx(&parts, &settings, &path).unwrap();
        let mut book: Xlsx<_> = open_workbook(&path).unwrap();
        let first = book.sheet_names()[0].clone();
        let formulas = book.worksheet_formula(&first).unwrap();
        let values = book.worksheet_range(&first).unwrap();
        let start = formulas.start().expect("formulas");
        let (r, c, formula) = formulas.cells().find(|(_, _, f)| f.contains("COUNTIF") && f.contains("\"CO\"")).expect("the CO formula");
        assert_eq!(formula.matches("-8*COUNTIF(").count(), 2, "the two weekend cells are taken out: {formula}");
        assert_eq!(values.get_value((start.0 + r as u32, start.1 + c as u32)), Some(&Data::Float(40.0)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_person_with_no_plan_has_zero_hours_and_the_office_writes_them_by_hand() {
        let ramona = member("Ertutk Ramona", Group::Office, Plan::Manual);
        let settings = Settings::default();
        let sheet = build_sheet(&[ramona.clone()], Group::Office, 2026, 12, &settings);
        assert_eq!(sheet.employees[0].totals.worked, 0.0);
        assert!(sheet.employees[0].days.iter().all(|day| day.hours.is_none() && day.intervals.is_empty()));
        // The plan is read from and written to the settings as `{ "kind": "manual" }`.
        assert_eq!(serde_json::to_value(&ramona.plan).unwrap(), serde_json::json!({ "kind": "manual" }));
        let mut typed = sheet;
        typed.employees[0].days[2].override_hours = Some(6.0);
        rules::recompute(&mut typed, &settings);
        assert_eq!(typed.employees[0].days[2].hours, Some(6.0), "hours typed by hand count like for anybody");
        assert_eq!(typed.employees[0].totals.worked, 6.0);
    }

    #[test]
    fn saved_months_follow_a_plan_that_changed_on_the_days_nobody_typed_on() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let before = vec![member("Ertutk Ramona", Group::Office, Plan::Weekdays { start: 480, end: 720 })];
        let old = Settings { roster: before.clone(), ..Settings::default() };
        let mut november = open(&dir, Group::Office, 2026, 11, &old).unwrap();
        assert_eq!(november.employees[0].days[1].hours, Some(4.0));
        november.employees[0].days[2].override_hours = Some(6.0); // typed by the office on the 3rd
        rules::recompute(&mut november, &old);
        save(&dir, Group::Office, &november).unwrap();
        let now = Settings { roster: vec![member("Ertutk Ramona", Group::Office, Plan::Manual)], ..Settings::default() };
        follow_plans(&dir, &before, &now).unwrap();
        let again = open(&dir, Group::Office, 2026, 11, &now).unwrap();
        let hours: Vec<Option<f64>> = again.employees[0].days.iter().map(|day| day.hours).collect();
        assert_eq!(hours[1], None, "the 2nd was the plan's and is zero now");
        assert_eq!(hours[2], Some(6.0), "the 3rd the office typed on stays");
        assert_eq!(again.employees[0].totals.worked, 6.0);
        // Nothing changed for a list that did not change: no month is written.
        let stamp = std::fs::metadata(dir.join("birou").join("2026-11.json")).unwrap().modified().unwrap();
        follow_plans(&dir, &now.roster, &now).unwrap();
        assert_eq!(std::fs::metadata(dir.join("birou").join("2026-11.json")).unwrap().modified().unwrap(), stamp);
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
