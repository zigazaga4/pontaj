//! The people the app starts with: the office, garage and city staff of the payroll file of October 2026 (with the plan their
//! hours are inferred from, so November goes on from October by itself) and the truck drivers (the green names).
//!
//! The list is kept in `seed.json` (read once from that file when the app was made; the app never reads a payroll file). From
//! the first start the list lives in the settings, where the office adds and removes people.

use std::sync::OnceLock;

use serde::Deserialize;

use super::StaffMember;

#[derive(Debug, Clone, Deserialize)]
struct Seed {
    staff: Vec<StaffMember>,
    tir: Vec<String>,
}

fn seed() -> &'static Seed {
    static SEED: OnceLock<Seed> = OnceLock::new();
    SEED.get_or_init(|| serde_json::from_str(include_str!("seed.json")).expect("seed.json is the list written by the roster_seed example"))
}

/// The staff the app starts with.
pub fn staff() -> Vec<StaffMember> {
    seed().staff.clone()
}

/// The truck drivers the app starts with.
pub fn tir_drivers() -> Vec<String> {
    seed().tir.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timesheet::Group;

    #[test]
    fn the_list_the_app_starts_with_has_every_group_and_no_one_twice() {
        let staff = staff();
        for group in super::super::GROUPS {
            assert!(staff.iter().any(|m| m.group == group), "{group:?} has people");
        }
        let mut names: Vec<&str> = staff.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "a person is listed once");
        assert!(staff.iter().all(|m| m.group != Group::Tachograph && m.group != Group::Tir && m.group != Group::CatB));
        assert!(!tir_drivers().is_empty());
    }
}
