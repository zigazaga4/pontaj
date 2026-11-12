//! The pontaj rules: how raw tachograph activity becomes timesheet hours.
//!
//! 1. Consecutive activity blocks (also across midnight) form one shift; the
//!    shift belongs to the calendar day holding most of its working time.
//! 2. Daily hours = working time rounded to whole hours (`round_up_from_min`).
//! 3. Two work intervals on a `slot_min` grid: one from the (rounded) first
//!    start, one ending at the (rounded) last end, each half of the hours.
//! 4. Night hours = overlap of those intervals with the night window, written
//!    only from `night_threshold_min` upwards.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::pdf::Segment;
use crate::settings::Settings;
use crate::time::{floor_to_slot, round_hours, round_to_slot, DAY};
use crate::timesheet::{DayEntry, Employee, Interval, Timesheet, Totals, CODES, OLP};

#[derive(Debug, Clone)]
pub struct Shift {
    /// Day the shift is booked on.
    pub day: NaiveDate,
    /// Minutes relative to `day` midnight (negative = evening before).
    pub start: i32,
    pub end: i32,
    pub work: i32,
    pub night_work: i32,
    pub blocks: Vec<Interval>,
}

fn abs_min(date: NaiveDate, epoch: NaiveDate, min: i32) -> i64 {
    (date - epoch).num_days() * DAY as i64 + min as i64
}

/// Groups activity blocks into shifts and books each on its main day.
pub fn shifts(segments: &[Segment], merge_gap_min: i32) -> Vec<Shift> {
    let Some(epoch) = segments.iter().map(|s| s.date).min() else { return Vec::new() };
    let mut sorted: Vec<&Segment> = segments.iter().collect();
    sorted.sort_by_key(|s| abs_min(s.date, epoch, s.start));

    let mut groups: Vec<Vec<&Segment>> = Vec::new();
    for seg in sorted {
        let start = abs_min(seg.date, epoch, seg.start);
        match groups.last_mut() {
            Some(group) if start - group.iter().map(|g| abs_min(g.date, epoch, g.end)).max().unwrap_or(i64::MIN) <= merge_gap_min as i64 => {
                group.push(seg)
            }
            _ => groups.push(vec![seg]),
        }
    }

    groups
        .into_iter()
        .map(|group| {
            let mut work_per_day: BTreeMap<NaiveDate, i32> = BTreeMap::new();
            for seg in &group {
                *work_per_day.entry(seg.date).or_default() += seg.work;
            }
            // Most work wins; on a tie the earliest day (BTreeMap order + strict >).
            let day = work_per_day
                .iter()
                .fold((group[0].date, i32::MIN), |best, (d, w)| if *w > best.1 { (*d, *w) } else { best })
                .0;
            let rel = |seg: &Segment, min: i32| min + (seg.date - day).num_days() as i32 * DAY;
            Shift {
                day,
                start: group.iter().map(|s| rel(s, s.start)).min().unwrap_or(0),
                end: group.iter().map(|s| rel(s, s.end)).max().unwrap_or(0),
                work: group.iter().map(|s| s.work).sum(),
                night_work: group.iter().map(|s| s.night_work).sum(),
                blocks: group.iter().map(|s| Interval { start: rel(s, s.start), end: rel(s, s.end) }).collect(),
            }
        })
        .collect()
}

/// Two work intervals covering `hours`, anchored to the real start and end.
pub fn make_intervals(start: i32, end: i32, hours: i32, s: &Settings) -> Vec<Interval> {
    let total = hours * 60;
    if total <= 0 {
        return Vec::new();
    }
    let slot = s.slot_min.max(1);
    let first_len = floor_to_slot(total / 2, slot).max(slot.min(total));
    let second_len = total - first_len;
    let from = round_to_slot(start, slot);
    let to = round_to_slot(end, slot).max(from);
    let first = Interval { start: from, end: from + first_len };
    if second_len <= 0 {
        return vec![first];
    }
    // Keep both intervals inside the real shift whenever a break of at least
    // one slot fits; otherwise (hours typed above the shift) extend after a break.
    let second = if to - from - total >= s.break_min.min(slot) {
        Interval { start: to - second_len, end: to }
    } else {
        let start = first.end + s.break_min.max(0);
        Interval { start, end: start + second_len }
    };
    vec![first, second]
}

