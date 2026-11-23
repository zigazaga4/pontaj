//! The two weekly checks of a driver, in every block of 7 days counted from the 1st of the month (1–7, 8–14, 15–21, 22–28, and
//! the rest of the month, which is shorter):
//! - the time he is active on trips must not pass `week_max_active_min` (40 h): waiting between trips and the time added to
//!   the days are not trip time;
//! - a full week (7 days) must have at least `week_min_free_days` (2) days without work.
//!
//! A day is judged as the pontaj shows it: it is free when it has no hours there (a leave day is free), and a day the office
//! took out of the pontaj is free too. The checks are only remarks; the office decides, and can turn a worked day into a free
//! one with the same "take the day out of the pontaj" the monthly ceiling uses (see `Driver::excluded_days`).

use chrono::NaiveDate;

use super::compute::trip_days;
use super::config::CatbSettings;
use super::model::*;
use crate::calendar;
use crate::time::fmt_duration;

/// Days in a week of the checks.
const WEEK_DAYS: u32 = 7;

/// The weeks of the month for one driver, with the checks done (they are off where the setting is 0).
pub(super) fn build(driver: &Driver, year: i32, month: u32, cfg: &CatbSettings) -> Vec<Week> {
    let last = calendar::days_in_month(year, month);
    (1..=last)
        .step_by(WEEK_DAYS as usize)
        .map(|first| {
            let end = (first + WEEK_DAYS - 1).min(last);
            let days: Vec<WeekDay> = (first..=end).filter_map(|day| NaiveDate::from_ymd_opt(year, month, day)).map(|date| week_day(driver, date)).collect();
            let active_min = days.iter().filter(|d| d.state == WeekDayState::Worked).map(|d| d.trip_min).sum();
            let free_count = days.iter().filter(|d| matches!(d.state, WeekDayState::Free | WeekDayState::Out)).count() as i32;
            let full = days.len() == WEEK_DAYS as usize;
            Week {
                from: days.first().and_then(|d| d.date),
                to: days.last().and_then(|d| d.date),
                active_min,
                free_count,
                full,
                limit_active_min: cfg.week_max_active_min.max(0),
                need_free_days: cfg.week_min_free_days.max(0),
                too_long: cfg.week_max_active_min > 0 && active_min > cfg.week_max_active_min,
                // A week cut short by the end of the month goes on in the next month's sheet: nothing to say about its rest here.
                few_free: full && free_count < cfg.week_min_free_days,
                days,
            }
        })
        .collect()
}

fn week_day(driver: &Driver, date: NaiveDate) -> WeekDay {
    let row = driver.days.iter().find(|d| d.date == Some(date));
    let worked = row.filter(|r| r.trips > 0 || r.total_min > 0);
    let state = match worked {
        Some(r) if r.excluded => WeekDayState::Out,
        Some(_) => WeekDayState::Worked,
        None if on_excursion(driver, date) => WeekDayState::Tachograph,
        None => WeekDayState::Free,
    };
    WeekDay { date: Some(date), state, trip_min: worked.map_or(0, |r| r.trip_min), leave: row.and_then(|r| r.leave.clone()) }
}

/// An excursion (counted on the tachograph, so not in the pontaj) takes the day: the driver did work.
fn on_excursion(driver: &Driver, date: NaiveDate) -> bool {
    driver.trips.iter().any(|t| !t.edit.removed && t.eff.excursion && trip_days(t).contains(&date))
}

/// `01.11–07.11`.
fn span(week: &Week) -> String {
    let day = |date: Option<NaiveDate>| date.map_or_else(String::new, |d| d.format("%d.%m").to_string());
    format!("{}–{}", day(week.from), day(week.to))
}

/// "nicio zi liberă", "o zi liberă", "2 zile libere".
fn free_days_text(count: i32) -> String {
    match count {
        0 => "nicio zi liberă".into(),
        1 => "o zi liberă".into(),
        n => format!("{n} zile libere"),
    }
}

