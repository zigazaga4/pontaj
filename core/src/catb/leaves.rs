//! Leave in Categoria B: the planned leaves ([`crate::leave`]) put on a driver's days, and the leave days on which a trip arrived.
//!
//! A driver's leave is a day without any trip that the office marked ([`Driver::leaves`]). A plan written in advance puts the
//! same mark on the working days of its range when the month is opened or downloaded. If the API then brings a trip for such a
//! day (the driver was on leave and drove anyway, or the trip was given to him by mistake), the day is a [`LeaveConflict`]:
//! the office is told and says whether he worked that day or not, see `resolveLeaveConflict` in the screen.

use std::path::Path;

use super::compute::{is_leave_code, trip_days};
use super::model::{Driver, LeaveConflict};
use super::store;
use crate::calendar;
use crate::leave::{touches, LeavePlan};
use crate::timesheet::Group;
use crate::Result;

/// Puts the planned leaves that touch the month on the working days (Monday–Friday, no legal holiday) of the driver they are for,
/// and on the weekends and holidays too when the plan is for the free days as well ([`LeavePlan::on_free_days`]).
/// Each plan is applied once per driver and month: a day that already has a leave keeps it, and one the office cleared later
/// is not marked again.
pub(super) fn apply(driver: &mut Driver, year: i32, month: u32, plans: &[LeavePlan]) {
    for plan in plans.iter().filter(|p| p.group == Group::CatB && p.has_known_code() && p.is_for(Group::CatB, &driver.name, Some(driver.id))) {
        let days = plan.days_in(year, month);
        if days.is_empty() || driver.applied_leaves.contains(&plan.id) {
            continue;
        }
        for day in days.into_iter().filter(|d| plan.on_free_days || (!calendar::is_weekend(*d) && calendar::holiday_name(*d).is_none())) {
            driver.leaves.entry(day).or_insert_with(|| plan.code.clone());
        }
        driver.applied_leaves.push(plan.id.clone());
    }
}

/// The leave days of the month on which the driver has a trip that counts. A day the office took out of the pontaj counts
/// nowhere (neither the trips nor the leave), so it is not asked about.
pub(super) fn conflicts(driver: &Driver, year: i32, month: u32) -> Vec<LeaveConflict> {
    use chrono::Datelike;
    driver
        .leaves
        .iter()
        .filter(|(date, code)| (date.year(), date.month()) == (year, month) && is_leave_code(code) && !driver.excluded_days.contains(date) && !driver.free_days.contains(date))
        .filter_map(|(date, code)| {
            let trips: Vec<String> = driver.trips.iter().filter(|t| !t.edit.removed && trip_days(t).contains(date)).map(|t| t.id.clone()).collect();
            (!trips.is_empty()).then(|| LeaveConflict { date: Some(*date), code: code.clone(), trips })
        })
        .collect()
}

