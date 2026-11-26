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

/// A period in which a person works other hours than his plan: on every day of the period (`from` and `to` included) that his plan has him
/// work, the shift starts at `start` and ends at `end` (minutes; an `end` above 24:00 is after midnight). The days he is free anyway stay free.
/// With `alternate` only every second week has the other hours: the first week (the 7 days from `from`), the third… and the weeks between
/// keep the hours of the plan ("one week from one hour, one week from the first hour"). If periods overlap, the one added last decides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HoursChange {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub start: i32,
    pub end: i32,
    pub alternate: bool,
}

impl Default for HoursChange {
    fn default() -> Self {
        Self { from: NaiveDate::default(), to: NaiveDate::default(), start: 0, end: 0, alternate: false }
    }
}

impl HoursChange {
    /// Are the other hours the ones of `date`? (Inside the period, and with `alternate` in the weeks 1, 3, 5… counted from `from`.)
    pub fn applies_on(&self, date: NaiveDate) -> bool {
        date >= self.from && date <= self.to && (!self.alternate || (date - self.from).num_days() / 7 % 2 == 0)
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
    /// The periods in which the person works other hours than his plan (see [`HoursChange`]); none for most people.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<HoursChange>,
    /// The first day the person works (a new hire). `None` = he has always worked. Before this day he is not in the pontaj.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on: Option<NaiveDate>,
    /// The first day the person no longer works (dismissed): the day before is his last one. `None` = he still works. From this day
    /// on he is not in the pontaj; the months before it stay as they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stops_on: Option<NaiveDate>,
}

impl StaffMember {
    /// Is the person employed on `date` (hired on or before it and not dismissed yet)?
    pub fn employed_on(&self, date: NaiveDate) -> bool {
        self.starts_on.is_none_or(|from| date >= from) && self.stops_on.is_none_or(|to| date < to)
    }

    /// Is the person employed on at least one day of the month? Only those are in the pontaj of the month.
    pub fn employed_in(&self, year: i32, month: u32) -> bool {
        (1..=calendar::days_in_month(year, month)).filter_map(|day| NaiveDate::from_ymd_opt(year, month, day)).any(|date| self.employed_on(date))
    }

    /// The norm of the month for this person when he is employed only a part of it: the working days of the calendar he is employed on
    /// × the hours of a day. `None` when he is there the whole month (the norm of the month is his).
    pub fn part_time_norm(&self, year: i32, month: u32, hours_per_day: i32) -> Option<i32> {
        let working = |employed_only: bool| {
            (1..=calendar::days_in_month(year, month))
                .filter_map(|day| NaiveDate::from_ymd_opt(year, month, day))
                .filter(|date| !calendar::is_weekend(*date) && calendar::holiday_name(*date).is_none())
                .filter(|date| !employed_only || self.employed_on(*date))
                .count() as i32
        };
        let (all, own) = (working(false), working(true));
        (own != all).then_some(own * hours_per_day)
    }

    /// By how many days the cycle is moved on `date`: the moves that started on or before it, added up.
    pub fn offset_on(&self, date: NaiveDate) -> i64 {
        self.moves.iter().filter(|m| m.from <= date).map(|m| i64::from(m.days)).sum()
    }

    /// Is the day in the norm of the person (a day he works, or for somebody who types everything by hand a working day of the
    /// calendar)? A leave on any other day is a leave on a free day.
    pub fn works_on(&self, date: NaiveDate) -> bool {
        if !self.employed_on(date) {
            return false;
        }
        match self.plan {
            Plan::Manual => !calendar::is_weekend(date) && calendar::holiday_name(date).is_none(),
            Plan::Weekdays { .. } | Plan::Rotation { .. } => self.shift_on(date).is_some(),
        }
    }