/// Minutes of the intervals falling in the night window (handles windows
/// spanning midnight and intervals reaching into the neighbouring days).
pub fn night_minutes(intervals: &[Interval], window: (i32, i32)) -> i32 {
    let (ns, ne) = window;
    let windows: Vec<(i32, i32)> = (-1..=2)
        .map(|k| if ns > ne { (k * DAY + ns, (k + 1) * DAY + ne) } else { (k * DAY + ns, k * DAY + ne) })
        .collect();
    intervals
        .iter()
        .flat_map(|iv| windows.iter().map(move |(ws, we)| (iv.end.min(*we) - iv.start.max(*ws)).max(0)))
        .sum()
}

pub fn normalize_code(code: &str) -> Option<String> {
    let c = code.trim().to_uppercase();
    (!c.is_empty()).then_some(c)
}

fn recompute_day(entry: &mut DayEntry, s: &Settings) {
    entry.code = entry.code.as_deref().and_then(normalize_code);
    if entry.code.is_some() {
        entry.hours = None;
        entry.night_hours = None;
        entry.night_min = 0;
        entry.intervals.clear();
        return;
    }
    let auto = (entry.worked_min > 0).then(|| round_hours(entry.worked_min, s.round_up_from_min));
    let hours = entry.override_hours.or(auto).filter(|h| *h > 0);
    entry.hours = hours;
    entry.intervals = match hours {
        Some(h) => {
            let start = entry.first_start.unwrap_or_else(|| s.default_start_min());
            let end = entry.last_end.unwrap_or(start + h * 60 + s.break_min);
            make_intervals(start, end, h, s)
        }
        None => Vec::new(),
    };
    entry.night_min = night_minutes(&entry.intervals, s.night_window());
    entry.night_hours = (entry.night_min > 0 && entry.night_min >= s.night_threshold_min)
        .then(|| round_hours(entry.night_min, s.round_up_from_min))
        .filter(|h| *h > 0);
}

/// Mirrors the formulas written in the Excel file.
fn compute_totals(employee: &Employee, norm: i32, hours_per_day: i32) -> Totals {
    let worked = employee.days.iter().filter_map(|d| d.hours).sum::<i32>();
    let night = employee.days.iter().filter_map(|d| d.night_hours).sum::<i32>();
    let not_worked = (norm - worked).max(0);
    let mut code_hours: Vec<i32> = CODES
        .iter()
        .map(|(code, _)| {
            if *code == OLP {
                return 0;
            }
            employee.days.iter().filter(|d| d.code.as_deref() == Some(code)).count() as i32 * hours_per_day
        })
        .collect();
    let absences: i32 = code_hours.iter().sum();
    if let Some(i) = CODES.iter().position(|(c, _)| *c == OLP) {
        code_hours[i] = (not_worked - absences).max(0);
    }
    Totals { worked, night, not_worked, code_hours, overtime: (worked + absences - norm).max(0) }
}