/// Takes a plan back out of the saved Categoria B months: the days still marked with the plan's kind of leave are free again.
/// The months are written only when something changed; nothing is recomputed here (a month is recomputed whenever it is opened).
pub fn strip_plan(dir: &Path, plan: &LeavePlan) -> Result<()> {
    for (year, month) in store::list(dir) {
        if !touches(plan, year, month) {
            continue;
        }
        let Some(mut sheet) = store::load(dir, year, month)? else { continue };
        let mut changed = false;
        for driver in sheet.drivers.iter_mut().chain(sheet.removed_drivers.iter_mut()).filter(|d| plan.is_for(Group::CatB, &d.name, Some(d.id))) {
            if !driver.applied_leaves.contains(&plan.id) {
                continue;
            }
            for day in plan.days_in(year, month) {
                if driver.leaves.get(&day) == Some(&plan.code) {
                    driver.leaves.remove(&day);
                }
            }
            driver.applied_leaves.retain(|id| *id != plan.id);
            changed = true;
        }
        if changed {
            store::save(dir, &sheet)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::super::model::{Sheet, Trip, TripKind};
    use super::super::{api::parse_time, new_sheet};
    use super::*;
    use crate::catb::recompute;
    use crate::settings::Settings;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 12, day).unwrap()
    }

    fn trip(id: &str, from: &str, to: &str, departure: &str, arrival: &str) -> Trip {
        Trip {
            id: id.into(),
            kind: TripKind::Eco,
            from: from.into(),
            to: to.into(),
            departure: parse_time(departure).unwrap(),
            arrival: parse_time(arrival),
            ..Default::default()
        }
    }

    fn plan(id: &str, from: u32, to: u32, code: &str) -> LeavePlan {
        LeavePlan { id: id.into(), group: Group::CatB, name: "Popescu Ion".into(), driver_id: Some(7), code: code.into(), from: date(from), to: date(to), on_free_days: false }
    }

    fn december(plans: Vec<LeavePlan>, trips: Vec<Trip>) -> (Sheet, Settings) {
        let settings = Settings { leaves: plans, ..Settings::default() };
        let mut sheet = new_sheet(2026, 12);
        sheet.drivers.push(Driver { id: 7, name: "POPESCU ION".into(), trips, ..Default::default() });
        recompute(&mut sheet, &settings);
        (sheet, settings)
    }

    fn marked(driver: &Driver) -> Vec<u32> {
        use chrono::Datelike;
        driver.days.iter().filter(|d| d.leave.is_some()).filter_map(|d| d.date).map(|d| d.day()).collect()
    }

    #[test]
    fn a_planned_leave_marks_the_working_days_of_its_range_and_counts_them() {
        // 14 December 2026 is a Monday: Mon 14 – Sun 20 is five working days and a weekend.
        let (sheet, _) = december(vec![plan("L1", 14, 20, "CO")], vec![]);
        let d = &sheet.drivers[0];
        assert_eq!(marked(d), [14, 15, 16, 17, 18], "no leave on Saturday and Sunday");
        assert_eq!(d.totals.leave_days.get("CO"), Some(&5));
        assert_eq!(d.applied_leaves, ["L1"]);
        assert!(d.leave_conflicts.is_empty() && d.alerts.iter().all(|a| a.code != "leave_trip"));
    }

    #[test]
    fn a_plan_for_the_free_days_too_marks_weekends_and_holidays_and_counts_them() {
        // 14 December 2026 is a Monday.
        let free = |plan: LeavePlan| LeavePlan { on_free_days: true, ..plan };
        let (sheet, _) = december(vec![free(plan("L1", 14, 20, "CM"))], vec![]);
        let d = &sheet.drivers[0];
        assert_eq!(marked(d), [14, 15, 16, 17, 18, 19, 20], "Saturday and Sunday too");
        assert_eq!(d.totals.leave_days.get("CM"), Some(&7));
        // 25 and 26 December are holidays; 27 is a Sunday.
        let (sheet, _) = december(vec![free(plan("L2", 24, 28, "CO"))], vec![]);
        assert_eq!(marked(&sheet.drivers[0]), [24, 25, 26, 27, 28], "the holidays too");
    }

    #[test]
    fn the_plan_skips_legal_holidays_and_waits_for_the_month_it_belongs_to() {
        // 25 and 26 December are holidays; 24 (Thursday) and 28 (Monday) are working days.
        let (sheet, _) = december(vec![plan("L1", 24, 28, "CM")], vec![]);
        assert_eq!(marked(&sheet.drivers[0]), [24, 28]);
        let settings = Settings { leaves: vec![plan("L1", 24, 28, "CM")], ..Settings::default() };
        let mut november = new_sheet(2026, 11);
        november.drivers.push(Driver { id: 7, name: "POPESCU ION".into(), ..Default::default() });
        recompute(&mut november, &settings);
        assert!(november.drivers[0].applied_leaves.is_empty() && november.drivers[0].leaves.is_empty(), "a plan of December does not touch November");
    }

    #[test]
    fn a_plan_is_applied_once_so_a_leave_the_office_took_off_does_not_come_back() {
        let (mut sheet, settings) = december(vec![plan("L1", 14, 18, "CO")], vec![]);
        sheet.drivers[0].leaves.remove(&date(16));
        recompute(&mut sheet, &settings);
        assert_eq!(marked(&sheet.drivers[0]), [14, 15, 17, 18], "the 16th stays free");
    }

    #[test]
    fn a_plan_does_not_overwrite_a_leave_the_office_chose_and_follows_a_driver_who_comes_later() {
        let settings = Settings { leaves: vec![plan("L1", 14, 15, "CO")], ..Settings::default() };
        let mut sheet = new_sheet(2026, 12);
        sheet.drivers.push(Driver { id: 7, name: "POPESCU ION".into(), leaves: [(date(14), "CM".to_string())].into(), ..Default::default() });
        recompute(&mut sheet, &settings);
        let d = &sheet.drivers[0];
        assert_eq!((d.leaves.get(&date(14)).map(String::as_str), d.leaves.get(&date(15)).map(String::as_str)), (Some("CM"), Some("CO")));
        // Another driver is not touched; one that appears at a later download is marked when he arrives.
        sheet.drivers.push(Driver { id: 9, name: "ALT SOFER".into(), ..Default::default() });
        recompute(&mut sheet, &settings);
        assert!(sheet.drivers.iter().find(|d| d.id == 9).unwrap().leaves.is_empty());
    }

    #[test]
    fn a_trip_on_a_day_of_leave_is_reported_with_the_trips_that_take_the_day() {
        let trips = vec![trip("eco-1", "Constanta", "Aeroport Otopeni Bucuresti", "2026-12-16 08:00", "2026-12-16 11:00"), trip("eco-2", "Aeroport Otopeni Bucuresti", "Constanta", "2026-12-16 15:00", "2026-12-16 18:00")];
        let (sheet, _) = december(vec![plan("L1", 14, 18, "CO")], trips);
        let d = &sheet.drivers[0];
        assert_eq!(marked(d), [14, 15, 17, 18], "the 16th does not count as leave while it has trips");
        assert_eq!(d.leave_conflicts.len(), 1);
        assert_eq!((d.leave_conflicts[0].date, d.leave_conflicts[0].code.as_str(), d.leave_conflicts[0].trips.as_slice()), (Some(date(16)), "CO", ["eco-1".to_string(), "eco-2".to_string()].as_slice()));
        let alert = d.alerts.iter().find(|a| a.code == "leave_trip").expect("the office is told");
        assert!(alert.text.contains("Vezi că am primit cursă") && alert.text.contains("16.12"), "{}", alert.text);
    }

    #[test]
    fn he_worked_that_day_the_leave_goes_and_the_trips_count() {
        let trips = vec![trip("eco-1", "Constanta", "Aeroport Otopeni Bucuresti", "2026-12-16 08:00", "2026-12-16 11:00")];
        let (mut sheet, settings) = december(vec![plan("L1", 14, 18, "CO")], trips);
        sheet.drivers[0].leaves.remove(&date(16)); // what "a lucrat" does
        recompute(&mut sheet, &settings);
        let d = &sheet.drivers[0];
        assert!(d.leave_conflicts.is_empty() && d.alerts.iter().all(|a| a.code != "leave_trip"));
        assert_eq!(marked(d), [14, 15, 17, 18]);
        assert!(d.days.iter().any(|row| row.date == Some(date(16)) && row.total_min > 0), "the day has the hours of its trip");
        assert_eq!(d.totals.leave_days.get("CO"), Some(&4));
    }

    #[test]
    fn he_did_not_work_that_day_the_trips_are_taken_out_and_the_leave_counts() {
        let trips = vec![trip("eco-1", "Constanta", "Aeroport Otopeni Bucuresti", "2026-12-16 08:00", "2026-12-16 11:00")];
        let (mut sheet, settings) = december(vec![plan("L1", 14, 18, "CO")], trips);
        for trip in &mut sheet.drivers[0].trips {
            trip.edit.removed = true; // what "n-a lucrat" does
        }
        recompute(&mut sheet, &settings);
        let d = &sheet.drivers[0];
        assert!(d.leave_conflicts.is_empty());
        assert_eq!(marked(d), [14, 15, 16, 17, 18]);
        assert_eq!(d.totals.leave_days.get("CO"), Some(&5));
        assert_eq!(d.totals.total_min, 0, "the removed trip adds no hours");
    }

    #[test]
    fn a_day_taken_out_of_the_pontaj_is_not_asked_about() {
        let trips = vec![trip("eco-1", "Constanta", "Aeroport Otopeni Bucuresti", "2026-12-16 08:00", "2026-12-16 11:00")];
        let (mut sheet, settings) = december(vec![plan("L1", 14, 18, "CO")], trips);
        sheet.drivers[0].excluded_days.push(date(16));
        recompute(&mut sheet, &settings);
        assert!(sheet.drivers[0].leave_conflicts.is_empty() && sheet.drivers[0].alerts.iter().all(|a| a.code != "leave_trip"));
    }

    #[test]
    fn taking_a_plan_back_clears_its_days_in_the_saved_month_and_only_those() {
        let dir = std::env::temp_dir().join(format!("pontaj-leaves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut sheet, _) = december(vec![plan("L1", 14, 18, "CO")], vec![]);
        sheet.drivers[0].leaves.insert(date(22), "CM".into()); // chosen by hand, not by the plan
        sheet.drivers[0].leaves.insert(date(15), "CM".into()); // the office changed this day of the plan to another kind
        store::save(&dir, &sheet).unwrap();
        strip_plan(&dir, &plan("L1", 14, 18, "CO")).unwrap();
        let back = store::load(&dir, 2026, 12).unwrap().unwrap();
        let d = &back.drivers[0];
        assert_eq!(d.leaves.keys().map(|k| chrono::Datelike::day(k)).collect::<Vec<_>>(), [15, 22], "what the plan did is gone, what the office did stays");
        assert!(d.applied_leaves.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