    /// The times of the day's shift, or `None` when the person is off that day: the plan, with the moves of the cycle and the periods of other hours taken into account.
    pub fn shift_on(&self, date: NaiveDate) -> Option<Interval> {
        if !self.employed_on(date) {
            return None;
        }
        let planned = match self.plan {
            Plan::Rotation { .. } => self.plan.shift_on(date - Duration::days(self.offset_on(date))),
            Plan::Weekdays { .. } | Plan::Manual => self.plan.shift_on(date),
        };
        // A day he works in a period with other hours has those hours; a day he is free stays free.
        planned.map(|shift| self.changes.iter().rev().find(|change| change.applies_on(date)).map_or(shift, |change| Interval { start: change.start, end: change.end }))
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
            let date = NaiveDate::from_ymd_opt(year, month, day);
            let shift = date.and_then(|date| member.shift_on(date));
            DayEntry { day, override_intervals: shift.map(|iv| vec![iv]), not_employed: date.is_some_and(|date| !member.employed_on(date)), ..Default::default() }
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
        for date in days.into_iter().filter(|date| member.employed_on(*date) && (plan.on_free_days || member.works_on(*date))) {
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
    // Only the people employed on some day of the month are in it: the months before a hire and after a dismissal do not have them.
    for member in staff.iter().filter(|m| m.group == group && m.employed_in(year, month)) {
        let mut fresh = employee(member, year, month, settings.plain_names);
        fresh.norm_hours = member.part_time_norm(year, month, settings.hours_per_day);
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
            // The days the office left stay, except the days the person is not employed on (they are empty) and the days he just became
            // employed on (they take his plan).
            for (day, saved_day) in employee.days.iter_mut().zip(&old.days) {
                if !day.not_employed && !saved_day.not_employed {
                    *day = saved_day.clone();
                }
            }
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
        current.norm_hours = member.part_time_norm(sheet.year, sheet.month, settings.hours_per_day);
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
        let changed = settings.roster.iter().any(|now| now.group == old.group && key(&now.name) == key(&old.name) && (now.plan != old.plan || now.moves != old.moves || now.changes != old.changes));
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
        StaffMember { name: name.into(), function: "Secretara".into(), group, plan, note: String::new(), moves: Vec::new(), changes: Vec::new(), starts_on: None, stops_on: None }
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

    fn employed(name: &str, starts_on: Option<NaiveDate>, stops_on: Option<NaiveDate>) -> StaffMember {
        StaffMember { starts_on, stops_on, ..member(name, Group::Office, Plan::standard_weekdays()) }
    }

    fn not_employed_days(sheet: &Timesheet, name: &str) -> Vec<u32> {
        sheet.employees.iter().find(|e| e.name == name).unwrap().days.iter().filter(|d| d.not_employed).map(|d| d.day).collect()
    }

    fn names(sheet: &Timesheet) -> Vec<&str> {
        sheet.employees.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn a_person_hired_in_the_middle_of_the_month_has_nothing_before_and_the_norm_of_the_rest() {
        // 1 November 2026 is a Sunday; 16 is a Monday. 16–20 and 23–27 are working days, 30 November is a holiday.
        let staff = [employed("Nou Ion", Some(date(2026, 11, 16)), None)];
        let sheet = build_sheet(&staff, Group::Office, 2026, 11, &Settings::default());
        let nou = &sheet.employees[0];
        assert_eq!(not_employed_days(&sheet, "NOU ION"), (1..=15).collect::<Vec<_>>());
        assert!(nou.days[..15].iter().all(|d| d.hours.is_none() && d.code.is_none() && d.override_intervals.is_none()));
        assert_eq!(nou.days[15].hours, Some(8.0), "16 November is his first day");
        assert_eq!(nou.norm_hours, Some(80), "ten working days he is employed on, 8 hours each");
        assert_eq!((nou.totals.worked, nou.totals.not_worked, nou.totals.overtime), (80.0, 0.0, 0.0));
        // Somebody there the whole month has the norm of the month.
        let all = build_sheet(&[employed("Vechi Ion", None, None)], Group::Office, 2026, 11, &Settings::default());
        assert_eq!((all.employees[0].norm_hours, not_employed_days(&all, "VECHI ION")), (None, Vec::new()));
    }

    #[test]
    fn a_person_dismissed_in_the_middle_of_the_month_works_until_the_day_before() {
        // He stops on Tuesday the 10th: he worked 2–6 and the 9th, six days.
        let staff = [employed("Plecat Ion", None, Some(date(2026, 11, 10)))];
        let sheet = build_sheet(&staff, Group::Office, 2026, 11, &Settings::default());
        let plecat = &sheet.employees[0];
        assert_eq!(not_employed_days(&sheet, "PLECAT ION"), (10..=30).collect::<Vec<_>>());
        assert_eq!((plecat.days[8].hours, plecat.days[9].hours), (Some(8.0), None), "the 9th is his last day, the 10th is not worked");
        assert_eq!(plecat.norm_hours, Some(48));
        assert_eq!((plecat.totals.worked, plecat.totals.not_worked), (48.0, 0.0), "no hours missing: the days after the dismissal are not in his norm");
    }

    #[test]
    fn a_person_is_in_the_months_he_is_employed_in_and_in_no_other() {
        let staff = [
            employed("Nou", Some(date(2026, 12, 1)), None),
            employed("Plecat", None, Some(date(2026, 11, 1))),
            employed("O zi", Some(date(2026, 11, 30)), Some(date(2026, 12, 1))),
            employed("Mereu", None, None),
        ];
        let settings = Settings::default();
        let in_month = |year, month| build_sheet(&staff, Group::Office, year, month, &settings);
        assert_eq!(names(&in_month(2026, 10)), ["MEREU", "PLECAT"], "the month before the dismissal still has the person");
        assert_eq!(names(&in_month(2026, 11)), ["MEREU", "O ZI"], "the dismissal day is the first day he is not there: 1 November is not his");
        assert_eq!(names(&in_month(2026, 12)), ["MEREU", "NOU"]);
        assert_eq!(names(&in_month(2025, 1)), ["MEREU", "PLECAT"], "before the hire the person is not there; before the dismissal he is");
    }

    #[test]
    fn what_the_office_typed_on_days_he_is_not_employed_on_is_dropped_and_the_days_he_gets_back_follow_the_plan() {
        let settings = Settings::default();
        let working = [member("Ana Pop", Group::Office, Plan::standard_weekdays())];
        let mut saved = build_sheet(&working, Group::Office, 2026, 11, &settings);
        saved.employees[0].days[9].override_hours = Some(6.0); // the 10th
        saved.employees[0].days[11].override_hours = Some(5.0); // the 12th
        rules::recompute(&mut saved, &settings);
        // She is dismissed from the 11th: the 10th stays as typed, the 12th is empty.
        let left = [StaffMember { stops_on: Some(date(2026, 11, 11)), ..working[0].clone() }];
        let sheet = sync(Some(saved), &left, Group::Office, 2026, 11, &settings);
        let days = &sheet.employees[0].days;
        assert_eq!((days[9].hours, days[10].hours, days[11].hours), (Some(6.0), None, None));
        assert!(days[10].not_employed && days[11].not_employed && !days[9].not_employed);
        // The dismissal is taken back: the days come back from her plan (what was typed on them is gone with the dismissal).
        let back = sync(Some(sheet), &working, Group::Office, 2026, 11, &settings);
        let days = &back.employees[0].days;
        assert_eq!((days[9].hours, days[10].hours, days[11].hours), (Some(6.0), Some(8.0), Some(8.0)));
        assert!(days.iter().all(|d| !d.not_employed));
        assert_eq!(back.employees[0].norm_hours, None);
    }

    #[test]
    fn nothing_can_be_written_on_a_day_the_person_is_not_employed_on() {
        let settings = Settings::default();
        let staff = [employed("Nou Ion", Some(date(2026, 11, 16)), None)];
        let mut sheet = build_sheet(&staff, Group::Office, 2026, 11, &settings);
        sheet.employees[0].days[2].override_hours = Some(8.0);
        sheet.employees[0].days[3].code = Some("CO".into());
        rules::recompute(&mut sheet, &settings);
        let days = &sheet.employees[0].days;
        assert_eq!((days[2].hours, days[2].override_hours, days[3].code.clone()), (None, None, None));
        assert!(days[2].not_employed && days[3].not_employed);
        assert_eq!(sheet.employees[0].totals.worked, 80.0);
    }

    #[test]
    fn a_leave_stays_inside_the_time_the_person_is_employed_even_for_the_free_days_too() {
        // 14 December 2026 is a Monday; he works from the 14th to the 17th (the 18th is the first day he is not there).
        let staff = [employed("Ana Ionescu", Some(date(2026, 12, 14)), Some(date(2026, 12, 18)))];
        let settings = Settings { roster: staff.to_vec(), leaves: vec![on_free(leave("L1", "Ana Ionescu", Group::Office, "CO", 1, 31))], ..Settings::default() };
        let sheet = build_sheet(&staff, Group::Office, 2026, 12, &settings);
        let days: Vec<u32> = codes(&sheet, "ANA IONESCU").into_iter().map(|(day, _)| day).collect();
        assert_eq!(days, [14, 15, 16, 17]);
        assert_eq!(sheet.employees[0].totals.code_hours[0], 32.0, "four days of leave, and the norm is his four days");
        assert_eq!(sheet.employees[0].norm_hours, Some(32));
    }

    #[test]
    fn the_dates_of_a_person_are_read_and_written_and_a_person_without_dates_has_none_in_the_file() {
        let old: StaffMember = serde_json::from_str(r#"{"name":"A B","function":"","group":"office","plan":{"kind":"manual"}}"#).unwrap();
        assert_eq!((old.starts_on, old.stops_on), (None, None));
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("startsOn") && !json.contains("stopsOn"), "{json}");
        let dated: StaffMember = serde_json::from_str(r#"{"name":"A B","function":"","group":"office","plan":{"kind":"manual"},"startsOn":"2026-12-01","stopsOn":"2027-02-01"}"#).unwrap();
        assert_eq!((dated.starts_on, dated.stops_on), (Some(date(2026, 12, 1)), Some(date(2027, 2, 1))));
        assert!(dated.employed_on(date(2026, 12, 1)) && dated.employed_on(date(2027, 1, 31)));
        assert!(!dated.employed_on(date(2026, 11, 30)) && !dated.employed_on(date(2027, 2, 1)));
    }

    fn change(from: (u32, u32), to: (u32, u32), start: i32, end: i32, alternate: bool) -> HoursChange {
        HoursChange { from: date(2026, from.0, from.1), to: date(2026, to.0, to.1), start, end, alternate }
    }

    fn with_changes(changes: Vec<HoursChange>) -> StaffMember {
        StaffMember { changes, ..member("Ana Pop", Group::Office, Plan::standard_weekdays()) }
    }

    /// The start of the shift of every day of November 2026 ("08:00" for the days with 08:00–16:00, empty for a free day).
    fn starts(sheet: &Timesheet) -> Vec<String> {
        sheet.employees[0].days.iter().map(|d| d.intervals.first().map(|i| format!("{:02}:{:02}", i.start / 60, i.start % 60)).unwrap_or_default()).collect()
    }

    #[test]
    fn a_period_with_other_hours_changes_the_days_he_works_and_nothing_else() {
        // Monday 16 – Sunday 22 November: 12:00–20:00 (the weekend stays free); the weeks around keep 08:00–16:00.
        let staff = [with_changes(vec![change((11, 16), (11, 22), 720, 1200, false)])];
        let sheet = build_sheet(&staff, Group::Office, 2026, 11, &Settings::default());
        let at = starts(&sheet);
        assert_eq!(&at[8..13], ["08:00", "08:00", "08:00", "08:00", "08:00"], "9–13 November as in the plan");
        assert_eq!(&at[15..22], ["12:00", "12:00", "12:00", "12:00", "12:00", "", ""], "16–20 November from 12:00, the weekend free");
        assert_eq!(&at[22..27], ["08:00", "08:00", "08:00", "08:00", "08:00"], "23–27 November as in the plan");
        let days = &sheet.employees[0].days;
        assert_eq!((days[15].hours, days[15].intervals[0].end), (Some(8.0), 1200), "the hours follow the length of the shift");
        assert_eq!(sheet.employees[0].totals.worked, 160.0);
        // A period of 10:00–16:00 gives six hours a day.
        let shorter = [with_changes(vec![change((11, 2), (11, 6), 600, 960, false)])];
        let sheet = build_sheet(&shorter, Group::Office, 2026, 11, &Settings::default());
        assert_eq!(sheet.employees[0].days[1].hours, Some(6.0));
        assert_eq!(sheet.employees[0].totals.worked, 5.0 * 6.0 + 15.0 * 8.0);
    }

    #[test]
    fn one_week_from_one_hour_and_one_week_from_the_first_hour() {
        // From Monday 2 November to Sunday 29 November: weeks 2–8 and 16–22 from 12:00, the weeks 9–15 and 23–29 as in the plan.
        let staff = [with_changes(vec![change((11, 2), (11, 29), 720, 1200, true)])];
        let at = starts(&build_sheet(&staff, Group::Office, 2026, 11, &Settings::default()));
        let monday = |day: usize| at[day - 1].clone();
        assert_eq!((monday(2), monday(9), monday(16), monday(23)), ("12:00".into(), "08:00".into(), "12:00".into(), "08:00".into()));
        assert_eq!((at[5].as_str(), at[11].as_str()), ("12:00", "08:00"), "Friday 6 November is in the first week, Thursday 12 November in the second");
        // Not alternating, the same period has the other hours in all four weeks.
        let all = [with_changes(vec![change((11, 2), (11, 29), 720, 1200, false)])];
        let at = starts(&build_sheet(&all, Group::Office, 2026, 11, &Settings::default()));
        assert!([2, 9, 16, 23].iter().all(|day| at[day - 1] == "12:00"));
    }

    #[test]
    fn the_weeks_are_counted_from_the_first_day_of_the_period() {
        // Wednesday 4 – Tuesday 10 November is the first week, Wednesday 11 – Tuesday 17 the second.
        let staff = [with_changes(vec![change((11, 4), (11, 30), 720, 1200, true)])];
        let at = starts(&build_sheet(&staff, Group::Office, 2026, 11, &Settings::default()));
        assert_eq!((at[3].as_str(), at[9].as_str(), at[10].as_str(), at[16].as_str(), at[17].as_str()), ("12:00", "12:00", "08:00", "08:00", "12:00"));
        assert_eq!(at[2], "08:00", "the day before the period is as in the plan");
    }

    #[test]
    fn on_a_cycle_the_other_hours_are_on_the_shifts_and_the_free_days_stay_free() {
        // The cycle starts on 1 December 2026 with two shifts then two days off. 10:00–18:00 from the 8th to the 14th.
        let person = StaffMember { changes: vec![HoursChange { from: date(2026, 12, 8), to: date(2026, 12, 14), start: 600, end: 1080, alternate: false }], ..mover(&[]) };
        let sheet = build_sheet(std::slice::from_ref(&person), Group::City, 2026, 12, &Settings::default());
        let days = &sheet.employees[0].days;
        let shift = |day: usize| days[day - 1].intervals.first().map(|i| (i.start, i.end));
        assert_eq!(shift(5), Some((540, 1260)), "5 December as in the plan: 09:00–21:00");
        assert_eq!(shift(9), Some((600, 1080)), "9 December from 10:00 to 18:00");
        assert_eq!(days[8].hours, Some(8.0));
        assert_eq!(shift(7), None, "7 December is a free day of the cycle");
        assert_eq!((shift(13), shift(15), shift(17)), (Some((600, 1080)), None, Some((540, 1260))), "the 13th from 10:00, the 15th is free, the 17th as in the plan");
        assert_eq!(on_days(&sheet), on_days(&build_sheet(&[mover(&[])], Group::City, 2026, 12, &Settings::default())), "the same days work, only the hours of some differ");
    }

    #[test]
    fn the_period_added_last_decides_when_two_overlap_and_a_period_never_works_a_day_off() {
        let staff = [with_changes(vec![change((11, 2), (11, 13), 720, 1200, false), change((11, 9), (11, 13), 600, 1080, false)])];
        let at = starts(&build_sheet(&staff, Group::Office, 2026, 11, &Settings::default()));
        assert_eq!((at[1].as_str(), at[8].as_str(), at[12].as_str()), ("12:00", "10:00", "10:00"));
        assert_eq!((at[6].as_str(), at[7].as_str()), ("", ""), "Saturday 7 and Sunday 8 November stay free");
        // A period that begins and ends on days off puts nothing there.
        let weekend = [with_changes(vec![change((11, 7), (11, 8), 720, 1200, false)])];
        let sheet = build_sheet(&weekend, Group::Office, 2026, 11, &Settings::default());
        assert_eq!((sheet.employees[0].days[6].hours, sheet.employees[0].days[7].hours), (None, None));
    }

    #[test]
    fn a_saved_month_follows_the_periods_on_the_days_nobody_typed_on() {
        let dir = std::env::temp_dir().join(format!("pontaj-roster-hours-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let before = with_changes(Vec::new());
        let settings = Settings { roster: vec![before.clone()], ..Settings::default() };
        // November is saved; the office typed 4 hours on Tuesday the 17th.
        let mut november = open(&dir, Group::Office, 2026, 11, &settings).unwrap();
        november.employees[0].days[16].override_hours = Some(4.0);
        rules::recompute(&mut november, &settings);
        save(&dir, Group::Office, &november).unwrap();
        // 12:00–20:00 for the week of 16 November.
        let after = with_changes(vec![change((11, 16), (11, 22), 720, 1200, false)]);
        let changed = Settings { roster: vec![after.clone()], ..settings.clone() };
        move_cycle(&dir, &before, date(2026, 11, 16), &changed).unwrap();
        let sheet = open(&dir, Group::Office, 2026, 11, &changed).unwrap();
        let at = starts(&sheet);
        assert_eq!((at[12].as_str(), at[17].as_str(), at[22].as_str()), ("08:00", "12:00", "08:00"), "before the period as it was, in it the new hours, after it as it was");
        assert_eq!(sheet.employees[0].days[16].hours, Some(4.0), "the 17th the office typed on stays as typed");
        // Taking the period back puts the hours of the plan again.
        move_cycle(&dir, &after, date(2026, 11, 16), &settings).unwrap();
        let back = open(&dir, Group::Office, 2026, 11, &settings).unwrap();
        assert_eq!(starts(&back)[17], "08:00");
        assert_eq!(back.employees[0].days[16].hours, Some(4.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_periods_are_read_and_written_and_a_person_without_periods_has_none_in_the_file() {
        let none: StaffMember = serde_json::from_str(r#"{"name":"A B","function":"","group":"office","plan":{"kind":"manual"}}"#).unwrap();
        assert!(none.changes.is_empty());
        assert!(!serde_json::to_string(&none).unwrap().contains("changes"));
        let some: StaffMember = serde_json::from_str(r#"{"name":"A B","function":"","group":"office","plan":{"kind":"manual"},"changes":[{"from":"2026-11-02","to":"2026-11-29","start":720,"end":1200,"alternate":true}]}"#).unwrap();
        assert_eq!(some.changes, [change((11, 2), (11, 29), 720, 1200, true)]);
        // Weeks 1 and 3 apply, weeks 2 and 4 do not.
        assert!(some.changes[0].applies_on(date(2026, 11, 8)) && !some.changes[0].applies_on(date(2026, 11, 9)) && some.changes[0].applies_on(date(2026, 11, 16)));
        assert!(!some.changes[0].applies_on(date(2026, 12, 1)) && !some.changes[0].applies_on(date(2026, 11, 1)));
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
