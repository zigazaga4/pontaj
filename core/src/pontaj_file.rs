//! Reads back the first sheet of the files the app writes ("Pontaj MM.YYYY", the foaie colectivă): who is on it and what each
//! day of each person says. Nothing is imported into the app from it except the hours of the Categoria B drivers, when the
//! file was changed outside the app (see [`crate::catb::reconcile`]).
//!
//! The layout is the one [`crate::excel`] writes: the day numbers on the second header row from column E, then every person as
//! a block of rows (the name, merged over the block, in column B), the hours of the days on the first row of the block, down to
//! the row that says TOTAL. A formula cell is read as the result the file has cached (what Excel last calculated).

use std::path::Path;

use calamine::{open_workbook, Data, Range, Reader, Xlsx};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

use crate::calendar::days_in_month;
use crate::catb::excel::{DETAIL_DATE_COL, DETAIL_END_COL, DETAIL_FROM_COL, DETAIL_HEADERS, DETAIL_KIND_COL, DETAIL_SHEET, DETAIL_START_COL, DETAIL_TO_COL};
use crate::excel::{FIRST_DATA_ROW, FIRST_DAY_COL as FIRST_DAY_COL_U16, HEADER_ROW};
use crate::timesheet::CODES;
use crate::{Error, Result};

/// The first day column, as the reader counts columns.
const FIRST_DAY_COL: u32 = FIRST_DAY_COL_U16 as u32;

/// What one day of one person says in the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// Nothing, or zero hours.
    Empty,
    Hours(f64),
    /// A code of the payroll file (CO, CM...), in capitals.
    Code(String),
    /// Something that is neither hours nor a code: the text as written.
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub name: String,
    /// One cell for each day of the month (the first is the 1st).
    pub days: Vec<Cell>,
}

/// One line of the sheet "Detaliu": a trip, a waiting, the time added... as the file says it.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailLine {
    /// The day the line counts on (column "Data").
    pub date: NaiveDate,
    /// The "Tip" as written: `Eco`, `Privată`, `Deplasare`, `Eco (scoasă)`, `Staționare`...
    pub kind: String,
    /// "Plecare" and "Sosire" as moments, or `None` when the cell is empty or is not a time. A time written `HH:MM (+1)` is
    /// on the next day of the line's date; one without a sign is on the line's date, except an arrival that would then be
    /// before the start (a time typed without the sign), which is the next day.
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    pub from: String,
    pub to: String,
}

/// The lines of one driver (the band with his name) in the sheet "Detaliu", in the order of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub name: String,
    pub lines: Vec<DetailLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub year: i32,
    pub month: u32,
    pub people: Vec<Person>,
    /// What the sheet "Detaliu" says (empty when the file has no such sheet).
    pub details: Vec<Detail>,
}

/// `Pontaj 11.2026` → (2026, 11).
fn month_of_sheet(name: &str) -> Option<(i32, u32)> {
    let (month, year) = name.trim().strip_prefix("Pontaj ")?.trim().split_once('.')?;
    let (month, year): (u32, i32) = (month.trim().parse().ok()?, year.trim().parse().ok()?);
    ((1..=12).contains(&month) && (2000..=2100).contains(&year)).then_some((year, month))
}

/// One cell of a day: hours (a number, or a number written as text), a code, or nothing.
fn cell_of(data: Option<&Data>) -> Cell {
    let hours = |value: f64| if value.is_finite() && value > 0.0 { Cell::Hours(value) } else if value == 0.0 { Cell::Empty } else { Cell::Other(value.to_string()) };
    match data {
        None | Some(Data::Empty) => Cell::Empty,
        Some(Data::Int(value)) => hours(*value as f64),
        Some(Data::Float(value)) => hours(*value),
        // A time typed as 8:00 is a fraction of a day for Excel: eight hours.
        Some(Data::DateTime(value)) => hours(value.as_f64() * 24.0),
        Some(Data::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                Cell::Empty
            } else if let Ok(value) = text.replace(',', ".").parse::<f64>() {
                hours(value)
            } else if CODES.iter().any(|(code, _)| code.eq_ignore_ascii_case(text)) {
                Cell::Code(text.to_uppercase())
            } else {
                Cell::Other(text.to_string())
            }
        }
        Some(other) => Cell::Other(other.to_string()),
    }
}

