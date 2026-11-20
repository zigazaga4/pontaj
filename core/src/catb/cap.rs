//! Suggestions for a driver who is over the monthly ceiling (8 h × the Mon–Fri days, holidays included).
//!
//! Only suggestions: the office decides. The waiting between trips is not work time, so it is not in the hours and there is
//! nothing to shorten there: the only way to come under the ceiling is to take whole days out, weekend and holiday days
//! first.

use std::cmp::Reverse;

use chrono::{Datelike, NaiveDate};

use super::model::*;
use crate::calendar;
use crate::time::fmt_duration;

pub(super) fn propose(driver: &Driver, cap_min: i32) -> Vec<Proposal> {
    let excess = driver.totals.total_min - cap_min;
    if excess <= 0 {
        return Vec::new();
    }
    let mut remaining = excess;
    let mut proposals: Vec<Proposal> = Vec::new();

    // Whole days, while the ceiling is still exceeded.
    let mut taken: Vec<NaiveDate> = Vec::new();
    while remaining > 0 {
        let candidates: Vec<(&DayRow, i32)> = driver
            .days
            .iter()
            .filter(|d| !d.excluded && d.total_min > 0)
            .filter(|d| d.date.is_some_and(|date| !taken.contains(&date)))
            .map(|d| (d, d.total_min))
            .collect();
        let off = |day: &DayRow| day.date.is_some_and(is_off_day);
        // A day that covers what is left (the smallest one), else the one that removes the most.
        let covering = candidates.iter().filter(|(_, gain)| *gain >= remaining).min_by_key(|(day, gain)| (!off(day), *gain));
        let Some(&(day, gain)) = covering.or_else(|| candidates.iter().min_by_key(|(day, gain)| (!off(day), Reverse(*gain)))) else { break };
        let Some(date) = day.date else { break };

        remaining -= gain;
        taken.push(date);
        proposals.push(Proposal {
            date,
            minutes: day.total_min,
            pay: day.pay_total,
            text: format!("Scoate ziua {} din pontaj: −{} ore.", label(date), fmt_duration(day.total_min)),
        });
    }
    proposals.sort_by_key(|p| p.date);
    proposals
}

fn is_off_day(date: NaiveDate) -> bool {
    calendar::is_weekend(date) || calendar::holiday_name(date).is_some()
}