/// The remarks of the weeks that break a limit: one per week and per limit.
pub(super) fn alerts(weeks: &[Week]) -> Vec<Alert> {
    let mut out = Vec::new();
    for week in weeks {
        let when = span(week);
        if week.too_long {
            out.push(Alert::new(
                "week_hours",
                Level::Warn,
                format!(
                    "Săptămâna {when}: {} ore active pe cursă, peste limita de {}. Fă una dintre zilele lucrate zi liberă sau verifică cursele.",
                    fmt_duration(week.active_min),
                    fmt_duration(week.limit_active_min)
                ),
            ));
        }
        if week.few_free {
            let have = if week.free_count == 0 { "nicio zi liberă".to_string() } else { format!("doar {}", free_days_text(week.free_count)) };
            out.push(Alert::new(
                "week_rest",
                Level::Warn,
                format!("Săptămâna {when}: {have}, trebuie cel puțin {}. Fă una dintre zilele lucrate zi liberă.", free_days_text(week.need_free_days)),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::super::compute::recompute_driver;
    use super::*;
    use crate::settings::Settings;
    use chrono::Datelike;

    /// The rules of these tests are about the weeks alone: days are not filled up to 8 hours.
    fn plain() -> Settings {
        let mut settings = Settings::default();
        settings.catb.fill_day = false;
        settings
    }

    fn trip(id: String, from: &str, to: &str, departure: String, arrival: String) -> Trip {
        Trip { id, kind: TripKind::Eco, from: from.into(), to: to.into(), departure: parse_time(&departure).unwrap(), arrival: parse_time(&arrival), ..Default::default() }
    }

    fn clock(day: u32, minute: i32) -> String {
        format!("2026-10-{day:02} {:02}:{:02}", minute / 60, minute % 60)
    }

    /// Each of `days` (October 2026) has two regular legs of `leg_min` minutes: `2 × leg_min` on trips.
    fn driver(days: &[u32], leg_min: i32) -> Driver {
        let mut trips = Vec::new();
        for &d in days {
            trips.push(trip(format!("a{d}"), "Constanta", "Otopeni", clock(d, 300), clock(d, 300 + leg_min)));
            trips.push(trip(format!("b{d}"), "Otopeni", "Constanta", clock(d, 320 + leg_min), clock(d, 320 + 2 * leg_min)));
        }
        let mut driver = Driver { id: 1, name: "TEST".into(), trips, ..Default::default() };
        recompute_driver(&mut driver, 2026, 10, &plain());
        driver
    }

    fn redo(driver: &mut Driver, settings: &Settings) {
        recompute_driver(driver, 2026, 10, settings);
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn codes(driver: &Driver) -> Vec<&str> {
        driver.alerts.iter().map(|a| a.code.as_str()).filter(|c| c.starts_with("week_")).collect()
    }

    #[test]
    fn the_month_is_cut_into_blocks_of_seven_days_from_the_first() {
        let d = driver(&[], 60);
        let spans: Vec<(u32, u32)> = d.weeks.iter().map(|w| (w.from.unwrap().day(), w.to.unwrap().day())).collect();
        assert_eq!(spans, vec![(1, 7), (8, 14), (15, 21), (22, 28), (29, 31)]);
        assert_eq!(d.weeks.iter().map(|w| w.full).collect::<Vec<_>>(), vec![true, true, true, true, false]);
        assert!(d.weeks.iter().all(|w| w.free_count as usize == w.days.len()), "no work = every day is free");
        assert!(d.weeks.iter().all(|w| w.limit_active_min == 2400 && w.need_free_days == 2), "the limits come from the settings");
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn five_days_of_exactly_forty_hours_with_two_days_free_is_fine() {
        // 5 days × 2 legs of 4 h = 40:00, which is the limit, not over it.
        let d = driver(&[1, 2, 3, 4, 5], 240);
        let week = &d.weeks[0];
        assert_eq!((week.active_min, week.free_count), (2400, 2));
        assert!(!week.too_long && !week.few_free);
        assert!(codes(&d).is_empty(), "{:?}", d.alerts);
    }

    #[test]
    fn more_than_forty_hours_on_trips_in_a_week_is_flagged() {
        // 5 days × 2 legs of 4 h 10 = 41:40.
        let d = driver(&[1, 2, 3, 4, 5], 250);
        assert!(d.weeks[0].too_long && !d.weeks[0].few_free);
        assert_eq!(codes(&d), vec!["week_hours"]);
        let text = &d.alerts.iter().find(|a| a.code == "week_hours").unwrap().text;
        assert!(text.contains("01.10–07.10"), "{text}");
        assert!(text.contains("41:40") && text.contains("40:00"), "{text}");
        assert_eq!(d.alerts[0].level, Level::Warn);
    }

    #[test]
    fn the_hours_of_the_week_are_trip_time_not_waiting_and_not_the_time_added() {
        // With the time added to the days on (1 h per trip: 5 days × 8 h in the pontaj) the trips are still 2 × 3 h a day.
        let mut d = driver(&[1, 2, 3, 4, 5], 180);
        redo(&mut d, &Settings::default());
        assert_eq!(d.totals.total_min, 5 * 8 * 60, "the pontaj has 40 h");
        assert_eq!(d.weeks[0].active_min, 5 * 6 * 60, "the driver was active 30 h");
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn a_full_week_of_work_has_no_free_day() {
        let d = driver(&[8, 9, 10, 11, 12, 13, 14], 60);
        let week = &d.weeks[1];
        assert_eq!(week.free_count, 0);
        assert!(week.few_free && !week.too_long);
        assert_eq!(codes(&d), vec!["week_rest"]);
        let text = &d.alerts[0].text;
        assert!(text.contains("08.10–14.10") && text.contains("nicio zi liberă") && text.contains("2 zile libere"), "{text}");
    }

    #[test]
    fn one_free_day_of_two_is_still_a_remark() {
        let d = driver(&[15, 16, 17, 18, 19, 20], 60);
        assert_eq!(d.weeks[2].free_count, 1);
        assert!(d.alerts.iter().any(|a| a.code == "week_rest" && a.text.contains("doar o zi liberă")), "{:?}", d.alerts);
    }

    #[test]
    fn a_worked_day_turned_free_counts_as_free_and_takes_its_hours_out() {
        // 7 days × 2 legs of 3 h 50 = 53:40 on trips, no free day.
        let mut d = driver(&[1, 2, 3, 4, 5, 6, 7], 230);
        assert_eq!(codes(&d), vec!["week_hours", "week_rest"]);
        assert_eq!(d.weeks[0].days.iter().filter(|x| x.state == WeekDayState::Worked).count(), 7);

        d.excluded_days.push(date(6));
        redo(&mut d, &plain());
        assert_eq!(d.weeks[0].days[5].state, WeekDayState::Out);
        assert_eq!((d.weeks[0].free_count, d.weeks[0].active_min), (1, 6 * 460));
        assert_eq!(codes(&d), vec!["week_hours", "week_rest"], "one free day is not enough yet");

        d.excluded_days.push(date(7));
        redo(&mut d, &plain());
        assert_eq!((d.weeks[0].free_count, d.weeks[0].active_min), (2, 5 * 460));
        assert!(codes(&d).is_empty(), "{:?}", d.alerts);

        // Putting the day back brings the remarks back.
        d.excluded_days.clear();
        redo(&mut d, &plain());
        assert_eq!(codes(&d), vec!["week_hours", "week_rest"]);
    }

    #[test]
    fn the_short_last_week_is_checked_for_hours_but_not_for_free_days() {
        // 29–31: three days of 2 × 8 h = 48 h in 3 days, which is over the limit whatever follows; but 3 days cannot show
        // that the rest is missing (the week goes on in November).
        let d = driver(&[29, 30, 31], 480);
        let last = &d.weeks[4];
        assert!(!last.full && last.too_long && !last.few_free);
        assert_eq!(codes(&d), vec!["week_hours"]);
        let d = driver(&[29, 30, 31], 60);
        assert!(codes(&d).is_empty(), "{:?}", d.alerts);
    }

    #[test]
    fn a_leave_day_and_a_day_without_trips_are_free() {
        let mut d = driver(&[8, 9, 10, 11, 12], 60);
        d.leaves.insert(date(13), "CO".into());
        redo(&mut d, &plain());
        let week = &d.weeks[1];
        assert_eq!(week.free_count, 2);
        assert_eq!(week.days[5].leave.as_deref(), Some("CO"));
        assert_eq!(week.days[5].state, WeekDayState::Free);
        assert!(codes(&d).is_empty());
    }

    #[test]
    fn an_excursion_day_is_a_day_of_work_with_nothing_to_take_out() {
        // 15–19 worked, 20 and 21 free, but an excursion by coach takes the 20th: the driver worked, so only one day is free.
        let mut d = driver(&[15, 16, 17, 18, 19], 60);
        d.trips.push(trip("bus-1".into(), "Constanta", "Kalambaka", clock(20, 240), clock(20, 240)));
        d.trips.last_mut().unwrap().kind = TripKind::Bus;
        d.trips.last_mut().unwrap().arrival = None;
        redo(&mut d, &plain());
        let day = &d.weeks[2].days[5];
        assert_eq!(day.state, WeekDayState::Tachograph);
        assert_eq!(d.weeks[2].free_count, 1);
        assert!(d.alerts.iter().any(|a| a.code == "week_rest"), "{:?}", d.alerts);
    }

    #[test]
    fn a_trip_the_office_removed_does_not_make_a_day_of_work() {
        let mut d = driver(&[1, 2, 3, 4, 5, 6, 7], 60);
        assert_eq!(codes(&d), vec!["week_rest"]);
        for trip in d.trips.iter_mut().filter(|t| t.id.ends_with('7')) {
            trip.edit.removed = true;
        }
        redo(&mut d, &plain());
        assert_eq!(d.weeks[0].free_count, 1);
    }

    #[test]
    fn the_limits_are_settings_and_zero_turns_a_check_off() {
        let mut d = driver(&[1, 2, 3, 4, 5, 6, 7], 240);
        assert_eq!(codes(&d), vec!["week_hours", "week_rest"]);

        let mut settings = plain();
        settings.catb.week_max_active_min = 0;
        settings.catb.week_min_free_days = 0;
        redo(&mut d, &settings);
        assert!(codes(&d).is_empty(), "{:?}", d.alerts);
        assert!(!d.weeks[0].too_long && !d.weeks[0].few_free);

        // 7 days × 8 h = 56 h is under a limit of 60 h, but one free day is still asked for.
        settings.catb.week_max_active_min = 60 * 60;
        settings.catb.week_min_free_days = 1;
        redo(&mut d, &settings);
        assert_eq!(codes(&d), vec!["week_rest"]);
        assert!(d.alerts.iter().any(|a| a.text.contains("cel puțin o zi liberă")), "{:?}", d.alerts);
    }
}