/// Re-applies the rules after a settings change or a manual edit.
pub fn recompute(sheet: &mut Timesheet, s: &Settings) {
    sheet.working_days = crate::calendar::working_days(sheet.year, sheet.month);
    sheet.norm_hours = sheet.working_days as i32 * s.hours_per_day;
    for employee in &mut sheet.employees {
        for entry in &mut employee.days {
            recompute_day(entry, s);
        }
        employee.totals = compute_totals(employee, sheet.norm_hours, s.hours_per_day);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_hm;

    fn hm(s: &str) -> i32 {
        parse_hm(s).unwrap()
    }

    fn seg(day: u32, start: &str, end: &str, work: &str, night: &str) -> Segment {
        let (start, mut end) = (hm(start), hm(end));
        if end <= start {
            end += DAY;
        }
        Segment {
            date: NaiveDate::from_ymd_opt(2026, 9, day).unwrap(),
            start,
            end,
            span: end - start,
            driving: hm(work),
            other_work: 0,
            availability: 0,
            rest: 0,
            day_work: 0,
            night_work: hm(night),
            work: hm(work),
        }
    }

    fn ivs(list: &[Interval]) -> Vec<(String, String)> {
        list.iter().map(|i| (crate::time::fmt_clock(i.start), crate::time::fmt_clock(i.end))).collect()
    }

    #[test]
    fn intervals_match_the_payroll_team() {
        let s = Settings::default();
        // Driver A, 12.09: 6:29–16:05, 7h → 06:30–10:00, 12:30–16:00
        assert_eq!(
            ivs(&make_intervals(hm("6:29"), hm("16:05"), 7, &s)),
            vec![("06:30".into(), "10:00".into()), ("12:30".into(), "16:00".into())]
        );
        // Driver B, 01.09: 3h on a 1:33–5:01 shift → the break fits inside the shift.
        assert_eq!(
            ivs(&make_intervals(hm("1:33"), hm("5:01"), 3, &s)),
            vec![("01:30".into(), "03:00".into()), ("03:30".into(), "05:00".into())]
        );
        // Same shift typed as 7h by hand → second interval after the break.
        assert_eq!(
            ivs(&make_intervals(hm("1:33"), hm("5:01"), 7, &s)),
            vec![("01:30".into(), "05:00".into()), ("06:00".into(), "09:30".into())]
        );
        // Driver A, 17.09: 10:00–20:26, 9h → 10:00–14:30, 16:00–20:30
        assert_eq!(
            ivs(&make_intervals(hm("10:00"), hm("20:26"), 9, &s)),
            vec![("10:00".into(), "14:30".into()), ("16:00".into(), "20:30".into())]
        );
    }

    #[test]
    fn cross_midnight_shift_goes_to_the_day_with_most_work() {
        // Driver B, 20.09 17:23–0:00 (3:16) + 21.09 0:00–2:28 (2:28) → one shift on the 20th.
        let list = shifts(&[seg(20, "17:23", "0:00", "3:16", "0:29"), seg(21, "0:00", "2:28", "2:28", "2:28")], 30);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].day.format("%d").to_string(), "20");
        assert_eq!(list[0].work, hm("5:44"));
        assert_eq!(round_hours(list[0].work, 20), 6);
        // Driver C, 13.09 23:05–0:00 (0:07) + 14.09 0:00–3:00, 3:00–4:55 → booked on the 14th.
        let list = shifts(&[seg(13, "23:05", "0:00", "0:07", "0:07"), seg(14, "0:00", "3:00", "1:08", "1:08"), seg(14, "3:00", "4:55", "1:55", "1:55")], 30);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].day.format("%d").to_string(), "14");
        assert_eq!(list[0].start, hm("23:05") - DAY);
    }

    #[test]
    fn night_hours_use_window_and_threshold() {
        let s = Settings::default();
        let w = s.night_window();
        // Driver C, 06.09: 3:49–13:34, 6h → 04:00–07:00, 10:30–13:30 → 3h of night (04–07).
        assert_eq!(night_minutes(&make_intervals(hm("3:49"), hm("13:34"), 6, &s), w), 180);
        // Driver B, 20.09: 17:30–20:30, 23:30–02:30 → 3h.
        let iv = vec![Interval { start: hm("17:30"), end: hm("20:30") }, Interval { start: hm("23:30"), end: hm("2:30") + DAY }];
        assert_eq!(night_minutes(&iv, w), 180);
        // Shift booked on the next day starting the evening before.
        assert_eq!(night_minutes(&[Interval { start: -60, end: 120 }], w), 180);
    }
}