/// `12.10 (sâmbătă)` / `30.11 (luni, sărbătoare)`.
fn label(date: NaiveDate) -> String {
    let weekday = calendar::weekday_long(date);
    let note = match calendar::holiday_name(date) {
        Some(_) => format!("{weekday}, sărbătoare"),
        None => weekday.to_string(),
    };
    format!("{:02}.{:02} ({note})", date.day(), date.month())
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::super::compute::recompute_driver;
    use super::*;
    use crate::settings::Settings;

    /// The rules of these tests are about the ceiling alone: days are not filled up to 8 hours.
    fn plain() -> Settings {
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        settings
    }

    fn trip(id: String, from: &str, to: &str, departure: String, arrival: String) -> Trip {
        Trip { id, kind: TripKind::Eco, from: from.into(), to: to.into(), departure: parse_time(&departure).unwrap(), arrival: parse_time(&arrival), ..Default::default() }
    }

    /// `days` days of two legs; `gap_h` hours of waiting between them, legs of `leg_h` hours.
    fn month(days: std::ops::RangeInclusive<u32>, leg_h: u32, gap_h: u32) -> Driver {
        let mut trips = Vec::new();
        for d in days {
            let first_end = 5 + leg_h;
            let second_start = first_end + gap_h;
            trips.push(trip(format!("a{d}"), "Constanta", "Otopeni", format!("2026-10-{d:02} 05:00"), format!("2026-10-{d:02} {first_end:02}:00")));
            trips.push(trip(format!("b{d}"), "Otopeni", "Constanta", format!("2026-10-{d:02} {second_start:02}:00"), format!("2026-10-{d:02} {:02}:00", second_start + leg_h)));
        }
        let mut driver = Driver { id: 1, name: "TEST".into(), trips, ..Default::default() };
        recompute_driver(&mut driver, 2026, 10, &plain());
        driver
    }

    /// `days` days of two legs of 4½ h without waiting: 9 h a day.
    fn heavy_month(days: std::ops::RangeInclusive<u32>) -> Driver {
        let mut trips = Vec::new();
        for d in days {
            trips.push(trip(format!("a{d}"), "Constanta", "Otopeni", format!("2026-10-{d:02} 05:00"), format!("2026-10-{d:02} 09:30")));
            trips.push(trip(format!("b{d}"), "Otopeni", "Constanta", format!("2026-10-{d:02} 09:30"), format!("2026-10-{d:02} 14:00")));
        }
        let mut driver = Driver { id: 1, name: "TEST".into(), trips, ..Default::default() };
        recompute_driver(&mut driver, 2026, 10, &plain());
        driver
    }

    #[test]
    fn within_the_ceiling_there_is_nothing_to_propose() {
        // 19 days × 9 h = 171 h ≤ 176 h.
        assert!(heavy_month(1..=19).proposals.is_empty());
    }

    #[test]
    fn waiting_does_not_count_toward_the_ceiling() {
        // 23 days × (3 + 3 h driving + 3 h waiting): the 69 h of waiting are not hours, only the 138 h of driving are.
        let driver = month(1..=23, 3, 3);
        assert_eq!(driver.totals.station_min, 23 * 3 * 60, "the waiting is still there to see");
        assert_eq!(driver.totals.total_min, 23 * 6 * 60);
        assert_eq!(driver.totals.over_min, 0, "138 h are under the ceiling of 176 h (with the waiting it would be 207 h)");
        assert!(driver.proposals.is_empty());
    }

    #[test]
    fn a_driver_over_the_ceiling_with_the_time_added_gets_a_whole_day_to_take_out() {
        // 23 days of 6 h of driving, each filled to 8 h = 184 h; the ceiling is 176 h → 8 h over, and one whole day covers it
        // (a weekend day goes first). There is no waiting to shorten any more.
        let mut driver = month(1..=23, 3, 3);
        recompute_driver(&mut driver, 2026, 10, &Settings::default());
        assert_eq!(driver.totals.total_min, 23 * 8 * 60);
        assert_eq!(driver.totals.over_min, 8 * 60);
        assert_eq!(driver.proposals.len(), 1, "{:?}", driver.proposals);
        assert!(is_off_day(driver.proposals[0].date), "a weekend day goes first");
        assert_eq!(driver.proposals[0].minutes, 8 * 60);
    }

    #[test]
    fn whole_days_go_weekends_first() {
        // 23 days of two 4½-hour legs without waiting = 9 h a day = 207 h; 31 h over.
        let driver = heavy_month(1..=23);
        assert_eq!(driver.totals.over_min, 31 * 60);
        let days = &driver.proposals;
        assert_eq!(days.len(), 4, "4 days of 9 h cover 31 h");
        assert!(days.iter().all(|p| is_off_day(p.date)), "weekend days first: {:?}", days.iter().map(|p| p.date).collect::<Vec<_>>());
        assert!(days[0].text.contains("Scoate ziua"));
        assert!(days.iter().map(|p| p.minutes).sum::<i32>() >= 31 * 60);
    }

    #[test]
    fn a_day_that_is_already_out_is_not_proposed_again() {
        let mut driver = heavy_month(1..=23);
        let date = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        driver.excluded_days.push(date);
        recompute_driver(&mut driver, 2026, 10, &plain());
        assert_eq!(driver.totals.over_min, 31 * 60 - 9 * 60);
        assert!(driver.proposals.iter().all(|p| p.date != date));
    }
}
