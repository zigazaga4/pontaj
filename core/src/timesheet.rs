//! The monthly timesheet: one row block per driver, one cell per day.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::calendar;
use crate::pdf::DriverReport;
use crate::rules::{self, Shift};
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
    pub warnings: Vec<String>,
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
    pub days: Vec<DayEntry>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub start: i32,
    pub end: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayEntry {
    pub day: u32,
    // --- From the PDF (read-only) ---
    /// Working time (driving + other work) attributed to this day.
    pub worked_min: i32,
    pub pdf_night_min: i32,
    /// First activity start / last activity end, minutes from this day's midnight.
    pub first_start: Option<i32>,
    pub last_end: Option<i32>,
    /// Raw activity blocks, for display.
    pub segments: Vec<Interval>,
    // --- Manual edits ---
    pub override_hours: Option<i32>,
    pub code: Option<String>,
    // --- Computed by `rules::recompute` ---
    pub hours: Option<i32>,
    pub night_hours: Option<i32>,
    pub night_min: i32,
    pub intervals: Vec<Interval>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub worked: i32,
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
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let mut warnings = Vec::new();

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
                    if !existing.segments.contains(&seg) {
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
        .map(|(name, report)| {
            let mut entries: Vec<DayEntry> = (1..=n_days).map(|day| DayEntry { day, ..Default::default() }).collect();
            for shift in rules::shifts(&report.segments, settings.merge_gap_min) {
                place_shift(&shift, first, &mut entries, &name, &mut warnings);
            }
            Employee {
                name,
                function: settings.default_function.clone(),
                location: report.location,
                days: entries,
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
        warnings,
        sources,
    };
    rules::recompute(&mut sheet, settings);
    Some(sheet)
}

fn place_shift(shift: &Shift, first: NaiveDate, entries: &mut [DayEntry], name: &str, warnings: &mut Vec<String>) {
    let idx = (shift.day - first).num_days();
    let Some(entry) = usize::try_from(idx).ok().and_then(|i| entries.get_mut(i)) else {
        warnings.push(format!(
            "{name}: tura din {} ({}) aparține altei luni și a fost ignorată.",
            shift.day.format("%d.%m.%Y"),
            crate::time::fmt_duration(shift.work)
        ));
        return;
    };
    entry.worked_min += shift.work;
    entry.pdf_night_min += shift.night_work;
    entry.first_start = Some(entry.first_start.map_or(shift.start, |s| s.min(shift.start)));
    entry.last_end = Some(entry.last_end.map_or(shift.end, |e| e.max(shift.end)));
    entry.segments.extend(shift.blocks.iter().cloned());
}