/// The text of a cell, trimmed (empty when there is none).
fn text_of(range: &Range<Data>, row: u32, col: u32) -> String {
    match range.get_value((row, col)) {
        Some(Data::String(text)) => text.trim().to_string(),
        _ => String::new(),
    }
}

/// How many days the header names: 1, 2, 3... one after the other from the first day column.
fn days_in_header(range: &Range<Data>) -> usize {
    let number = |day: u32| match range.get_value((HEADER_ROW + 1, FIRST_DAY_COL + day)) {
        Some(Data::Int(n)) => Some(*n as f64),
        Some(Data::Float(n)) => Some(*n),
        _ => None,
    };
    (0..31u32).take_while(|day| number(*day) == Some(f64::from(day + 1))).count()
}

/// `05:00`, `5:00` or `05:00 (+1)` / `05:00 (-1)`: the time, the days it is away from the line's date, and whether the text said so.
fn parse_clock(text: &str) -> Option<(NaiveTime, i64, bool)> {
    let text = text.trim();
    let (time, days) = match text.split_once('(') {
        Some((time, sign)) => (time.trim(), Some(sign.trim().strip_suffix(')')?.trim().trim_start_matches('+').parse::<i64>().ok()?)),
        None => (text, None),
    };
    let mut parts = time.split(':');
    let (hour, minute): (u32, u32) = (parts.next()?.trim().parse().ok()?, parts.next()?.trim().parse().ok()?);
    Some((NaiveTime::from_hms_opt(hour, minute, 0)?, days.unwrap_or(0), days.is_some()))
}

/// The moment a "Plecare"/"Sosire" cell says on `date`, and whether the cell gave the day itself (`(+1)`). The cell is text as
/// the app writes it, or a time Excel made of what was typed (a fraction of a day).
fn clock_of(data: Option<&Data>, date: NaiveDate) -> Option<(NaiveDateTime, bool)> {
    let (time, days, explicit) = match data? {
        Data::String(text) => parse_clock(text)?,
        Data::DateTime(value) => (time_of_fraction(value.as_f64())?, 0, false),
        Data::Float(value) => (time_of_fraction(*value)?, 0, false),
        _ => return None,
    };
    Some(((date + Duration::days(days)).and_time(time), explicit))
}

fn time_of_fraction(value: f64) -> Option<NaiveTime> {
    let minutes = (value.fract().abs() * 24.0 * 60.0).round() as u32;
    NaiveTime::from_hms_opt(minutes / 60 % 24, minutes % 60, 0).filter(|_| value.is_finite())
}

/// The day of a cell of "Data": a date as Excel keeps it (days since 30.12.1899).
fn date_of(data: Option<&Data>) -> Option<NaiveDate> {
    let serial = match data? {
        Data::DateTime(value) => value.as_f64(),
        Data::Float(value) => *value,
        Data::Int(value) => *value as f64,
        _ => return None,
    };
    (serial.is_finite() && serial >= 1.0).then(|| NaiveDate::from_ymd_opt(1899, 12, 30).map(|zero| zero + Duration::days(serial.floor() as i64))).flatten()
}

