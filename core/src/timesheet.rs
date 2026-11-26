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

/// Is the code a leave (CO, CM, CP, CFP, CC, CIM: the kinds a planned leave can be) and not another absence? A leave has its own
/// column and takes no part in the overtime.
pub fn is_leave(code: &str) -> bool {
    crate::catb::LEAVE_TYPES.iter().any(|(known, _)| *known == code)
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// Where an employee's hours come from. It decides the colour of the name in the foaie colectivă, as in the payroll file:
/// yellow for the tachograph, white for everybody else, green for the truck drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    /// Read from a tachograph report (the category D drivers are here too). Yellow.
    #[default]
    Tachograph,
    /// A truck driver (tir): on the tachograph too, but listed apart in the settings (`Settings::tir_drivers`). Green.
    Tir,
    /// Categoria B: the trips of the ALT API. White.
    CatB,
    /// Office staff; the hours are inferred from a plan (see [`crate::roster`]). White.
    Office,
    /// The mechanics of the garage, inferred like the office. White.
    Garage,
    /// The drivers on the city routes, inferred like the office. White.
    City,
}

impl Group {
    /// Does a person of this group come from a tachograph report?
    pub fn is_tachograph(self) -> bool {
        matches!(self, Group::Tachograph | Group::Tir)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Employee {
    pub name: String,
    pub function: String,
    pub location: String,
    /// Where the hours come from; old files without it are tachograph sheets.
    #[serde(default)]
    pub group: Group,
    /// The norm of this person when it is not the month's (Categoria B has a ceiling of its own); `None` = the sheet's.
    #[serde(default)]
    pub norm_hours: Option<i32>,
    /// The rows read from the report(s); editing them re-derives every day.
    #[serde(default)]
    pub activities: Vec<Segment>,
    /// Ids of the planned leaves ([`crate::leave::LeavePlan`]) already put on this person's days, so each one is applied once.
    #[serde(default)]
    pub applied_leaves: Vec<String>,
    pub days: Vec<DayEntry>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub start: i32,
    pub end: i32,
}

/// A remark on one time of a day: the start (`end == false`) or the end of its `interval`-th period.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimeNote {
    pub interval: usize,
    pub end: bool,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DayEntry {
    pub day: u32,
    // --- Derived from the employee's activities by `rules::recompute` ---
    /// Working time (driving + other work) attributed to this day.
    pub worked_min: i32,
    pub pdf_night_min: i32,
    /// The breaks the card recorded on this day (the report's "Pauză" column).
    pub rest_min: i32,
    /// First activity start / last activity end, minutes from this day's midnight.
    pub first_start: Option<i32>,
    pub last_end: Option<i32>,
    /// Raw activity blocks, for display.
    pub segments: Vec<Interval>,
    /// The blocks joined into the periods actually worked (what the card shows for the day).
    pub card_intervals: Vec<Interval>,
    // --- Manual edits (kept when the activities change) ---
    /// Hours typed by hand: whole or half hours.
    pub override_hours: Option<f64>,
    pub override_night: Option<f64>,
    pub override_intervals: Option<Vec<Interval>>,
    pub code: Option<String>,
    /// The code is a planned leave put on a day the person is free anyway (weekend, holiday, day off of the cycle): the cell shows the
    /// code, but the day is not in the norm, so its hours do not go into the hours of the code, into the absences or into the overtime.
    /// Gone with the code.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub leave_on_free_day: bool,
    /// The person is not employed on this day (not hired yet, or already dismissed): the day stays empty and cannot be written on, and
    /// it is outside the norm of the month. Set by [`crate::roster`] from the dates on the staff list.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not_employed: bool,
    /// Remarks written beside some of the day's start/end times (a note on that cell in the Excel): for instance that time
    /// was added to the first departure. Only the mode that has something to say sets them.
    pub time_notes: Vec<TimeNote>,
    // --- Computed by `rules::recompute` ---
    pub hours: Option<f64>,
    pub night_hours: Option<f64>,
    pub night_min: i32,
    pub intervals: Vec<Interval>,
    /// Grid mode only: the intervals do not add up to the hours (hand-edited day).
    pub intervals_off: bool,
    /// Only in the one Excel file, for somebody who worked on the tachograph (category D) and on Categoria B: the part of `hours`
    /// that comes from the tachograph. Its cell is yellow; what Categoria B adds keeps the colour of the day. See [`crate::combined`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tachograph_hours: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub worked: f64,
    /// Working minutes on the tachograph for the days written in the sheet.
    pub card_min: i32,
    pub night: f64,
    pub not_worked: f64,
    /// Hours per code, aligned with [`CODES`].
    pub code_hours: Vec<f64>,
    pub overtime: f64,
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

/// The days of a month, each with its weekday and whether it is a weekend or a legal holiday.
pub fn month_days(year: i32, month: u32) -> Vec<DayInfo> {
    (1..=calendar::days_in_month(year, month))
        .filter_map(|d| NaiveDate::from_ymd_opt(year, month, d))
        .map(|date| DayInfo {
            day: date.day(),
            weekday: calendar::weekday_short(date).to_string(),
            weekend: calendar::is_weekend(date),
            holiday: calendar::holiday_name(date).map(str::to_string),
        })
        .collect()
}

/// Builds the timesheet for the month covered by the reports.
pub fn build(reports: Vec<DriverReport>, settings: &Settings) -> Option<Timesheet> {
    let (year, month) = report_month(&reports)?;
    let n_days = calendar::days_in_month(year, month);

    let days = month_days(year, month);

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
                group: Group::default(),
                norm_hours: None,
                activities: report.segments,
                applied_leaves: Vec::new(),
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

/// Takes the PDF `source` out of the sheet: every row read from it goes away (the ones that were edited too), the rows added by
/// hand stay, and a driver that the PDF left with no rows goes too, unless something was typed on his days. A row that two PDFs
/// both had belongs to the one read first, so it goes away with that one.
pub fn remove_source(sheet: &mut Timesheet, source: &str, settings: &Settings) {
    if source.is_empty() {
        return; // the rows with no PDF are the ones added by hand
    }
    sheet.sources.retain(|name| name != source);
    let typed = |day: &DayEntry| day.override_hours.is_some() || day.override_night.is_some() || day.override_intervals.is_some() || day.code.is_some();
    sheet.employees.retain_mut(|employee| {
        let before = employee.activities.len();
        employee.activities.retain(|row| row.source != source);
        let lost_rows = employee.activities.len() < before;
        !(lost_rows && employee.activities.is_empty() && !employee.days.iter().any(typed))
    });
    rules::recompute(sheet, settings);
}

/// Puts newly read PDFs (`fresh`) where they belong. The sheet on screen takes them when it is the same month; otherwise
/// the month they cover is looked up among the saved ones (`saved`), so that reading one more PDF of September while
/// October is open adds to the September already kept, with its edits, instead of starting it again. A month nobody has
/// kept starts as `fresh`. PDFs that the kept month already has change nothing. A kept month that cannot be read is an
/// error, never "no month": starting over would write on top of the file that is still there.
pub fn combine(
    previous: Option<Timesheet>,
    fresh: Timesheet,
    settings: &Settings,
    saved: impl FnOnce(i32, u32) -> crate::Result<Option<Timesheet>>,
) -> crate::Result<Timesheet> {
    let base = match previous {
        Some(open) if (open.year, open.month) == (fresh.year, fresh.month) => Some(open),
        _ => saved(fresh.year, fresh.month)?,
    };
    Ok(match base {
        None => fresh,
        Some(base) if fresh.sources.iter().all(|source| base.sources.contains(source)) => base,
        Some(base) => merge(base, fresh, settings),
    })
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
        sheet.employees[0].days[0].override_hours = Some(9.0);

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
        assert_eq!(popescu.days[0].override_hours, Some(9.0), "manual hours survive");
        assert_eq!(popescu.days[1].hours, Some(4.0));
        assert_eq!(merged.sources, vec!["a.pdf".to_string(), "b.pdf".to_string()]);
    }

    #[test]
    fn a_pdf_can_be_taken_out_again() {
        let s = Settings::default();
        let first = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        let second = build(
            vec![report("b.pdf", "Popescu, Ion", &[(9, 2, 480, 720)]), report("b.pdf", "Ionescu, Vasile", &[(9, 3, 480, 960)]), report("b.pdf", "Marin, Dan", &[(9, 4, 480, 960)])],
            &s,
        )
        .unwrap();
        let mut sheet = merge(first, second, &s);
        // Something typed by hand on a driver who has rows only in b.pdf, a row added by hand, and a row of b.pdf that was edited.
        sheet.employees.iter_mut().find(|e| e.name == "MARIN DAN").unwrap().days[3].override_hours = Some(9.0);
        let popescu = sheet.employees.iter_mut().find(|e| e.name == "POPESCU ION").unwrap();
        popescu.activities.push(Segment::manual(NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(), 480, 600));
        popescu.activities.iter_mut().find(|a| a.source == "b.pdf").unwrap().manual = true;
        popescu.days[0].override_hours = Some(7.0);

        remove_source(&mut sheet, "b.pdf", &s);

        assert_eq!(sheet.sources, ["a.pdf"]);
        let names: Vec<&str> = sheet.employees.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["MARIN DAN", "POPESCU ION"], "Ionescu had nothing but b.pdf, Marin has something typed");
        let popescu = sheet.employees.iter().find(|e| e.name == "POPESCU ION").unwrap();
        assert_eq!(popescu.activities.len(), 2, "a.pdf's row and the one added by hand");
        assert!(popescu.activities.iter().all(|a| a.source != "b.pdf"));
        assert_eq!(popescu.days[0].override_hours, Some(7.0), "what was typed stays");
        assert_eq!(popescu.days[1].hours, None, "the day of b.pdf has no hours any more");
        assert_eq!(sheet.employees[0].days[3].override_hours, Some(9.0));

        remove_source(&mut sheet, "", &s);
        assert_eq!(sheet.employees.iter().find(|e| e.name == "POPESCU ION").unwrap().activities.len(), 2, "rows with no PDF are not a PDF");
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
    fn pdfs_of_a_kept_month_are_added_to_it_even_when_another_month_is_open() {
        let s = Settings::default();
        let mut sep = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        sep.employees[0].days[0].override_hours = Some(9.0);
        let oct = build(vec![report("c.pdf", "Popescu, Ion", &[(10, 1, 480, 960)])], &s).unwrap();
        let more_sep = build(vec![report("b.pdf", "Popescu, Ion", &[(9, 2, 480, 720)])], &s).unwrap();
        let kept = sep.clone();
        let merged = combine(Some(oct), more_sep, &s, move |year, month| Ok(((year, month) == (2026, 9)).then_some(kept))).unwrap();
        assert_eq!((merged.year, merged.month), (2026, 9));
        assert_eq!(merged.sources, ["a.pdf", "b.pdf"]);
        assert_eq!(merged.employees[0].days[0].override_hours, Some(9.0), "what was typed in the kept month stays");
        assert_eq!(merged.employees[0].days[1].hours, Some(4.0));
    }

    #[test]
    fn pdfs_a_kept_month_already_has_change_nothing_and_a_new_month_starts_from_the_pdfs() {
        let s = Settings::default();
        let mut sep = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        sep.employees[0].days[0].override_hours = Some(9.0);
        sep.warnings.push("o problemă".into());
        let again = build(vec![report("a.pdf", "Popescu, Ion", &[(9, 1, 480, 960)])], &s).unwrap();
        let kept = sep.clone();
        let same = combine(None, again, &s, move |_, _| Ok(Some(kept))).unwrap();
        assert_eq!(same.warnings, ["o problemă"], "nothing was merged, so nothing was repeated");
        assert_eq!(same.employees[0].days[0].override_hours, Some(9.0));

        let nov = build(vec![report("n.pdf", "Popescu, Ion", &[(11, 3, 480, 960)])], &s).unwrap();
        let fresh = combine(Some(sep), nov, &s, |_, _| Ok(None)).unwrap();
        assert_eq!((fresh.year, fresh.month, fresh.sources.len()), (2026, 11, 1));

        let dec = build(vec![report("d.pdf", "Popescu, Ion", &[(12, 3, 480, 960)])], &s).unwrap();
        let unreadable = combine(None, dec, &s, |_, _| Err(crate::Error::new("fișier stricat")));
        assert!(unreadable.is_err(), "an unreadable kept month stops the reading instead of being overwritten");
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
