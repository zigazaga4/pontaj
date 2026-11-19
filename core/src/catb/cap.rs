//! Suggestions for a driver who is over the monthly ceiling (8 h × the Mon–Fri days, holidays included).
//!
//! Only suggestions: the office decides. The order is the one that costs the least:
//! 1. shorten the longest waiting times (hours nobody drove);
//! 2. if that is not enough, take whole days out, weekend and holiday days first.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};

use super::compute::station_pay;
use super::model::*;
use crate::calendar;
use crate::settings::Settings;
use crate::time::fmt_duration;

/// Waiting is cut in steps of this many minutes, so the hours stay easy to read.
const STEP_MIN: i32 = 30;

pub(super) fn propose(driver: &Driver, cap_min: i32, settings: &Settings) -> Vec<Proposal> {
    let excess = driver.totals.total_min - cap_min;
    if excess <= 0 {
        return Vec::new();
    }
    let excluded = |date: NaiveDate| driver.excluded_days.contains(&date);
    let mut remaining = excess;

    // 1. Waiting, longest first. A day is brought up to a full one (8 h), so waiting can only be cut down to that: what is cut
    //    below it would be added back as time before the departure and after the arrival.
    let floor = if settings.catb.fill_day { settings.hours_per_day * 60 } else { 0 };
    let mut room: BTreeMap<NaiveDate, i32> = driver.days.iter().filter_map(|d| Some((d.date?, (d.trip_min + d.station_min - floor).max(0)))).collect();
    let mut pauses: Vec<&Pause> = driver.pauses.iter().filter(|p| p.counted_min > 0 && p.day.is_some_and(|day| !excluded(day))).collect();
    pauses.sort_by_key(|p| Reverse(p.counted_min));
    let mut proposals: Vec<Proposal> = Vec::new();
    for pause in pauses {
        if remaining <= 0 {
            break;
        }
        let Some(date) = pause.day else { continue };
        let day_room = room.entry(date).or_insert(0);
        let cuttable = pause.counted_min.min(*day_room);
        if cuttable <= 0 {
            continue;
        }
        let take = (remaining.div_euclid(STEP_MIN) + i32::from(remaining % STEP_MIN > 0)).saturating_mul(STEP_MIN).min(cuttable);
        *day_room -= take;
        let keep = pause.counted_min - take;
        let pay = pause.extra_pay - station_pay(keep, settings);
        remaining -= take;
        proposals.push(Proposal {
            kind: ProposalKind::Pause,
            date,
            prev: Some(pause.prev.clone()),
            next: Some(pause.next.clone()),
            set_hours: Some(f64::from(keep) / 60.0),
            minutes: take,
            pay,
            text: format!(
                "Scurtează staționarea din {} ({} → {}) de la {} la {}: −{} ore.",
                label(date),
                pause.place_from,
                pause.place_to,
                fmt_duration(pause.counted_min),
                fmt_duration(keep),
                fmt_duration(take)
            ),
        });
    }

    // 2. Whole days, while the ceiling is still exceeded.
    let mut taken: Vec<NaiveDate> = Vec::new();
    while remaining > 0 {
        let trimmed = |date: NaiveDate| -> i32 { proposals.iter().filter(|p| p.kind == ProposalKind::Pause && p.date == date).map(|p| p.minutes).sum() };
        let candidates: Vec<(&DayRow, i32)> = driver
            .days
            .iter()
            .filter(|d| !d.excluded && d.total_min > 0)
            .filter(|d| d.date.is_some_and(|date| !taken.contains(&date)))
            .map(|d| (d, d.total_min - d.date.map_or(0, trimmed)))
            .collect();
        let off = |day: &DayRow| day.date.is_some_and(is_off_day);
        // A day that covers what is left (the smallest one), else the one that removes the most.
        let covering = candidates.iter().filter(|(_, gain)| *gain >= remaining).min_by_key(|(day, gain)| (!off(day), *gain));
        let Some(&(day, gain)) = covering.or_else(|| candidates.iter().min_by_key(|(day, gain)| (!off(day), Reverse(*gain)))) else { break };
        let Some(date) = day.date else { break };

        // The pause cuts of that day are not needed any more: the whole day goes.
        let cut_pay: f64 = proposals.iter().filter(|p| p.kind == ProposalKind::Pause && p.date == date).map(|p| p.pay).sum();
        proposals.retain(|p| !(p.kind == ProposalKind::Pause && p.date == date));
        remaining -= gain;
        taken.push(date);
        let pay = day.pay_total - cut_pay;
        proposals.push(Proposal {
            kind: ProposalKind::Day,
            date,
            prev: None,
            next: None,
            set_hours: None,
            minutes: day.total_min,
            pay,
            text: format!("Scoate ziua {} din pontaj: −{} ore.", label(date), fmt_duration(day.total_min)),
        });
    }
    proposals.sort_by_key(|p| (p.date, p.kind == ProposalKind::Day));
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

    #[test]
    fn within_the_ceiling_there_is_nothing_to_propose() {
        // 19 days × 9 h = 171 h ≤ 176 h.
        assert!(month(1..=19, 3, 3).proposals.is_empty());
    }

    #[test]
    fn waiting_is_shortened_first_longest_first() {
        // 23 days × (3 + 3 h driving + 3 h waiting) = 207 h; ceiling 176 h → 31 h too many, 69 h of waiting available.
        let driver = month(1..=23, 3, 3);
        assert_eq!(driver.totals.over_min, 31 * 60);
        assert!(driver.proposals.iter().all(|p| p.kind == ProposalKind::Pause), "waiting alone is enough");
        let cut: i32 = driver.proposals.iter().map(|p| p.minutes).sum();
        assert_eq!(cut, 31 * 60, "exactly what is over");
        assert_eq!(driver.proposals.len(), 11);
        let first = &driver.proposals[0];
        assert_eq!((first.prev.is_some(), first.set_hours), (true, Some(0.0)));
        assert!(first.text.contains("Scurtează staționarea"));
    }

    #[test]
    fn waiting_is_only_cut_down_to_the_hours_of_a_full_day_when_days_are_filled() {
        // The same 23 days of 9 h (3 + 3 h driving, 3 h waiting): a day is never worth less than 8 h, so each wait gives up
        // 1 h at most (the 31 h over the ceiling need more than the 23 h that waiting can give).
        let mut driver = month(1..=23, 3, 3);
        recompute_driver(&mut driver, 2026, 10, &Settings::default());
        assert_eq!(driver.totals.total_min, 23 * 9 * 60, "days of 9 h are not filled");
        let pauses: Vec<_> = driver.proposals.iter().filter(|p| p.kind == ProposalKind::Pause).collect();
        assert!(pauses.iter().all(|p| p.minutes == 60 && p.set_hours == Some(2.0)), "one hour of each wait: {pauses:?}");
        let days: Vec<_> = driver.proposals.iter().filter(|p| p.kind == ProposalKind::Day).collect();
        assert_eq!(days.len(), 1, "one whole day covers the rest");
        assert!(is_off_day(days[0].date), "a weekend day goes first");
        let gained: i32 = pauses.iter().map(|p| p.minutes).sum::<i32>() + days.iter().map(|p| p.minutes).sum::<i32>();
        assert!(gained >= driver.totals.over_min);
    }

    #[test]
    fn whole_days_go_when_waiting_is_not_enough_and_weekends_go_first() {
        // 23 days of two 4½-hour legs without waiting = 9 h a day = 207 h; 31 h over.
        let driver = {
            let mut trips = Vec::new();
            for d in 1..=23u32 {
                trips.push(trip(format!("a{d}"), "Constanta", "Otopeni", format!("2026-10-{d:02} 05:00"), format!("2026-10-{d:02} 09:30")));
                trips.push(trip(format!("b{d}"), "Otopeni", "Constanta", format!("2026-10-{d:02} 09:30"), format!("2026-10-{d:02} 14:00")));
            }
            let mut driver = Driver { id: 1, name: "TEST".into(), trips, ..Default::default() };
            recompute_driver(&mut driver, 2026, 10, &plain());
            driver
        };
        assert_eq!(driver.totals.over_min, 31 * 60);
        let days: Vec<_> = driver.proposals.iter().filter(|p| p.kind == ProposalKind::Day).collect();
        assert_eq!(days.len(), 4, "4 days of 9 h cover 31 h");
        assert!(days.iter().all(|p| is_off_day(p.date)), "weekend days first: {:?}", days.iter().map(|p| p.date).collect::<Vec<_>>());
        assert!(days[0].text.contains("Scoate ziua"));
        assert!(days.iter().map(|p| p.minutes).sum::<i32>() >= 31 * 60);
    }

    #[test]
    fn a_day_that_is_already_out_is_not_proposed_again() {
        let mut driver = month(1..=23, 3, 3);
        let date = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        driver.excluded_days.push(date);
        recompute_driver(&mut driver, 2026, 10, &plain());
        assert_eq!(driver.totals.over_min, 31 * 60 - 9 * 60);
        assert!(driver.proposals.iter().all(|p| p.date != date));
    }
}