/// The lines of every driver of the sheet "Detaliu": under the row of headings, a band with the name of the driver opens each
/// block, the lines follow (a date in the first column), and a row "Total ore …" closes it.
fn details_of(range: &Range<Data>) -> Vec<Detail> {
    let Some((first, last)) = range.start().zip(range.end()).map(|(start, end)| (start.0, end.0)) else { return Vec::new() };
    let (date_head, kind_head) = (DETAIL_HEADERS[DETAIL_DATE_COL as usize], DETAIL_HEADERS[DETAIL_KIND_COL as usize]);
    let Some(header) = (first..=last).find(|row| text_of(range, *row, DETAIL_DATE_COL.into()) == date_head && text_of(range, *row, DETAIL_KIND_COL.into()) == kind_head) else { return Vec::new() };
    let mut details: Vec<Detail> = Vec::new();
    for row in header + 1..=last {
        if let Some(date) = date_of(range.get_value((row, DETAIL_DATE_COL.into()))) {
            let Some(current) = details.last_mut() else { continue };
            let start = clock_of(range.get_value((row, DETAIL_START_COL.into())), date).map(|(at, _)| at);
            let end = clock_of(range.get_value((row, DETAIL_END_COL.into())), date).map(|(at, explicit)| match start {
                // An arrival typed with no sign that comes out before the departure is after midnight.
                Some(start) if !explicit && at < start => at + Duration::days(1),
                _ => at,
            });
            current.lines.push(DetailLine {
                date,
                kind: text_of(range, row, DETAIL_KIND_COL.into()),
                start,
                end,
                from: text_of(range, row, DETAIL_FROM_COL.into()),
                to: text_of(range, row, DETAIL_TO_COL.into()),
            });
        } else {
            let name = text_of(range, row, DETAIL_DATE_COL.into());
            if !name.is_empty() && !name.to_lowercase().starts_with("total") {
                details.push(Detail { name, lines: Vec::new() });
            }
        }
    }
    details
}

