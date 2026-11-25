//! The pontaj rules: how raw tachograph activity becomes timesheet hours.
//!
//! 1. Consecutive activity blocks (also across midnight) form one shift; the
//!    shift belongs to the calendar day holding most of its working time.
//! 2. Daily hours = working time rounded to whole hours (`round_up_from_min`).
//! 3. Two work intervals on a `slot_min` grid: one from the (rounded) first
//!    start, one ending at the (rounded) last end, each half of the hours.
//!    (`IntervalMode::Card` writes the real first start / last end instead.)
//! 4. Night hours come from the overlap of the written intervals with the night
//!    window (00:00–07:00 by default) or, if chosen, from the report's own
//!    "Noapte" column (22:00–06:00). They are written only from
//!    `night_threshold_min` upwards.
//!
//! Everything is derived from `Employee::activities`, so editing a scanned row
//! and calling [`recompute`] updates the whole sheet; manual day edits (hours,
//! night, intervals, absence codes) survive the recalculation.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::pdf::Segment;
use crate::settings::{IntervalMode, NightSource, Settings};
use crate::time::{floor_to_slot, round_hours, round_to_half, round_to_slot, DAY};
use crate::timesheet::{is_leave, DayEntry, Employee, Group, Interval, Timesheet, Totals, CODES, OLP};

#[derive(Debug, Clone)]
pub struct Shift {
    /// Day the shift is booked on.
    pub day: NaiveDate,
    /// Minutes relative to `day` midnight (negative = evening before).
    pub start: i32,
    pub end: i32,
    pub work: i32,
    pub night_work: i32,
    /// The breaks the card recorded in the shift (the report's "Pauză" column).
    pub rest: i32,
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
                rest: group.iter().map(|s| s.rest).sum(),
                blocks: group.iter().map(|s| Interval { start: rel(s, s.start), end: rel(s, s.end) }).collect(),
            }
        })
        .collect()
}

fn hours_to_min(hours: f64) -> i32 {
    (hours * 60.0).round() as i32
}

