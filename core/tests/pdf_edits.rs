//! What the edits made on screen in the tachograph (PDF) mode reach in the exported workbook: a day set to zero hours, times
//! typed with a pause of their own, a day with a single typed interval, and a day taken back to the automatic values.

use calamine::{open_workbook, Data, Reader, Xlsx};
use chrono::NaiveDate;
use pontaj_core::pdf::{DriverReport, Segment};
use pontaj_core::timesheet::{build, Interval, Timesheet};
use pontaj_core::{recompute, write_xlsx, Settings};

const FIRST_DAY_COL: u32 = 4;
const FIRST_EMPLOYEE_ROW: u32 = 10;

fn hm(h: i32, m: i32) -> i32 {
    h * 60 + m
}

fn segment(day: u32, start: i32, end: i32) -> Segment {
    let mut seg = Segment::manual(NaiveDate::from_ymd_opt(2026, 9, day).unwrap(), start, end);
    seg.manual = false;
    seg.source = "raport.pdf".into();
    seg
}

fn sheet_of(days: &[(u32, i32, i32)], settings: &Settings) -> Timesheet {
    let report = DriverReport {
        name: "Testescu, Ion".into(),
        company: "EXEMPLU SRL".into(),
        location: String::new(),
        period: None,
        segments: days.iter().map(|&(d, s, e)| segment(d, s, e)).collect(),
        source: "raport.pdf".into(),
    };
    build(vec![report], settings).expect("a sheet")
}

/// The first sheet of the file as (row, column) → value, in the file's own coordinates.
fn read_first_sheet(path: &std::path::Path) -> std::collections::HashMap<(u32, u32), Data> {
    let mut book: Xlsx<_> = open_workbook(path).expect("the exported file opens");
    let name = book.sheet_names().first().cloned().expect("a sheet");
    let range = book.worksheet_range(&name).expect("the first sheet");
    let (r0, c0) = range.start().unwrap_or((0, 0));
    range.used_cells().map(|(r, c, v)| ((r0 + r as u32, c0 + c as u32), v.clone())).collect()
}

/// The rows of the only driver of a sheet for `day`: hours, night, then the start/end pairs.
struct DayCells<'a> {
    cells: &'a std::collections::HashMap<(u32, u32), Data>,
    col: u32,
}

impl DayCells<'_> {
    fn at(&self, offset: u32) -> Option<f64> {
        match self.cells.get(&(FIRST_EMPLOYEE_ROW + offset, self.col)) {
            Some(Data::Float(f)) => Some(*f),
            Some(Data::Int(i)) => Some(*i as f64),
            Some(Data::DateTime(d)) => Some(d.as_f64()), // the time cells have a time format
            _ => None,
        }
    }
    fn hours(&self) -> Option<f64> {
        self.at(0)
    }
    fn night(&self) -> Option<f64> {
        self.at(1)
    }
    /// Start or end `n` (0 = start 1, 1 = end 1, 2 = start 2, 3 = end 2) in minutes.
    fn time(&self, n: u32) -> Option<i32> {
        self.at(2 + n).map(|v| (v * 1440.0).round() as i32)
    }
}

#[test]
fn zero_days_typed_times_and_a_reset_reach_the_workbook() {
    let settings = Settings::default();
    let mut sheet = sheet_of(&[(2, hm(8, 0), hm(16, 0)), (3, hm(7, 0), hm(15, 0)), (4, hm(7, 0), hm(15, 0)), (7, hm(7, 0), hm(15, 0))], &settings);
    let auto_hours = (sheet.employees[0].days[1].hours, sheet.employees[0].days[2].hours);
    assert_eq!(auto_hours, (Some(8.0), Some(8.0)));

    // As the screen sends them. The 2nd: zero hours. The 3rd: times with a 2 h pause (the hours follow: 7). The 4th: one
    // interval of 4 h. The 7th was changed and then taken back to the automatic values.
    {
        let days = &mut sheet.employees[0].days;
        days[1].override_hours = Some(0.0);
        days[2].override_intervals = Some(vec![Interval { start: hm(8, 0), end: hm(11, 30) }, Interval { start: hm(13, 30), end: hm(17, 0) }]);
        days[2].override_hours = Some(7.0);
        days[3].override_intervals = Some(vec![Interval { start: hm(6, 0), end: hm(10, 0) }]);
        days[3].override_hours = Some(4.0);
        days[6].override_hours = Some(3.0);
    }
    recompute(&mut sheet, &settings);
    assert_eq!(sheet.employees[0].days[6].hours, Some(3.0));
    sheet.employees[0].days[6].override_hours = None; // the ↺ button of the screen
    recompute(&mut sheet, &settings);

    let day = &sheet.employees[0].days;
    assert_eq!((day[1].hours, day[1].intervals.len(), day[1].night_hours), (Some(0.0), 0, None), "zero keeps the day, with no times");
    assert_eq!((day[2].hours, day[3].hours, day[6].hours), (Some(7.0), Some(4.0), Some(8.0)));
    assert!(!day[2].intervals_off && !day[3].intervals_off, "the typed times add up to the hours");
    assert_eq!(sheet.employees[0].totals.worked, 8.0 + 7.0 + 4.0, "the zero day adds nothing: 8 (the 7th) + 7 + 4");

    let path = std::env::temp_dir().join("pontaj_pdf_edits.xlsx");
    write_xlsx(&sheet, &settings, &path).expect("workbook written");
    let cells = read_first_sheet(&path);
    let day_cells = |d: u32| DayCells { cells: &cells, col: FIRST_DAY_COL + d - 1 };

    let zero = day_cells(2);
    assert_eq!((zero.hours(), zero.night(), zero.time(0), zero.time(2)), (Some(0.0), None, None, None), "a 0 is written, not an empty cell");

    let pause = day_cells(3);
    assert_eq!(pause.hours(), Some(7.0));
    assert_eq!((pause.time(0), pause.time(1), pause.time(2), pause.time(3)), (Some(hm(8, 0)), Some(hm(11, 30)), Some(hm(13, 30)), Some(hm(17, 0))), "the pause of 2 h is the gap between the pairs");

    let single = day_cells(4);
    assert_eq!((single.hours(), single.time(0), single.time(1), single.time(2)), (Some(4.0), Some(hm(6, 0)), Some(hm(10, 0)), None));

    let reset = day_cells(7);
    assert_eq!(reset.hours(), Some(8.0), "back to the automatic hours of the card");
    assert!(reset.time(0).is_some() && reset.time(3).is_some(), "and the automatic two intervals");
}