/// Reads the sheet "Pontaj MM.YYYY" of an Excel file written by the app, with its changes if somebody made some, and the
/// lines of the sheet "Detaliu" when the file has it.
pub fn read(path: &Path) -> Result<File> {
    let mut workbook: Xlsx<_> = open_workbook(path).map_err(|e| Error::new(format!("Nu pot deschide fișierul Excel (este un .xlsx? este deschis în altă parte?): {e}")))?;
    let Some((name, year, month)) = workbook.sheet_names().into_iter().find_map(|name| month_of_sheet(&name).map(|(year, month)| (name, year, month))) else {
        return Err(Error::new("În fișier nu este foaia „Pontaj LL.AAAA”. Alege un Excel exportat din aplicație."));
    };
    let range = workbook.worksheet_range(&name).map_err(|e| Error::new(format!("Nu pot citi foaia „{name}”: {e}")))?;

    let days = days_in_month(year, month) as usize;
    if days_in_header(&range) != days {
        return Err(Error::new(format!("Foaia „{name}” nu are zilele lunii pe rândul lor (1, 2, 3…): nu pare exportată din aplicație sau a fost mutată.")));
    }

    let last = range.end().map_or(FIRST_DATA_ROW, |(row, _)| row);
    // The people end at the row that says TOTAL (the names after it, if any, are not people).
    let total = (FIRST_DATA_ROW..=last).find(|row| text_of(&range, *row, 0).eq_ignore_ascii_case("TOTAL")).unwrap_or(last + 1);
    let people = (FIRST_DATA_ROW..total.min(last + 1))
        .filter_map(|row| {
            let name = text_of(&range, row, 1);
            (!name.is_empty()).then(|| Person { name, days: (0..days as u32).map(|day| cell_of(range.get_value((row, FIRST_DAY_COL + day)))).collect() })
        })
        .collect();
    // The detail of the trips is optional: a file without it still gives the hours of the days.
    let details = workbook.worksheet_range(DETAIL_SHEET).map(|range| details_of(&range)).unwrap_or_default();
    Ok(File { year, month, people, details })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{self, Plan, StaffMember};
    use crate::settings::Settings;
    use crate::timesheet::Group;

    fn person(name: &str) -> StaffMember {
        StaffMember { name: name.into(), function: "Șofer".into(), group: Group::Office, plan: Plan::standard_weekdays(), note: String::new() }
    }

    fn write(timesheet: &crate::Timesheet, name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pontaj-file-{name}-{}.xlsx", std::process::id()));
        let mut workbook = rust_xlsxwriter::Workbook::new();
        crate::excel::pontaj_sheet(workbook.add_worksheet(), timesheet, &Settings::default(), None).unwrap();
        workbook.save(&path).unwrap();
        path
    }

    #[test]
    fn the_sheet_name_gives_the_month() {
        assert_eq!(month_of_sheet("Pontaj 11.2026"), Some((2026, 11)));
        assert_eq!(month_of_sheet("Pontaj 1.2026"), Some((2026, 1)));
        assert_eq!(month_of_sheet("Verificare"), None);
        assert_eq!(month_of_sheet("Pontaj 13.2026"), None);
    }

    #[test]
    fn a_cell_is_hours_a_code_or_nothing() {
        assert_eq!(cell_of(Some(&Data::Float(8.0))), Cell::Hours(8.0));
        assert_eq!(cell_of(Some(&Data::Int(12))), Cell::Hours(12.0));
        assert_eq!(cell_of(Some(&Data::String(" 7,5 ".into()))), Cell::Hours(7.5), "hours written as text, with a comma");
        assert_eq!(cell_of(Some(&Data::String("co".into()))), Cell::Code("CO".into()));
        assert_eq!(cell_of(Some(&Data::String(" ".into()))), Cell::Empty, "the blank text a formula gives for no hours");
        assert_eq!(cell_of(Some(&Data::Float(0.0))), Cell::Empty);
        assert_eq!(cell_of(Some(&Data::String("bolnav".into()))), Cell::Other("bolnav".into()));
        assert_eq!(cell_of(None), Cell::Empty);
    }

    #[test]
    fn the_people_and_their_days_are_read_back() {
        let settings = Settings::default();
        let mut sheet = roster::build_sheet(&[person("Zaharia Ion"), person("Andrei Vasile")], Group::Office, 2026, 11, &settings);
        // Mon 2 Nov: 10 hours, Tue 3: a leave, Wed 4: nothing.
        let zaharia = sheet.employees.iter_mut().find(|e| e.name == "ZAHARIA ION").unwrap();
        zaharia.days[1].override_hours = Some(10.0);
        zaharia.days[2].code = Some("CO".into());
        zaharia.days[3].override_hours = Some(0.0);
        crate::recompute(&mut sheet, &settings);
        let path = write(&sheet, "back");

        let file = read(&path).expect("the file is read");
        std::fs::remove_file(&path).ok();
        assert_eq!((file.year, file.month), (2026, 11));
        assert_eq!(file.people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["ANDREI VASILE", "ZAHARIA ION"], "in the order of the sheet");
        let first = file.people.iter().find(|p| p.name == "ZAHARIA ION").unwrap();
        assert_eq!(first.days.len(), 30);
        assert_eq!(first.days[0], Cell::Empty, "Sunday 1 Nov");
        assert_eq!(first.days[1], Cell::Hours(10.0));
        assert_eq!(first.days[2], Cell::Code("CO".into()));
        assert_eq!(first.days[3], Cell::Empty, "a day set to nothing");
        assert_eq!(first.days[4], Cell::Hours(8.0));
    }

    #[test]
    fn the_times_of_the_detail_sheet_are_read_with_their_day_sign() {
        let clock = |text: &str| parse_clock(text).map(|(time, days, explicit)| (time.format("%H:%M").to_string(), days, explicit));
        assert_eq!(clock("01:30 (+1)"), Some(("01:30".into(), 1, true)));
        assert_eq!(clock("23:30 (-1)"), Some(("23:30".into(), -1, true)));
        assert_eq!(clock(" 7:05 "), Some(("07:05".into(), 0, false)));
        assert_eq!(clock("fără oră"), None);
        // A time Excel made of what was typed is a fraction of a day.
        assert_eq!(time_of_fraction(0.5), NaiveTime::from_hms_opt(12, 0, 0));
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let (at, explicit) = clock_of(Some(&Data::String("01:30 (+1)".into())), day).unwrap();
        assert_eq!((at.to_string(), explicit), ("2026-11-04 01:30:00".to_string(), true));
        assert_eq!(date_of(Some(&Data::Float(46329.0))), Some(day), "3 Nov 2026 as Excel counts days");
    }

    #[test]
    fn a_file_without_the_pontaj_sheet_is_refused_with_a_reason() {
        let path = std::env::temp_dir().join(format!("pontaj-file-other-{}.xlsx", std::process::id()));
        let mut workbook = rust_xlsxwriter::Workbook::new();
        workbook.add_worksheet().set_name("Altceva").unwrap();
        workbook.save(&path).unwrap();
        let error = read(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).ok();
        assert!(error.contains("Pontaj LL.AAAA"), "{error}");
    }
}
