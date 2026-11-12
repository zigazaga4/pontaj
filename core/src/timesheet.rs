//! The monthly timesheet: one row block per driver, one cell per day.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::calendar;
use crate::pdf::{DriverReport, Segment};
use crate::rules;
use crate::settings::Settings;

/// Absence codes with their own column in the timesheet, in column order.
/// `OLP` is not counted from day cells: it is the remainder of unworked hours.
pub const CODES: [(&str, &str); 11] = [
    ("CO", "Co"),
    ("CM", "CM"),
    ("CP", "CP"),
    ("BP", "Bp"),
    ("AM", "Am"),
    ("CIM", "CIM"),
    ("CC", "CC"),
    ("CFP", "CFP"),
    ("OLP", "OLP"),
    ("N", "N"),
    ("ZLP", "ZLP"),
];
pub const OLP: &str = "OLP";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timesheet {
    pub company: String,
    pub location: String,
    pub year: i32,
    pub month: u32,
    pub working_days: u32,
    pub norm_hours: i32,
    pub days: Vec<DayInfo>,
    pub employees: Vec<Employee>,
    /// Problems met while reading the PDFs (kept until the sheet is cleared).
    pub warnings: Vec<String>,
    /// Observations from the last calculation (e.g. shifts outside the month).
    #[serde(default)]
    pub notices: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayInfo {
    pub day: u32,
    pub weekday: String,
    pub weekend: bool,
    pub holiday: Option<String>,
}

impl DayInfo {
    pub fn is_off(&self) -> bool {
        self.weekend || self.holiday.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Employee {
    pub name: String,
    pub function: String,
    pub location: String,
    /// The rows read from the report(s); editing them re-derives every day.
    #[serde(default)]
    pub activities: Vec<Segment>,
    pub days: Vec<DayEntry>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub start: i32,
    pub end: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DayEntry {
    pub day: u32,
    // --- Derived from the employee's activities by `rules::recompute` ---
    /// Working time (driving + other work) attributed to this day.
    pub worked_min: i32,
    pub pdf_night_min: i32,
    /// First activity start / last activity end, minutes from this day's midnight.
    pub first_start: Option<i32>,
    pub last_end: Option<i32>,
    /// Raw activity blocks, for display.
    pub segments: Vec<Interval>,
    /// The blocks joined into the periods actually worked (what the card shows for the day).
    pub card_intervals: Vec<Interval>,
    // --- Manual edits (kept when the activities change) ---
    pub override_hours: Option<i32>,
    pub override_night: Option<i32>,
    pub override_intervals: Option<Vec<Interval>>,
    pub code: Option<String>,
    // --- Computed by `rules::recompute` ---
    pub hours: Option<i32>,
    pub night_hours: Option<i32>,
    pub night_min: i32,
    pub intervals: Vec<Interval>,
    /// Grid mode only: the intervals do not add up to the hours (hand-edited day).
    pub intervals_off: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub worked: i32,
    /// Working minutes on the tachograph for the days written in the sheet.
    pub card_min: i32,
    pub night: i32,
    pub not_worked: i32,
    /// Hours per code, aligned with [`CODES`].
    pub code_hours: Vec<i32>,
    pub overtime: i32,
}

/// Name as the payroll team writes it: "Popescu, Ion" → "POPESCU ION".
pub fn display_name(raw: &str, plain: bool) -> String {
    let name = raw.replace(',', " ").split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
    if !plain {
        return name;
    }
    name.chars()
        .map(|c| match c {
            'Ă' | 'Â' => 'A',
            'Î' => 'I',
            'Ș' | 'Ş' => 'S',
            'Ț' | 'Ţ' => 'T',
            other => other,
        })
        .collect()
}

/// The month the reports cover: the report period if printed, else the most frequent month.
fn report_month(reports: &[DriverReport]) -> Option<(i32, u32)> {
    if let Some((from, _)) = reports.iter().find_map(|r| r.period) {
        return Some((from.year(), from.month()));
    }
    let mut counts: BTreeMap<(i32, u32), usize> = BTreeMap::new();
    for seg in reports.iter().flat_map(|r| &r.segments) {
        *counts.entry((seg.date.year(), seg.date.month())).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(ym, _)| ym)
}

/// Builds the timesheet for the month covered by the reports.
pub fn build(reports: Vec<DriverReport>, settings: &Settings) -> Option<Timesheet> {
    let (year, month) = report_month(&reports)?;
    let n_days = calendar::days_in_month(year, month);

    let days = (1..=n_days)
        .filter_map(|d| NaiveDate::from_ymd_opt(year, month, d))
        .map(|date| DayInfo {
            day: date.day(),
            weekday: calendar::weekday_short(date).to_string(),
            weekend: calendar::is_weekend(date),
            holiday: calendar::holiday_name(date).map(str::to_string),
        })
        .collect();

    // The same driver may appear in several PDFs: merge by display name.
    let mut by_name: BTreeMap<String, DriverReport> = BTreeMap::new();
    let mut sources: Vec<String> = Vec::new();
    for report in reports {
        if !sources.contains(&report.source) {
            sources.push(report.source.clone());
        }
        let key = display_name(&report.name, settings.plain_names);
        match by_name.get_mut(&key) {
            Some(existing) => {
                for seg in report.segments {
                    if !existing.segments.iter().any(|s| s.same_row(&seg)) {
                        existing.segments.push(seg);
                    }
                }
            }
            None => {
                by_name.insert(key, report);
            }
        }
    }

    let company = by_name.values().map(|r| r.company.clone()).find(|c| !c.is_empty()).unwrap_or_default();
    let location = by_name.values().map(|r| r.location.clone()).find(|c| !c.is_empty()).unwrap_or_default();

    let employees = by_name
        .into_iter()
        .map(|(name, mut report)| {
            report.segments.sort_by_key(|s| (s.date, s.start));
            Employee {
                name,
                function: settings.default_function.clone(),
                location: report.location,
                activities: report.segments,
                days: (1..=n_days).map(|day| DayEntry { day, ..Default::default() }).collect(),
                totals: Totals::default(),
            }
        })
        .collect();

    let mut sheet = Timesheet {
        company,
        location,
        year,
        month,
        working_days: calendar::working_days(year, month),
        norm_hours: 0,
        days,
        employees,
        warnings: Vec::new(),
        notices: Vec::new(),
        sources,
    };
    rules::recompute(&mut sheet, settings);
    Some(sheet)
}

/// Adds the drivers/rows of `fresh` (newly read PDFs) to `previous`, keeping
/// every manual edit. A different month starts a new timesheet instead.
pub fn merge(mut previous: Timesheet, fresh: Timesheet, settings: &Settings) -> Timesheet {
    if (previous.year, previous.month) != (fresh.year, fresh.month) {
        return fresh;
    }
    for employee in fresh.employees {
        match previous.employees.iter_mut().find(|e| e.name == employee.name) {
            Some(existing) => {
                for seg in employee.activities {
                    if !existing.activities.iter().any(|s| s.same_row(&seg)) {
                        existing.activities.push(seg);
                    }
                }
                existing.activities.sort_by_key(|s| (s.date, s.start));
            }
            None => previous.employees.push(employee),
        }
    }
    previous.employees.sort_by(|a, b| a.name.cmp(&b.name));
    for source in fresh.sources {
        if !previous.sources.contains(&source) {
            previous.sources.push(source);
        }
    }
    previous.warnings.extend(fresh.warnings);
    rules::recompute(&mut previous, settings);
    previous
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(source: &str, name: &str, rows: &[(u32, u32, i32, i32)]) -> DriverReport {
        let segments = rows
            .iter()
            .map(|&(month, day, start, end)| {
                let mut seg = Segment::manual(NaiveDate::from_ymd_opt(2026, month, day).unwrap(), start, end);
                seg.manual = false;
                seg.source = source.to_string();
                seg
            })
            .collect();
        DriverReport { name: name.into(), company: "EXEMPLU SRL".into(), location: String::new(), period: None, segments, source: source.into() }
    }

    #[test]
    fn merging_keeps_edits_and_skips_repeated_rows() {
        let s = Settings::default();
        let mut sheet = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        sheet.employees[0].days[0].override_hours = Some(9);

        // The same driver in a second file: one repeated row, one new row; plus a new driver.
        let fresh = build(
            vec![
                report("b.pdf", "Popescu, Ion", &[(9, 1, 480, 960), (9, 2, 480, 720)]),
                report("b.pdf", "Ionescu, Vasile", &[(9, 3, 480, 960)]),
            ],
            &s,
        )
        .unwrap();
        let merged = merge(sheet, fresh, &s);

        assert_eq!(merged.employees.len(), 2);
        let popescu = merged.employees.iter().find(|e| e.name == "POPESCU ION").unwrap();
        assert_eq!(popescu.activities.len(), 2, "the repeated row is not added twice");
        assert_eq!(popescu.days[0].override_hours, Some(9), "manual hours survive");
        assert_eq!(popescu.days[1].hours, Some(4));
        assert_eq!(merged.sources, vec!["a.pdf".to_string(), "b.pdf".to_string()]);
    }

    #[test]
    fn another_month_starts_a_new_sheet() {
        let s = Settings::default();
        let sep = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        let oct = build(vec![report("b.pdf", "Popescu, Ion", &[(10, 1, 480, 960)])], &s).unwrap();
        let merged = merge(sep, oct, &s);
        assert_eq!((merged.year, merged.month), (2026, 10));
        assert_eq!(merged.sources, vec!["b.pdf".to_string()]);
    }

    #[test]
    fn shifts_outside_the_month_become_notices() {
        let s = Settings::default();
        let mut sheet = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960), (9, 2, 480, 960)])], &s).unwrap();
        sheet.employees[0].activities[1].date = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        rules::recompute(&mut sheet, &s);
        assert_eq!(sheet.notices.len(), 1);
        rules::recompute(&mut sheet, &s);
        assert_eq!(sheet.notices.len(), 1, "notices are rebuilt, not accumulated");
    }
}
