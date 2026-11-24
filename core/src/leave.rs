//! Planned leaves ("concedii în avans"): a person, a kind of leave and a range of days, written down before the days come.
//!
//! The list lives in the settings ([`crate::Settings::leaves`]), so it holds for every month, past and future, and for every
//! mode that has no tachograph: Categoria B ([`crate::catb`]) and the office, garage and city staff ([`crate::roster`]).
//! A month that is opened (or downloaded) takes the plans that touch it and writes each leave day into the person's own data
//! (a leave of a Categoria B driver, the code on a day of the staff). Every person remembers which plans were put on his
//! days (`applied_leaves`), so a plan is applied once: what the office changes afterwards on those days stays changed, and a
//! day cleared by hand does not come back at the next opening.
//!
//! What the plans mean for each mode is in [`crate::catb`] (a trip that arrives for a day of leave is reported and the office
//! says whether he worked) and [`crate::roster`] (only the days the person would work take the leave).

use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::catb::LEAVE_TYPES;
use crate::settings::Settings;
use crate::timesheet::{display_name, Group};
use crate::{catb, roster, Result};

/// A leave written down in advance for one person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LeavePlan {
    /// Unique, made by the screen when the plan is added; the people remember by it which plans they took.
    pub id: String,
    /// Where the person is: [`Group::CatB`], [`Group::Office`], [`Group::Garage`] or [`Group::City`].
    pub group: Group,
    /// The person's name as the list shows it (the staff list of the settings, or the name the API gives the driver).
    pub name: String,
    /// The API's id of a Categoria B driver (more reliable than his name); `None` for the staff.
    pub driver_id: Option<i64>,
    /// The kind of leave ([`LEAVE_TYPES`]): CO, CM, CP, CFP, CC, CIM.
    pub code: String,
    /// First and last day of the leave, both included.
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl Default for LeavePlan {
    fn default() -> Self {
        Self { id: String::new(), group: Group::CatB, name: String::new(), driver_id: None, code: "CO".into(), from: NaiveDate::default(), to: NaiveDate::default() }
    }
}

impl LeavePlan {
    /// Is the kind of leave one the pontaj knows?
    pub fn has_known_code(&self) -> bool {
        LEAVE_TYPES.iter().any(|(code, _)| *code == self.code)
    }

    /// The days of the plan that fall in the month, in order (empty when it does not touch the month).
    pub fn days_in(&self, year: i32, month: u32) -> Vec<NaiveDate> {
        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else { return Vec::new() };
        let last = first.with_day(crate::calendar::days_in_month(year, month)).unwrap_or(first);
        let (from, to) = (self.from.max(first), self.to.min(last));
        from.iter_days().take_while(|day| *day <= to).collect()
    }

    /// Is the plan for this person of the group? A Categoria B driver is found by his API id when the plan has one, else by
    /// name; the staff by name, the way the pontaj writes it.
    pub fn is_for(&self, group: Group, name: &str, driver_id: Option<i64>) -> bool {
        if self.group != group {
            return false;
        }
        match (self.driver_id, driver_id) {
            (Some(planned), Some(id)) => planned == id,
            _ => display_name(&self.name, true) == display_name(name, true),
        }
    }
}

/// Takes a plan back out of what is saved: the days it put a leave on go back to what they were before (a day of a Categoria B
/// driver is free again, a day of the staff follows the person's plan again). Only the days that still carry the plan's kind
/// of leave change, so what the office did after the plan was applied stays. Months that were never saved hold nothing of the
/// plan (it is applied in memory when they are opened), and the saved ones are written only when they changed.
pub fn strip_saved(plan: &LeavePlan, roster_dir: &Path, catb_dir: &Path, settings: &Settings) -> Result<()> {
    match plan.group {
        Group::CatB => catb::strip_plan(catb_dir, plan),
        _ => roster::strip_plan(roster_dir, plan, settings),
    }
}

/// Does a saved month `(year, month)` touch the plan?
pub fn touches(plan: &LeavePlan, year: i32, month: u32) -> bool {
    !plan.days_in(year, month).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn plan(from: NaiveDate, to: NaiveDate) -> LeavePlan {
        LeavePlan { id: "L1".into(), name: "Popescu Ion".into(), from, to, ..Default::default() }
    }

    #[test]
    fn a_plan_gives_its_days_in_each_month_it_touches() {
        let p = plan(date(2026, 12, 28), date(2027, 1, 4));
        assert_eq!(p.days_in(2026, 12).iter().map(|d| d.day()).collect::<Vec<_>>(), [28, 29, 30, 31]);
        assert_eq!(p.days_in(2027, 1).iter().map(|d| d.day()).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(p.days_in(2026, 11).is_empty() && p.days_in(2027, 2).is_empty());
        assert!(touches(&p, 2027, 1) && !touches(&p, 2026, 11));
        assert_eq!(plan(date(2026, 12, 3), date(2026, 12, 3)).days_in(2026, 12), [date(2026, 12, 3)], "one day is a plan too");
        assert!(plan(date(2026, 12, 9), date(2026, 12, 3)).days_in(2026, 12).is_empty(), "a range that ends before it starts has no days");
    }

    #[test]
    fn a_plan_finds_its_person_by_the_api_id_or_by_the_name_the_pontaj_writes() {
        let mut p = plan(date(2026, 12, 1), date(2026, 12, 2));
        assert!(p.is_for(Group::CatB, "POPESCU ION", Some(7)), "no id on the plan: the name decides");
        assert!(p.is_for(Group::CatB, "popescu  ion", None));
        assert!(!p.is_for(Group::Office, "POPESCU ION", None), "another group is another person");
        p.driver_id = Some(7);
        assert!(p.is_for(Group::CatB, "Altă Persoană", Some(7)), "the id wins over a name written differently");
        assert!(!p.is_for(Group::CatB, "POPESCU ION", Some(8)));
    }

    #[test]
    fn a_plan_is_read_from_the_settings_file_and_a_file_without_plans_has_none() {
        let json = r#"{ "leaves": [{ "id": "L1", "group": "catB", "name": "POPESCU ION", "driverId": 7, "code": "CM", "from": "2026-12-14", "to": "2026-12-18" }] }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.leaves.len(), 1);
        assert_eq!((settings.leaves[0].group, settings.leaves[0].driver_id, settings.leaves[0].code.as_str()), (Group::CatB, Some(7), "CM"));
        assert!(settings.leaves[0].has_known_code());
        let old: Settings = serde_json::from_str(r#"{ "companyName": "EXEMPLU SRL" }"#).unwrap();
        assert!(old.leaves.is_empty());
        let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.leaves, settings.leaves);
    }
}