/// Two work intervals covering `hours`, anchored to the real start and end.
pub fn make_intervals(start: i32, end: i32, hours: f64, s: &Settings) -> Vec<Interval> {
    let total = hours_to_min(hours);
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

/// The real working periods of a day: touching blocks are joined, and when
/// more than `max` remain the closest ones are merged (the sheet has two slots).
pub fn card_intervals(blocks: &[Interval], merge_gap_min: i32, max: usize) -> Vec<Interval> {
    let mut sorted = blocks.to_vec();
    sorted.sort_by_key(|b| (b.start, b.end));
    let mut joined: Vec<Interval> = Vec::new();
    for block in sorted {
        match joined.last_mut() {
            Some(last) if block.start - last.end <= merge_gap_min => last.end = last.end.max(block.end),
            _ => joined.push(block),
        }
    }
    while joined.len() > max.max(1) {
        let i = (1..joined.len()).min_by_key(|&i| joined[i].start - joined[i - 1].end).unwrap_or(1);
        let next = joined.remove(i);
        joined[i - 1].end = joined[i - 1].end.max(next.end);
    }
    joined
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

/// Start/end times of a day with `hours` worked, in the configured style.
fn day_intervals(entry: &DayEntry, hours: f64, s: &Settings) -> Vec<Interval> {
    if s.interval_mode == IntervalMode::Card && entry.worked_min > 0 {
        return card_intervals(&entry.card_intervals, 0, 2);
    }
    let start = entry.first_start.unwrap_or_else(|| s.default_start_min());
    let end = entry.last_end.unwrap_or(start + hours_to_min(hours) + s.break_min);
    make_intervals(start, end, hours, s)
}

pub(crate) fn recompute_day(entry: &mut DayEntry, s: &Settings) {
    entry.card_intervals = card_intervals(&entry.segments, s.merge_gap_min, usize::MAX);
    entry.code = entry.code.as_deref().and_then(normalize_code);
    entry.leave_on_free_day &= entry.code.is_some();
    entry.override_intervals = entry.override_intervals.take().filter(|v| !v.is_empty());
    entry.intervals_off = false;
    // Hours typed by hand are whole or half hours, whatever the caller sent.
    entry.override_hours = entry.override_hours.map(round_to_half);
    entry.override_night = entry.override_night.map(round_to_half);
    if entry.code.is_some() {
        entry.hours = None;
        entry.night_hours = None;
        entry.night_min = 0;
        entry.intervals.clear();
        return;
    }
    let auto = (entry.worked_min > 0).then(|| f64::from(round_hours(entry.worked_min, s.round_up_from_min)));
    // Typing start/end times on an empty day is enough to give it hours (to the half hour).
    let typed = entry.override_intervals.as_ref().map(|v| round_to_half(f64::from(v.iter().map(|i| i.end - i.start).sum::<i32>()) / 60.0));
    // Hours typed by hand win, a zero included (an explicit "no work" for a day the card shows work on); then the
    // card; then the times typed on an empty day.
    let hours = match entry.override_hours {
        Some(typed_hours) => Some(typed_hours).filter(|h| *h >= 0.0),
        None => auto.or(typed).filter(|h| *h > 0.0),
    };
    entry.hours = hours;
    // A day set to zero has no times and no night.
    let zero = hours.is_some_and(|h| h <= 0.0);
    entry.intervals = match (&entry.override_intervals, hours) {
        _ if zero => Vec::new(),
        (Some(typed), _) => typed.clone(),
        (None, Some(h)) => day_intervals(entry, h, s),
        (None, None) => Vec::new(),
    };
    entry.intervals_off = s.interval_mode == IntervalMode::Grid
        && hours.is_some_and(|h| entry.intervals.iter().map(|i| i.end - i.start).sum::<i32>() != hours_to_min(h));
    entry.night_min = match (s.night_source, entry.worked_min > 0) {
        _ if zero => 0,
        (NightSource::Card, true) => entry.pdf_night_min,
        _ => night_minutes(&entry.intervals, s.night_window()),
    };
    let auto_night = (entry.night_min > 0 && entry.night_min >= s.night_threshold_min)
        .then(|| f64::from(round_hours(entry.night_min, s.round_up_from_min)))
        .filter(|h| *h > 0.0);
    entry.night_hours = entry.override_night.map_or(auto_night, |n| (n > 0.0).then_some(n));
}

/// A day of this many hours or more written for somebody of the tachograph is told to the office (a day that long is rare and
/// usually a mistake of the report, or two days run together).
pub const LONG_DAY_HOURS: f64 = 12.0;

/// The days of a person of the tachograph with [`LONG_DAY_HOURS`] or more, as one line for the office; `None` when there is none.
fn long_days_notice(employee: &Employee, first: NaiveDate) -> Option<String> {
    let days: Vec<String> = employee
        .days
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| {
            let hours = entry.hours.filter(|hours| *hours >= LONG_DAY_HOURS)?;
            let date = first + chrono::Duration::days(at as i64);
            Some(format!("{} ({} ore)", date.format("%d.%m"), hours.to_string().replace('.', ",")))
        })
        .collect();
    (!days.is_empty()).then(|| format!("{}: a lucrat {} ore sau mai mult într-o zi, pe {}. Verifică dacă este corect.", employee.name, LONG_DAY_HOURS, days.join(", ")))
}

/// Day the activity rows land on, shift by shift; shifts outside the month are reported.
fn place_activities(employee: &mut Employee, first: NaiveDate, merge_gap_min: i32, notices: &mut Vec<String>) {
    for entry in &mut employee.days {
        entry.worked_min = 0;
        entry.pdf_night_min = 0;
        entry.rest_min = 0;
        entry.first_start = None;
        entry.last_end = None;
        entry.segments.clear();
    }
    for shift in shifts(&employee.activities, merge_gap_min) {
        let idx = (shift.day - first).num_days();
        let Some(entry) = usize::try_from(idx).ok().and_then(|i| employee.days.get_mut(i)) else {
            notices.push(format!(
                "{}: tura din {} ({}) aparține altei luni și nu a fost trecută.",
                employee.name,
                shift.day.format("%d.%m.%Y"),
                crate::time::fmt_duration(shift.work)
            ));
            continue;
        };
        entry.worked_min += shift.work;
        entry.pdf_night_min += shift.night_work;
        entry.rest_min += shift.rest;
        entry.first_start = Some(entry.first_start.map_or(shift.start, |s| s.min(shift.start)));
        entry.last_end = Some(entry.last_end.map_or(shift.end, |e| e.max(shift.end)));
        entry.segments.extend(shift.blocks.iter().cloned());
    }
}

/// The hours a day with a code (a leave, an absence) counts in the hours of its code: the shift the day had (a planned leave keeps the
/// times of the shift it took the place of, so a leave of somebody on 12-hour shifts counts 12 per day), else a norm day; nothing for a
/// leave on a day the person is free anyway ([`DayEntry::leave_on_free_day`]).
pub(crate) fn code_day_hours(day: &DayEntry, hours_per_day: i32) -> f64 {
    if day.leave_on_free_day {
        return 0.0;
    }
    match day.override_intervals.as_deref() {
        Some(shift) if !shift.is_empty() => round_to_half(f64::from(shift.iter().map(|i| i.end - i.start).sum::<i32>()) / 60.0),
        _ => f64::from(hours_per_day),
    }
}

/// Mirrors the formulas written in the Excel file.
pub(crate) fn compute_totals(employee: &Employee, norm: i32, hours_per_day: i32) -> Totals {
    // `+ 0.0`: summing nothing gives -0.0, which would show up as "-0" in the sheet and the workbook.
    let worked = employee.days.iter().filter_map(|d| d.hours).sum::<f64>() + 0.0;
    let card_min = employee.days.iter().filter(|d| d.hours.is_some()).map(|d| d.worked_min).sum::<i32>();
    let night = employee.days.iter().filter_map(|d| d.night_hours).sum::<f64>() + 0.0;
    let norm = f64::from(norm);
    let not_worked = (norm - worked).max(0.0);
    let mut code_hours: Vec<f64> = CODES
        .iter()
        .map(|(code, _)| {
            if *code == OLP {
                return 0.0;
            }
            employee.days.iter().filter(|d| d.code.as_deref() == Some(code)).map(|d| code_day_hours(d, hours_per_day)).sum::<f64>()
        })
        .collect();
    let absences: f64 = code_hours.iter().sum();
    // A leave is only a leave: it has its column and is neither an absence nor a reason for overtime.
    let other_absences: f64 = CODES.iter().zip(&code_hours).filter(|((code, _), _)| !is_leave(code)).map(|(_, hours)| hours).sum();
    if let Some(i) = CODES.iter().position(|(c, _)| *c == OLP) {
        code_hours[i] = (not_worked - absences).max(0.0);
    }
    Totals { worked, card_min, night, not_worked, code_hours, overtime: (worked + other_absences - norm).max(0.0) }
}

/// Re-applies the rules after a settings change or a manual edit (of the
/// scanned rows as well as of the days).
pub fn recompute(sheet: &mut Timesheet, s: &Settings) {
    sheet.working_days = crate::calendar::working_days(sheet.year, sheet.month);
    sheet.norm_hours = sheet.working_days as i32 * s.hours_per_day;
    sheet.notices.clear();
    let Some(first) = NaiveDate::from_ymd_opt(sheet.year, sheet.month, 1) else { return };
    for employee in &mut sheet.employees {
        // The truck drivers are known from the list in the settings, whatever the month's reports say about them.
        if employee.group.is_tachograph() {
            employee.group = if s.is_tir(&employee.name) { Group::Tir } else { Group::Tachograph };
        }
        for activity in &mut employee.activities {
            activity.normalize();
        }
        place_activities(employee, first, s.merge_gap_min, &mut sheet.notices);
        for entry in &mut employee.days {
            recompute_day(entry, s);
        }
        employee.totals = compute_totals(employee, employee.norm_hours.unwrap_or(sheet.norm_hours), s.hours_per_day);
        if employee.group.is_tachograph() {
            sheet.notices.extend(long_days_notice(employee, first));
        }
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
            source: String::new(),
            manual: false,
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
            ivs(&make_intervals(hm("6:29"), hm("16:05"), 7.0, &s)),
            vec![("06:30".into(), "10:00".into()), ("12:30".into(), "16:00".into())]
        );
        // Driver B, 01.09: 3h on a 1:33–5:01 shift → the break fits inside the shift.
        assert_eq!(
            ivs(&make_intervals(hm("1:33"), hm("5:01"), 3.0, &s)),
            vec![("01:30".into(), "03:00".into()), ("03:30".into(), "05:00".into())]
        );
        // Same shift typed as 7h by hand → second interval after the break.
        assert_eq!(
            ivs(&make_intervals(hm("1:33"), hm("5:01"), 7.0, &s)),
            vec![("01:30".into(), "05:00".into()), ("06:00".into(), "09:30".into())]
        );
        // Driver A, 17.09: 10:00–20:26, 9h → 10:00–14:30, 16:00–20:30
        assert_eq!(
            ivs(&make_intervals(hm("10:00"), hm("20:26"), 9.0, &s)),
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
        // The old 22:00–07:00 window, set explicitly: the default is now 00:00–07:00.
        let s = Settings { night_start: "22:00".into(), night_end: "07:00".into(), ..Settings::default() };
        let w = s.night_window();
        // Driver C, 06.09: 3:49–13:34, 6h → 04:00–07:00, 10:30–13:30 → 3h of night (04–07).
        assert_eq!(night_minutes(&make_intervals(hm("3:49"), hm("13:34"), 6.0, &s), w), 180);
        // Driver B, 20.09: 17:30–20:30, 23:30–02:30 → 3h.
        let iv = vec![Interval { start: hm("17:30"), end: hm("20:30") }, Interval { start: hm("23:30"), end: hm("2:30") + DAY }];
        assert_eq!(night_minutes(&iv, w), 180);
        // Shift booked on the next day starting the evening before.
        assert_eq!(night_minutes(&[Interval { start: -60, end: 120 }], w), 180);
    }

    // ---- whole-sheet behaviour -------------------------------------------

    use crate::pdf::DriverReport;

    fn sheet_with(segments: Vec<Segment>, s: &Settings) -> Timesheet {
        let report = DriverReport {
            name: "Popescu, Ion".into(),
            company: "EXEMPLU SRL".into(),
            location: String::new(),
            period: None,
            segments,
            source: "raport.pdf".into(),
        };
        crate::timesheet::build(vec![report], s).unwrap()
    }

    fn day(sheet: &Timesheet, d: usize) -> &DayEntry {
        &sheet.employees[0].days[d - 1]
    }

    #[test]
    fn a_day_of_12_hours_or_more_on_the_tachograph_is_told_to_the_office() {
        let s = Settings::default();
        // 12:00 of work on the 15th, 11:00 on the 16th, 12:30 on the 17th (a shift over midnight, the work stays on the 17th).
        let mut sheet = sheet_with(vec![seg(15, "05:00", "18:00", "12:00", "0:00"), seg(16, "06:00", "18:00", "11:00", "0:00"), seg(17, "05:00", "18:30", "12:30", "0:00")], &s);
        let notices: Vec<&String> = sheet.notices.iter().filter(|n| n.contains("ore sau mai mult")).collect();
        assert_eq!(notices.len(), 1, "one line for the person: {:?}", sheet.notices);
        assert!(notices[0].contains("15.09 (12 ore)") && notices[0].contains("17.09") && !notices[0].contains("16.09"), "{}", notices[0]);
        // Hours typed by hand count like the others, and the line goes when the day is corrected.
        sheet.employees[0].days[15].override_hours = Some(14.0);
        recompute(&mut sheet, &s);
        assert!(sheet.notices.iter().any(|n| n.contains("16.09 (14 ore)")), "{:?}", sheet.notices);
        sheet.employees[0].days[14].override_hours = Some(8.0);
        sheet.employees[0].days[15].override_hours = Some(8.0);
        sheet.employees[0].days[16].override_hours = Some(8.0);
        recompute(&mut sheet, &s);
        assert!(!sheet.notices.iter().any(|n| n.contains("ore sau mai mult")), "{:?}", sheet.notices);
    }

    #[test]
    fn default_night_window_is_00_to_07() {
        let w = Settings::default().night_window();
        assert_eq!(w, (0, 7 * 60));
        let n = |a: &str, b: &str| {
            let (start, mut end) = (hm(a), hm(b));
            if end <= start {
                end += 24 * 60; // the shift crosses midnight
            }
            night_minutes(&[Interval { start, end }], w)
        };
        assert_eq!(n("01:00", "05:00"), 4 * 60, "fully inside 00-07");
        assert_eq!(n("22:00", "08:00"), 7 * 60, "crossing midnight: only 00:00-07:00 counts");
        assert_eq!(n("06:00", "10:00"), 60, "06:00-10:00 has one night hour");
        assert_eq!(n("07:00", "15:00"), 0, "07:00-15:00 has none");
        assert_eq!(n("20:00", "23:59"), 0, "evening before midnight is not night");
    }

    #[test]
    fn a_driver_without_night_work_has_a_plain_zero_total() {
        let sheet = sheet_with(vec![seg(9, "8:00", "16:00", "8:00", "0:00")], &Settings::default());
        let night = sheet.employees[0].totals.night;
        assert_eq!(night, 0.0);
        assert!(night.is_sign_positive(), "an empty sum must not be -0.0");
    }

    #[test]
    fn night_can_come_from_the_report_column() {
        let s = Settings { night_source: NightSource::Card, night_start: "22:00".into(), night_end: "06:00".into(), night_threshold_min: 180, ..Settings::default() };
        // 04:23–08:10, 3:14 worked, 1:22 of it at night (22–06): under a minimum of 3h chosen by the office.
        let sheet = sheet_with(vec![seg(7, "4:23", "8:10", "3:14", "1:22")], &s);
        assert_eq!((day(&sheet, 7).hours, day(&sheet, 7).night_min, day(&sheet, 7).night_hours), (Some(3.0), 82, None));
        // 5h shift with 3:30 at night → written, rounded like the hours (3:30 → 4).
        let sheet = sheet_with(vec![seg(8, "0:00", "5:00", "5:00", "3:30")], &s);
        assert_eq!((day(&sheet, 8).night_min, day(&sheet, 8).night_hours), (210, Some(4.0)));
        // Threshold 0 writes every night hour that survives the rounding.
        let all = Settings { night_threshold_min: 0, ..s };
        let sheet = sheet_with(vec![seg(7, "4:23", "8:10", "3:14", "1:22")], &all);
        assert_eq!(day(&sheet, 7).night_hours, Some(2.0), "1:22 rounds up like the hours do");
    }

    #[test]
    fn night_defaults_to_00_07_measured_on_the_written_intervals() {
        let s = Settings::default();
        // 04:00–13:00, 8h → 04:00–08:00 + 09:00–13:00 → 3h inside 00–07.
        let sheet = sheet_with(vec![seg(9, "4:00", "13:00", "8:00", "2:00")], &s);
        assert_eq!((day(&sheet, 9).night_min, day(&sheet, 9).night_hours), (180, Some(3.0)));
        // 21:30–01:30, 4h, booked on the 10th (most work before midnight). The tight shift gets the 1h break, so the
        // written intervals are 21:30–23:30 and 00:30–02:30: the evening before midnight is no longer night, only the
        // 2h after 00:00 count. Every night hour is written (no minimum by default): 2.
        let sheet = sheet_with(vec![seg(10, "21:30", "0:00", "2:30", "2:30"), seg(11, "0:00", "1:30", "1:30", "1:30")], &s);
        let d = day(&sheet, 10);
        assert_eq!(ivs(&d.intervals), vec![("21:30".into(), "23:30".into()), ("00:30".into(), "02:30".into())]);
        assert_eq!((d.hours, d.night_min, d.night_hours), (Some(4.0), 120, Some(2.0)));
        // Two hours at the start of a day, 05:00–09:00: the 5–7 part is night, written too (it used to wait for 3 hours).
        let sheet = sheet_with(vec![seg(12, "5:00", "9:00", "4:00", "2:00")], &s);
        assert_eq!((day(&sheet, 12).night_min, day(&sheet, 12).night_hours), (120, Some(2.0)));
        // A minimum chosen by the office (3h) leaves the 2h out again.
        let minimum = Settings { night_threshold_min: 180, ..Settings::default() };
        let sheet = sheet_with(vec![seg(10, "21:30", "0:00", "2:30", "2:30"), seg(11, "0:00", "1:30", "1:30", "1:30")], &minimum);
        assert_eq!((day(&sheet, 10).night_min, day(&sheet, 10).night_hours), (120, None));
        // The same shift under the old 22:00–07:00 window counted 1h30 + 2h = 3h30 of night and wrote it (rounded to 4).
        let old = Settings { night_start: "22:00".into(), ..Settings::default() };
        let sheet = sheet_with(vec![seg(10, "21:30", "0:00", "2:30", "2:30"), seg(11, "0:00", "1:30", "1:30", "1:30")], &old);
        assert_eq!((day(&sheet, 10).night_min, day(&sheet, 10).night_hours), (210, Some(4.0)));
    }

    #[test]
    fn card_mode_writes_the_real_times() {
        let s = Settings { interval_mode: IntervalMode::Card, ..Settings::default() };
        let sheet = sheet_with(vec![seg(4, "6:29", "16:05", "6:25", "0:00")], &s);
        let d = day(&sheet, 4);
        assert_eq!(ivs(&d.intervals), vec![("06:29".into(), "16:05".into())]);
        assert_eq!(d.hours, Some(7.0));
        assert!(!d.intervals_off, "only the grid mode compares intervals with hours");
        // Two separate blocks stay two intervals; three get merged down to two.
        let blocks = [
            Interval { start: 360, end: 480 },
            Interval { start: 600, end: 720 },
            Interval { start: 735, end: 800 },
        ];
        assert_eq!(card_intervals(&blocks, 30, 2), vec![Interval { start: 360, end: 480 }, Interval { start: 600, end: 800 }]);
        assert_eq!(card_intervals(&blocks, 30, 1), vec![Interval { start: 360, end: 800 }]);
    }

    #[test]
    fn editing_a_scanned_row_recalculates_the_day_and_keeps_manual_edits() {
        let s = Settings::default();
        let mut sheet = sheet_with(vec![seg(2, "6:50", "17:09", "7:29", "0:00"), seg(3, "7:00", "15:00", "7:00", "0:00")], &s);
        assert_eq!(day(&sheet, 2).hours, Some(8.0), "7:29 rounds up");
        assert_eq!(day(&sheet, 3).hours, Some(7.0));
        sheet.employees[0].days[2].override_hours = Some(8.0); // manual hours on the 3rd

        // The 2nd: one more hour of other work → 8:29 worked → 9h; the row is now manual.
        let row = &mut sheet.employees[0].activities[0];
        row.other_work += 60;
        row.manual = true;
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 2).worked_min, 8 * 60 + 29);
        assert_eq!(day(&sheet, 2).hours, Some(9.0));
        assert_eq!(day(&sheet, 3).hours, Some(8.0), "manual hours survive a recalculation");

        // Deleting the row empties the day again.
        sheet.employees[0].activities.remove(0);
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 2).hours, None);
    }

    #[test]
    fn typed_intervals_and_night_override_the_computed_ones() {
        let s = Settings::default();
        let mut sheet = sheet_with(vec![seg(2, "6:50", "17:09", "7:29", "0:00")], &s);
        // A day with no card data: typing start/end times gives it hours.
        sheet.employees[0].days[9].override_intervals = Some(vec![Interval { start: hm("08:00"), end: hm("12:00") }, Interval { start: hm("13:00"), end: hm("17:00") }]);
        // The 2nd keeps its hours but gets other times and a manual night.
        sheet.employees[0].days[1].override_intervals = Some(vec![Interval { start: hm("07:00"), end: hm("14:00") }]);
        sheet.employees[0].days[1].override_night = Some(2.0);
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 10).hours, Some(8.0));
        assert_eq!(ivs(&day(&sheet, 2).intervals), vec![("07:00".into(), "14:00".into())]);
        assert_eq!((day(&sheet, 2).hours, day(&sheet, 2).night_hours), (Some(8.0), Some(2.0)));
        assert!(day(&sheet, 2).intervals_off, "7h of intervals against 8h written");
        assert!(!day(&sheet, 10).intervals_off);
        // Clearing the overrides returns to the automatic values.
        sheet.employees[0].days[1].override_intervals = None;
        sheet.employees[0].days[1].override_night = None;
        recompute(&mut sheet, &s);
        assert!(!day(&sheet, 2).intervals_off);
        assert_eq!(day(&sheet, 2).night_hours, None);
    }

    #[test]
    fn hours_typed_by_hand_can_be_half_hours() {
        let s = Settings::default();
        let mut sheet = sheet_with(vec![seg(2, "6:50", "17:09", "7:29", "0:00")], &s);
        // 7:40 typed → the nearest half hour; the intervals add up to it (3:30 + 4:00).
        sheet.employees[0].days[1].override_hours = Some(7.7);
        sheet.employees[0].days[1].override_night = Some(2.25);
        // A day without card data gets 4.5 h.
        sheet.employees[0].days[4].override_hours = Some(4.5);
        recompute(&mut sheet, &s);
        let d = day(&sheet, 2);
        assert_eq!((d.override_hours, d.hours, d.night_hours), (Some(7.5), Some(7.5), Some(2.5)));
        assert_eq!(d.intervals.iter().map(|i| i.end - i.start).sum::<i32>(), 450);
        assert!(!d.intervals_off);
        assert_eq!(day(&sheet, 5).hours, Some(4.5));
        // Totals keep the halves; the norm and OLP follow.
        let t = &sheet.employees[0].totals;
        assert_eq!((t.worked, t.night), (12.0, 2.5));
        assert_eq!(t.not_worked, sheet.norm_hours as f64 - 12.0);
        // Typing only start/end times on an empty day: 08:00–12:00 + 13:00–16:40 → 7:40 → 7.5 h.
        sheet.employees[0].days[9].override_intervals = Some(vec![Interval { start: hm("08:00"), end: hm("12:00") }, Interval { start: hm("13:00"), end: hm("16:40") }]);
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 10).hours, Some(7.5));
        assert!(day(&sheet, 10).intervals_off, "7:40 of intervals against 7.5 h written");
    }

    #[test]
    fn a_day_can_be_set_to_zero_hours() {
        // The card shows 7:29 worked on the 2nd (8 h written, 1 h of it at night under the report's 22–06 window).
        let s = Settings { night_source: NightSource::Card, night_start: "22:00".into(), night_end: "06:00".into(), night_threshold_min: 0, ..Settings::default() };
        let mut sheet = sheet_with(vec![seg(2, "0:00", "7:29", "7:29", "5:59"), seg(3, "7:00", "15:00", "7:00", "0:00")], &s);
        assert_eq!((day(&sheet, 2).hours, day(&sheet, 2).night_hours.is_some()), (Some(8.0), true));

        // Zero typed: the day stays in the sheet as 0 (not empty, not the card's hours), with no times and no night.
        sheet.employees[0].days[1].override_hours = Some(0.0);
        // A zero on a day with no card data at all is also kept.
        sheet.employees[0].days[4].override_hours = Some(0.0);
        recompute(&mut sheet, &s);
        let d = day(&sheet, 2);
        assert_eq!((d.hours, d.night_hours, d.night_min), (Some(0.0), None, 0));
        assert!(d.intervals.is_empty() && !d.intervals_off);
        assert_eq!(day(&sheet, 5).hours, Some(0.0));
        // The totals leave the day out of the hours but still compare the 0 with what the card shows.
        let t = &sheet.employees[0].totals;
        assert_eq!((t.worked, t.night), (7.0, 0.0));
        assert_eq!(t.card_min, 7 * 60 + 29 + 7 * 60);

        // Typed times on a zero day are ignored while it is zero; clearing the zero returns to the card.
        sheet.employees[0].days[1].override_intervals = Some(vec![Interval { start: hm("08:00"), end: hm("12:00") }]);
        recompute(&mut sheet, &s);
        assert!(day(&sheet, 2).intervals.is_empty());
        sheet.employees[0].days[1].override_hours = None;
        sheet.employees[0].days[1].override_intervals = None;
        sheet.employees[0].days[4].override_hours = None;
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 2).hours, Some(8.0));
        assert_eq!(day(&sheet, 5).hours, None);
    }

    #[test]
    fn a_day_shows_the_pause_the_card_recorded() {
        let s = Settings::default();
        let mut first = seg(2, "6:00", "12:00", "5:00", "0:00");
        first.rest = hm("1:00");
        let mut second = seg(2, "12:20", "17:00", "4:00", "0:00");
        second.rest = hm("0:40");
        let mut other_day = seg(3, "7:00", "15:00", "7:00", "0:00");
        other_day.rest = hm("1:00");
        let mut sheet = sheet_with(vec![first, second, other_day], &s);
        assert_eq!((day(&sheet, 2).rest_min, day(&sheet, 3).rest_min, day(&sheet, 4).rest_min), (hm("1:40"), hm("1:00"), 0));
        // Deleting the rows of the 2nd takes its pause away too.
        sheet.employees[0].activities.retain(|a| a.date.format("%d").to_string() != "02");
        recompute(&mut sheet, &s);
        assert_eq!(day(&sheet, 2).rest_min, 0);
    }

    #[test]
    fn totals_compare_with_the_card() {
        let s = Settings::default();
        let sheet = sheet_with(vec![seg(2, "6:50", "17:09", "7:29", "0:00"), seg(3, "7:00", "13:00", "6:10", "0:00")], &s);
        let t = &sheet.employees[0].totals;
        // 7:29 → 8h and 6:10 → 6h written, against 13:39 on the card.
        assert_eq!((t.worked, t.card_min), (8.0 + 6.0, 7 * 60 + 29 + 6 * 60 + 10));
    }
}
