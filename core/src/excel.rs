//! Writes the "Foaie colectivă de prezență (pontaj)" workbook, laid out like the payroll team's own file
//! (the October 2026 one): company and legend on top, the title, the month and the days as columns, six rows per
//! employee (hours, night hours, two start/end pairs; more pairs when a day of the employee has more intervals) and
//! live formulas for every total.
//! [`pontaj_sheet`] draws that one worksheet from a [`Timesheet`]; the tachograph mode and Categoria B both use it.

use std::path::Path;

use chrono::NaiveDate;
use rust_xlsxwriter::{Color, ColNum, Format, FormatAlign, FormatBorder, Formula, Note, RowNum, Workbook, Worksheet, XlsxError};

use crate::calendar::month_name;
use crate::settings::{IntervalMode, NightSource, Settings};
use crate::time::{fmt_clock, fmt_duration, DAY};
use crate::timesheet::{DayEntry, Employee, Interval, Timesheet, CODES, OLP};
use crate::{Error, Result};

/// Row of the first header line (Excel row 9); the days numbers are on the next one.
const HEADER_ROW: RowNum = 8;
const FIRST_DATA_ROW: RowNum = 10;
/// The hours and the night hours, the two rows on top of every employee; the start/end pairs come under them.
const TOP_ROWS: RowNum = 2;
/// Start/end pairs of an employee in the payroll file; a day with more intervals gives the employee more pairs.
const MIN_SLOTS: usize = 2;
/// Column E: A is the number, B the name, C the job, D the department (left empty, as in the payroll file).
const FIRST_DAY_COL: ColNum = 4;

pub(crate) const HEADER_FILL: u32 = 0xD9E1F2;
pub(crate) const OFF_FILL: u32 = 0xEDEDED;
pub(crate) const HOLIDAY_FILL: u32 = 0xFCE4D6;
pub(crate) const TOTAL_FILL: u32 = 0xFFF2CC;
/// The green of weekends, holidays and the month header in the payroll file (theme accent 3, 40% lighter).
const WEEKEND_FILL: u32 = 0xC3D69B;
/// What is calculated, and the names.
const YELLOW: u32 = 0xFFFF00;
const CODE_FONT: u32 = 0xFF0000;
/// Absence codes on the "Verificare" sheet.
const CODE_FILL: u32 = 0xE2EFDA;

/// The legend in the top right corner, word for word as in the payroll file.
const LEGEND: [&str; 6] = [
    "CFP- concediu fara plata",
    "OLP - ore libere plătite",
    "N - absente nemotivate",
    "S - ore suplimentare",
    "CP- concediu parental",
    "zlp - zi libera platita",
];

const MONTH_ABBREVIATIONS: [&str; 12] = ["IAN", "FEB", "MAR", "APR", "MAI", "IUN", "IUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// Widths of the day columns (day 1 to 31) as the payroll file has them.
const DAY_WIDTHS: [f64; 31] = [
    5.57, 4.71, 5.43, 4.86, 5.57, 5.57, 5.57, 4.86, 4.86, 4.71, 4.71, 4.71, 4.71, 5.57, 5.86, 4.71, 4.71, 4.86, 5.57, 4.86, 4.86, 4.86, 4.86, 4.86, 4.86, 4.86, 5.43, 5.43, 5.43, 5.43, 5.43,
];

/// Column positions derived from the number of days in the month.
struct Cols {
    last_day: ColNum,
    worked: ColNum,
    night: ColNum,
    not_worked: ColNum,
    first_code: ColNum,
    overtime: ColNum,
}

impl Cols {
    fn new(days: usize) -> Self {
        let last_day = FIRST_DAY_COL + days as ColNum - 1;
        let first_code = last_day + 4;
        Cols { last_day, worked: last_day + 1, night: last_day + 2, not_worked: last_day + 3, first_code, overtime: first_code + CODES.len() as ColNum }
    }
    fn code(&self, i: usize) -> ColNum {
        self.first_code + i as ColNum
    }
}

/// `A1`-style reference (0-based row/col).
pub(crate) fn cell(row: RowNum, col: ColNum) -> String {
    let mut name = String::new();
    let mut n = col as u32 + 1;
    while n > 0 {
        let rem = (n - 1) % 26;
        name.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    format!("{name}{}", row + 1)
}

pub(crate) fn range(row: RowNum, from: ColNum, to: ColNum) -> String {
    format!("{}:{}", cell(row, from), cell(row, to))
}

/// Start/end pairs written for an employee: the two of the payroll file, or as many as its busiest day has.
fn slots(emp: &Employee) -> usize {
    emp.days.iter().map(|d| d.intervals.len()).max().unwrap_or(0).max(MIN_SLOTS)
}

/// Rows of an employee on the sheet: hours, night hours and a start and an end row per pair.
fn rows_of(emp: &Employee) -> RowNum {
    TOP_ROWS + 2 * slots(emp) as RowNum
}

/// Reusable cell formats.
pub(crate) struct Styles {
    pub(crate) base: Format,
}

impl Styles {
    pub(crate) fn new() -> Self {
        let base = Format::new()
            .set_font_name("Arial Narrow")
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x808080));
        Styles { base }
    }
    /// The look of the pontaj sheet: Arial Narrow 10, centered, thin black borders.
    fn pontaj() -> Self {
        let base = Format::new()
            .set_font_name("Arial Narrow")
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin);
        Styles { base }
    }
    pub(crate) fn fill(&self, color: u32) -> Format {
        self.base.clone().set_background_color(Color::RGB(color))
    }
    pub(crate) fn header(&self) -> Format {
        self.fill(HEADER_FILL).set_font_name("Arial").set_bold().set_text_wrap()
    }
    pub(crate) fn day_header(&self, off: bool, holiday: bool) -> Format {
        let color = if holiday { HOLIDAY_FILL } else if off { OFF_FILL } else { HEADER_FILL };
        self.fill(color).set_bold().set_text_wrap()
    }
    pub(crate) fn day(&self, off: bool) -> Format {
        if off { self.fill(OFF_FILL) } else { self.base.clone() }
    }
    pub(crate) fn plain() -> Format {
        Format::new().set_font_name("Arial").set_font_size(10)
    }
}

/// Writes the timesheet; a second worksheet ("Verificare") lists, day by day,
/// what the tachograph shows next to what was written.
pub fn write_xlsx(sheet: &Timesheet, settings: &Settings, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    pontaj_sheet(workbook.add_worksheet(), sheet, settings, None).map_err(xlsx_err)?;
    build_audit(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    workbook.save(path).map_err(|e| Error::new(format!("Nu pot salva fișierul Excel: {e}")))
}

pub(crate) fn xlsx_err(e: XlsxError) -> Error {
    Error::new(format!("Eroare la generarea Excel: {e}"))
}

/// Gives the hours of a day as a formula instead of a number: `(employee index, day of the month) -> "=…"`, or `None`
/// for a day that stays a plain value. The cell keeps what the app computed as the cached result (a blank text when
/// the day has no hours: the formula gives an empty text then, since a formula cannot return an empty cell).
pub(crate) type HoursFormula<'a> = &'a dyn Fn(usize, u32) -> Option<String>;

/// The worksheet "Pontaj MM.YYYY": the payroll team's foaie colectivă, with a formula for every total.
pub(crate) fn pontaj_sheet(ws: &mut Worksheet, sheet: &Timesheet, settings: &Settings, hours_from: Option<HoursFormula>) -> std::result::Result<(), XlsxError> {
    let st = Styles::pontaj();
    let cols = Cols::new(sheet.days.len());
    let month = month_name(sheet.month);
    ws.set_name(format!("Pontaj {:02}.{}", sheet.month, sheet.year))?;

    write_title(ws, sheet, settings, &cols)?;
    write_header(ws, sheet, &st, &cols, month)?;

    let mut row = FIRST_DATA_ROW;
    let mut time_rows: Vec<RowNum> = Vec::new();
    for (i, employee) in sheet.employees.iter().enumerate() {
        write_employee(ws, sheet, settings, &st, &cols, row, i + 1, employee, hours_from)?;
        let rows = rows_of(employee);
        time_rows.extend(row + TOP_ROWS..row + rows);
        row += rows;
    }
    let total_row = row;
    write_totals(ws, sheet, &st, &cols, total_row)?;
    write_signatures(ws, settings, &cols, total_row + 3)?;
    layout(ws, &cols, &time_rows, total_row)?;
    Ok(())
}

/// Excel's number for a date (days since 1899-12-30), for cells that must stay real dates.
pub(crate) fn excel_date(year: i32, month: u32, day: u32) -> f64 {
    let epoch = NaiveDate::from_ymd_opt(1899, 12, 30).unwrap_or_default();
    NaiveDate::from_ymd_opt(year, month, day).map_or(0.0, |date| (date - epoch).num_days() as f64)
}

fn write_title(ws: &mut Worksheet, sheet: &Timesheet, settings: &Settings, cols: &Cols) -> std::result::Result<(), XlsxError> {
    let company = if settings.company_name.trim().is_empty() { sheet.company.to_uppercase() } else { settings.company_name.trim().to_string() };
    let bold = Styles::plain().set_bold().set_align(FormatAlign::Left);
    ws.merge_range(0, 0, 0, FIRST_DAY_COL - 1, &company, &bold)?;
    if !settings.cui.trim().is_empty() {
        ws.merge_range(1, 0, 1, FIRST_DAY_COL - 1, &format!("CUI: {}", settings.cui.trim()), &bold)?;
    }
    for (i, text) in LEGEND.iter().enumerate() {
        ws.write_string_with_format(i as RowNum, cols.night, *text, &Styles::plain())?;
    }

    let title = Styles::plain().set_bold().set_align(FormatAlign::Center).set_align(FormatAlign::VerticalCenter).set_text_wrap();
    ws.merge_range(6, 0, 6, cols.overtime, "FOAIE COLECTIVA DE PREZENTA (PONTAJ)", &title)?;

    // The month again, short, and as a date: the small helper line above the day columns of the payroll file.
    let narrow = Format::new().set_font_name("Arial Narrow").set_font_size(10).set_bold();
    ws.write_string_with_format(7, FIRST_DAY_COL, MONTH_ABBREVIATIONS[(sheet.month as usize + 11) % 12], &narrow)?;
    let helper = narrow.clone().set_align(FormatAlign::Center).set_num_format("mmm-yy");
    ws.merge_range(7, FIRST_DAY_COL + 1, 7, FIRST_DAY_COL + 9, "", &helper)?;
    ws.write_number_with_format(7, FIRST_DAY_COL + 1, excel_date(sheet.year, sheet.month, 1), &helper)?;
    Ok(())
}

fn write_header(ws: &mut Worksheet, sheet: &Timesheet, st: &Styles, cols: &Cols, month: &str) -> std::result::Result<(), XlsxError> {
    let (r1, r2) = (HEADER_ROW, HEADER_ROW + 1);
    let head = st.base.clone().set_font_name("Arial").set_text_wrap();
    let turned = head.clone().set_rotation(90);
    let narrow_turned = st.base.clone().set_text_wrap().set_rotation(90);
    ws.merge_range(r1, 0, r2, 0, "Nr crt", &turned)?;
    ws.merge_range(r1, 1, r2, 1, "Numele si prenumele", &head)?;
    ws.merge_range(r1, 2, r2, 2, "Meseria sau functia", &turned)?;
    ws.merge_range(r1, 3, r2, 3, "Departament", &narrow_turned.clone().set_align(FormatAlign::Left))?;
    // The month, over all the days; each day is only its number (weekends and holidays in green).
    ws.merge_range(r1, FIRST_DAY_COL, r1, cols.last_day, &month.to_uppercase(), &st.fill(WEEKEND_FILL).set_bold().set_text_wrap())?;
    for (i, day) in sheet.days.iter().enumerate() {
        let fmt = if day.is_off() { st.fill(WEEKEND_FILL) } else { st.base.clone() };
        ws.write_number_with_format(r2, FIRST_DAY_COL + i as ColNum, f64::from(day.day), &fmt.set_text_wrap())?;
    }
    ws.merge_range(r1, cols.worked, r2, cols.worked, "Total ore lucrate", &narrow_turned)?;
    ws.write_string_with_format(r1, cols.night, "din care:", &head)?;
    ws.write_string_with_format(r2, cols.night, "ore de noapte", &turned)?;
    ws.merge_range(r1, cols.not_worked, r2, cols.not_worked, "Total ore nelucrate", &turned)?;
    ws.merge_range(r1, cols.first_code, r1, cols.code(CODES.len() - 1), "din care:", &head)?;
    for (i, (_, label)) in CODES.iter().enumerate() {
        ws.write_string_with_format(r2, cols.code(i), *label, &turned)?;
    }
    ws.merge_range(r1, cols.overtime, r2, cols.overtime, "ORE SUPL.", &turned)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_employee(
    ws: &mut Worksheet,
    sheet: &Timesheet,
    settings: &Settings,
    st: &Styles,
    cols: &Cols,
    row: RowNum,
    nr: usize,
    emp: &Employee,
    hours_from: Option<HoursFormula>,
) -> std::result::Result<(), XlsxError> {
    let rows = rows_of(emp);
    let times = 2 * slots(emp);
    let last = row + rows - 1;

    // Number and name over all the rows; the job over the first two (hours and night), then the labels of the times.
    let numbered = st.base.clone().set_text_wrap();
    ws.merge_range(row, 0, last, 0, "", &numbered)?;
    ws.write_number_with_format(row, 0, nr as f64, &numbered)?;
    ws.merge_range(row, 1, last, 1, &emp.name, &st.fill(YELLOW).set_font_name("Arial").set_align(FormatAlign::Left).set_text_wrap())?;
    ws.merge_range(row, 2, row + 1, 2, &emp.function, &st.base.clone().set_font_size(9).set_text_wrap())?;
    let label = st.base.clone().set_font_size(9).set_italic().set_align(FormatAlign::Left).set_text_wrap();
    for k in 0..times {
        let text = if k % 2 == 0 { "Ora începerii" } else { "Ora terminării" };
        ws.write_string_with_format(row + TOP_ROWS + k as RowNum, 2, text, &label)?;
    }
    let department = st.base.clone().set_num_format("h:mm");
    for k in 0..rows {
        ws.write_blank(row + k, 3, &department)?;
    }

    for (i, (info, entry)) in sheet.days.iter().zip(&emp.days).enumerate() {
        let col = FIRST_DAY_COL + i as ColNum;
        let day = if info.is_off() { st.fill(WEEKEND_FILL) } else { st.base.clone() };
        let linked = hours_from.and_then(|f| f(nr - 1, info.day));
        match (&entry.code, entry.hours, linked) {
            (Some(code), _, _) => ws.write_string_with_format(row, col, code, &day.clone().set_bold().set_font_color(Color::RGB(CODE_FONT)))?,
            (None, hours, Some(text)) => {
                // The writer turns an empty result into "0": a blank text keeps non-calculating viewers from showing zeros.
                let result = Formula::new(text).set_result(hours.map_or_else(|| " ".to_string(), |h| h.to_string()));
                ws.write_formula_with_format(row, col, result, &day)?
            }
            (None, Some(h), None) => ws.write_number_with_format(row, col, h, &day)?,
            (None, None, None) => ws.write_blank(row, col, &day)?,
        };
        match entry.night_hours {
            Some(n) => ws.write_number_with_format(row + 1, col, n, &day)?,
            None => ws.write_blank(row + 1, col, &day)?,
        };
        let time_fmt = day.clone().set_num_format("h:mm");
        for k in 0..times {
            let r = row + TOP_ROWS + k as RowNum;
            let value = entry.intervals.get(k / 2).map(|iv| if k % 2 == 0 { iv.start } else { iv.end });
            match value {
                Some(min) => ws.write_number_with_format(r, col, min.rem_euclid(DAY) as f64 / DAY as f64, &time_fmt)?,
                None => ws.write_blank(r, col, &time_fmt)?,
            };
            // A remark about this very time (e.g. "an hour was added here: it was 12:00, now 11:00") sits on its cell.
            if let Some(note) = entry.time_notes.iter().find(|n| n.interval == k / 2 && n.end == (k % 2 == 1)).filter(|_| value.is_some()) {
                ws.insert_note(r, col, &Note::new(&note.text).add_author_prefix(false).set_width(300).set_height(80))?;
            }
        }
    }

    // Totals: a yellow block, one formula per cell with its cached result (hours, night and the rest sit on the rows
    // they are on in the payroll file: hours, the unworked time and the codes on the first row, the night on the second).
    let t = &emp.totals;
    let total_fmt = st.fill(YELLOW).set_text_wrap();
    for col in cols.worked..=cols.overtime {
        for k in 0..rows {
            ws.write_blank(row + k, col, &total_fmt)?;
        }
    }
    let hours_range = range(row, FIRST_DAY_COL, cols.last_day);
    let night_range = range(row + 1, FIRST_DAY_COL, cols.last_day);
    let worked_ref = cell(row, cols.worked);
    let not_worked_ref = cell(row, cols.not_worked);
    let olp_idx = CODES.iter().position(|(c, _)| *c == OLP).unwrap_or(CODES.len());
    let absences = format!(
        "SUM({},{})",
        range(row, cols.first_code, cols.code(olp_idx.saturating_sub(1))),
        range(row, cols.code(olp_idx + 1), cols.code(CODES.len() - 1))
    );

    let mut formulas: Vec<(ColNum, String, f64)> = vec![
        (cols.worked, format!("=SUM({hours_range})"), t.worked),
        (cols.night, format!("=SUM({night_range})"), t.night),
        (cols.not_worked, format!("=MAX(0,{}-{worked_ref})", sheet.norm_hours), t.not_worked),
        (cols.overtime, format!("=MAX(0,{worked_ref}+{absences}-{})", sheet.norm_hours), t.overtime),
    ];
    for (i, (code, _)) in CODES.iter().enumerate() {
        let value = t.code_hours.get(i).copied().unwrap_or(0.0);
        let formula = if i == olp_idx {
            format!("=MAX(0,{not_worked_ref}-{absences})")
        } else {
            format!("=COUNTIF({hours_range},\"{code}\")*{}", settings.hours_per_day)
        };
        formulas.push((cols.code(i), formula, value));
    }
    for (col, formula, value) in formulas {
        let bold = col == cols.worked || col == cols.night || col == cols.not_worked;
        let fmt = if bold { total_fmt.clone().set_bold() } else { total_fmt.clone() };
        let fmt = if col == cols.not_worked { fmt.set_num_format("0") } else { fmt };
        let at = if col == cols.night { row + 1 } else { row };
        ws.write_formula_with_format(at, col, Formula::new(formula).set_result(value.to_string()), &fmt)?;
    }
    Ok(())
}

fn write_totals(ws: &mut Worksheet, sheet: &Timesheet, st: &Styles, cols: &Cols, row: RowNum) -> std::result::Result<(), XlsxError> {
    let fmt = st.fill(YELLOW).set_bold();
    ws.merge_range(row, 0, row, cols.last_day, "TOTAL", &fmt)?;
    let mut summary: Vec<(ColNum, f64)> = vec![
        (cols.worked, sheet.employees.iter().map(|e| e.totals.worked).sum::<f64>()),
        (cols.night, sheet.employees.iter().map(|e| e.totals.night).sum::<f64>()),
        (cols.not_worked, sheet.employees.iter().map(|e| e.totals.not_worked).sum::<f64>()),
        (cols.overtime, sheet.employees.iter().map(|e| e.totals.overtime).sum::<f64>()),
    ];
    for i in 0..CODES.len() {
        summary.push((cols.code(i), sheet.employees.iter().map(|e| e.totals.code_hours.get(i).copied().unwrap_or(0.0)).sum::<f64>()));
    }
    for (col, value) in summary {
        let formula = if row > FIRST_DATA_ROW {
            format!("=SUM({}:{})", cell(FIRST_DATA_ROW, col), cell(row - 1, col))
        } else {
            "=0".to_string()
        };
        ws.write_formula_with_format(row, col, Formula::new(formula).set_result(value.to_string()), &fmt)?;
    }
    Ok(())
}

fn write_signatures(ws: &mut Worksheet, settings: &Settings, cols: &Cols, row: RowNum) -> std::result::Result<(), XlsxError> {
    let bold = Styles::plain().set_bold();
    let plain = Styles::plain();
    ws.write_string_with_format(row, 1, "Întocmit,", &bold)?;
    ws.write_string_with_format(row, cols.worked, "Verificat,", &bold)?;
    if !settings.prepared_by.trim().is_empty() {
        ws.write_string_with_format(row + 1, 1, settings.prepared_by.trim(), &plain)?;
    }
    if !settings.approved_by.trim().is_empty() {
        ws.write_string_with_format(row + 1, cols.worked, settings.approved_by.trim(), &plain)?;
    }
    ws.write_string_with_format(row + 3, 1, "Semnătura: ____________________", &plain)?;
    ws.write_string_with_format(row + 3, cols.worked, "Semnătura: ____________________", &plain)?;
    Ok(())
}

/// `time_rows`: the rows of the start/end times of every employee (lower than the others).
fn layout(ws: &mut Worksheet, cols: &Cols, time_rows: &[RowNum], total_row: RowNum) -> std::result::Result<(), XlsxError> {
    ws.set_column_width(0, 3.86)?;
    ws.set_column_width(1, 15.57)?;
    ws.set_column_width(2, 10.29)?;
    ws.set_column_width(3, 4.86)?;
    for col in FIRST_DAY_COL..=cols.last_day {
        ws.set_column_width(col, DAY_WIDTHS[(col - FIRST_DAY_COL) as usize % DAY_WIDTHS.len()])?;
    }
    ws.set_column_width(cols.worked, 5.71)?;
    for col in cols.night..=cols.code(CODES.len() - 1) {
        ws.set_column_width(col, 4.71)?;
    }
    ws.set_column_width(cols.overtime, 6.0)?;
    ws.set_row_height(6, 12.75)?;
    ws.set_row_height(7, 15.75)?;
    ws.set_row_height(HEADER_ROW, 38.25)?;
    ws.set_row_height(HEADER_ROW + 1, 64.5)?;
    for &r in time_rows {
        ws.set_row_height(r, 13.5)?;
    }
    ws.set_row_height(total_row, 18)?;

    ws.set_freeze_panes(FIRST_DATA_ROW, 3)?;
    ws.set_repeat_rows(HEADER_ROW, HEADER_ROW + 1)?;
    ws.set_landscape();
    ws.set_paper_size(8); // A3
    ws.set_print_fit_to_pages(1, 0);
    ws.set_print_center_horizontally(true);
    ws.set_margins(0.3, 0.3, 0.5, 0.5, 0.3, 0.3);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

// ---------------------------------------------------------------------------
// "Verificare" worksheet: tachograph vs. timesheet, one row per day.
// ---------------------------------------------------------------------------

const AUDIT_HEADERS: [&str; 12] = [
    "Șofer",
    "Data",
    "Ziua",
    "Pe card: lucrat (h:mm)",
    "Pe card: program",
    "Pontaj: ore / cod",
    "Pontaj: interval 1",
    "Pontaj: interval 2",
    "Pe card: noapte (h:mm)",
    "Pontaj: noapte (ore)",
    "Diferență pontaj − card (ore)",
    "Observații",
];
const AUDIT_WIDTHS: [f64; 12] = [24.0, 11.0, 6.0, 12.0, 24.0, 10.0, 15.0, 15.0, 12.0, 10.0, 14.0, 44.0];

/// The rules the numbers were produced with, one line each.
fn audit_rules(sheet: &Timesheet, settings: &Settings) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Ore pe zi: timpul lucrat (condus + alte activități), rotunjit în sus de la {} minute. Legea nu impune o regulă de rotunjire: este convenția firmei.",
            settings.round_up_from_min
        ),
        format!("Norma lunii: {} zile lucrătoare × {} ore = {} ore.", sheet.working_days, settings.hours_per_day, sheet.norm_hours),
    ];
    lines.push(match settings.interval_mode {
        IntervalMode::Grid => format!(
            "Ora de început și de sfârșit: pe grilă de {} minute, două intervale care însumează orele zilei (pauză de {} minute între ele).",
            settings.slot_min, settings.break_min
        ),
        IntervalMode::Card => "Ora de început și de sfârșit: orele reale de pe card (primul început – ultimul sfârșit).".to_string(),
    });
    let source = match settings.night_source {
        NightSource::Card => "coloana „Noapte” din raportul de tahograf (22:00–06:00)".to_string(),
        NightSource::Intervals => format!("intervalele scrise în pontaj, între {} și {}", settings.night_start, settings.night_end),
    };
    let threshold = if settings.night_threshold_min > 0 {
        format!("se trec doar nopțile de cel puțin {} (rotunjite ca orele de lucru)", fmt_duration(settings.night_threshold_min))
    } else {
        "se trec toate orele de noapte".to_string()
    };
    lines.push(format!("Ore de noapte: {source}; {threshold}."));
    lines
}

/// What was done by hand on a day, for the "Observații" column.
fn audit_remarks(entry: &DayEntry) -> String {
    let mut remarks: Vec<&str> = Vec::new();
    if entry.code.is_some() {
        remarks.push("absență trecută manual");
    } else if entry.override_hours.is_some() {
        remarks.push("ore trecute manual");
    } else if entry.worked_min == 0 && entry.hours.is_some() {
        remarks.push("fără date pe card");
    }
    if entry.override_intervals.is_some() {
        remarks.push("intervale trecute manual");
    }
    if entry.override_night.is_some() {
        remarks.push("noapte trecută manual");
    }
    if entry.intervals_off {
        remarks.push("intervalele nu însumează orele");
    }
    let mut text = remarks.join("; ");
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    text
}

fn intervals_text(intervals: &[Interval]) -> String {
    intervals.iter().map(|i| format!("{}–{}", fmt_clock(i.start), fmt_clock(i.end))).collect::<Vec<_>>().join(", ")
}

/// A day appears in the audit when the card or the timesheet has something for it.
fn audit_has_data(entry: &DayEntry) -> bool {
    entry.worked_min > 0 || entry.hours.is_some() || entry.code.is_some()
}

/// A duration as an Excel time value (shown `h:mm`); nothing for zero minutes.
pub(crate) fn write_duration(ws: &mut Worksheet, row: RowNum, col: ColNum, minutes: i32, fmt: &Format) -> std::result::Result<(), XlsxError> {
    if minutes > 0 {
        ws.write_number_with_format(row, col, minutes as f64 / DAY as f64, fmt)?;
    } else {
        ws.write_blank(row, col, fmt)?;
    }
    Ok(())
}

fn build_audit(ws: &mut Worksheet, sheet: &Timesheet, settings: &Settings) -> std::result::Result<(), XlsxError> {
    let st = Styles::new();
    let plain = Styles::plain();
    ws.set_name("Verificare")?;
    let title = Styles::plain().set_bold().set_font_size(14);
    ws.write_string_with_format(0, 0, "VERIFICARE PONTAJ FAȚĂ DE TAHOGRAF", &title)?;
    ws.write_string_with_format(
        1,
        0,
        format!("{} {} · pontajul din foaia colectivă, zi cu zi, față de orele lucrate pe card. Diferențele vin din rotunjire sau din valori trecute manual.", month_name(sheet.month), sheet.year),
        &plain,
    )?;
    let rules = audit_rules(sheet, settings);
    for (i, line) in rules.iter().enumerate() {
        ws.write_string_with_format(2 + i as RowNum, 0, line, &plain)?;
    }

    let header_row = 3 + rules.len() as RowNum + 1;
    let head = st.header();
    for (col, text) in AUDIT_HEADERS.iter().enumerate() {
        ws.write_string_with_format(header_row, col as ColNum, *text, &head)?;
    }
    ws.set_row_height(header_row, 42)?;

    let left = st.base.clone().set_align(FormatAlign::Left);
    let time = st.base.clone().set_num_format("[h]:mm");
    let diff_fmt = st.base.clone().set_num_format("+0.00;-0.00;0.00");
    let hours_fmt = st.base.clone().set_bold();
    let total = st.fill(TOTAL_FILL).set_bold();
    let total_time = total.clone().set_num_format("[h]:mm");
    let total_diff = total.clone().set_num_format("+0.00;-0.00;0.00");

    // The rows are numbered as Excel does (1-based) inside the formulas.
    let mut row = header_row + 1;
    for emp in &sheet.employees {
        let first = row;
        let (mut card_min, mut written, mut night, mut diff) = (0, 0.0_f64, 0.0_f64, 0.0_f64);
        for (info, entry) in sheet.days.iter().zip(&emp.days).filter(|(_, e)| audit_has_data(e)) {
            let off = info.is_off();
            let (left, time, hours_fmt) = if off {
                (left.clone().set_background_color(Color::RGB(OFF_FILL)), time.clone().set_background_color(Color::RGB(OFF_FILL)), hours_fmt.clone().set_background_color(Color::RGB(OFF_FILL)))
            } else {
                (left.clone(), time.clone(), hours_fmt.clone())
            };
            let day_fmt = if off { st.fill(OFF_FILL) } else { st.base.clone() };

            ws.write_string_with_format(row, 0, &emp.name, &left)?;
            ws.write_string_with_format(row, 1, format!("{:02}.{:02}.{}", info.day, sheet.month, sheet.year), &day_fmt)?;
            ws.write_string_with_format(row, 2, &info.weekday, &day_fmt)?;
            write_duration(ws, row, 3, entry.worked_min, &time)?;
            ws.write_string_with_format(row, 4, intervals_text(&entry.card_intervals), &left)?;
            match (&entry.code, entry.hours) {
                (Some(code), _) => ws.write_string_with_format(row, 5, code, &hours_fmt.clone().set_background_color(Color::RGB(CODE_FILL)))?,
                (None, Some(h)) => ws.write_number_with_format(row, 5, h, &hours_fmt)?,
                (None, None) => ws.write_blank(row, 5, &hours_fmt)?,
            };
            for slot in 0..2 {
                ws.write_string_with_format(row, 6 + slot as ColNum, entry.intervals.get(slot).map(|i| intervals_text(std::slice::from_ref(i))).unwrap_or_default(), &day_fmt)?;
            }
            write_duration(ws, row, 8, entry.pdf_night_min, &time)?;
            match entry.night_hours {
                Some(n) => ws.write_number_with_format(row, 9, n, &day_fmt)?,
                None => ws.write_blank(row, 9, &day_fmt)?,
            };
            match (&entry.code, entry.hours) {
                (None, Some(h)) => {
                    let value = ((h - entry.worked_min as f64 / 60.0) * 100.0).round() / 100.0;
                    let formula = format!("=ROUND({}-{}*24,2)", cell(row, 5), cell(row, 3));
                    ws.write_formula_with_format(row, 10, Formula::new(formula).set_result(value.to_string()), &diff_fmt)?;
                    diff += value;
                }
                _ => {
                    ws.write_blank(row, 10, &diff_fmt)?;
                }
            }
            ws.write_string_with_format(row, 11, audit_remarks(entry), &left)?;

            card_min += entry.worked_min;
            written += if entry.code.is_none() { entry.hours.unwrap_or(0.0) } else { 0.0 };
            night += entry.night_hours.unwrap_or(0.0);
            row += 1;
        }

        // One total line per driver (also for a driver without any activity).
        ws.merge_range(row, 0, row, 2, &format!("Total {}", emp.name), &total)?;
        let sums: [(ColNum, f64, &Format); 4] = [
            (3, card_min as f64 / DAY as f64, &total_time),
            (5, written, &total),
            (9, night, &total),
            (10, (diff * 100.0).round() / 100.0, &total_diff),
        ];
        for col in [4, 6, 7, 8, 11] {
            ws.write_blank(row, col, &total)?;
        }
        for (col, value, fmt) in sums {
            let formula = if row > first { format!("=SUM({}:{})", cell(first, col), cell(row - 1, col)) } else { "=0".to_string() };
            ws.write_formula_with_format(row, col, Formula::new(formula).set_result(value.to_string()), fmt)?;
        }
        row += 2; // a blank line between drivers
    }

    let notes: Vec<&String> = sheet.warnings.iter().chain(&sheet.notices).collect();
    if !notes.is_empty() {
        ws.write_string_with_format(row, 0, "De reținut la citirea PDF-urilor", &Styles::plain().set_bold())?;
        for (i, note) in notes.iter().enumerate() {
            ws.write_string_with_format(row + 1 + i as RowNum, 0, note.as_str(), &plain)?;
        }
    }

    for (col, width) in AUDIT_WIDTHS.iter().enumerate() {
        ws.set_column_width(col as ColNum, *width)?;
    }
    ws.set_freeze_panes(header_row + 1, 3)?;
    ws.set_repeat_rows(header_row, header_row)?;
    ws.set_landscape();
    ws.set_paper_size(9); // A4
    ws.set_print_fit_to_pages(1, 0);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_states_the_rules_in_use() {
        // A minimum of 3 hours for the night chosen by the office (the default writes every night hour).
        let mut settings = Settings { night_threshold_min: 180, ..Settings::default() };
        let blank = Timesheet {
            company: String::new(),
            location: String::new(),
            year: 2026,
            month: 9,
            working_days: 22,
            norm_hours: 176,
            days: Vec::new(),
            employees: Vec::new(),
            warnings: Vec::new(),
            notices: Vec::new(),
            sources: Vec::new(),
        };
        let lines = audit_rules(&blank, &settings).join("\n");
        assert!(lines.contains("de la 20 minute"));
        assert!(lines.contains("22 zile lucrătoare × 8 ore = 176 ore"));
        assert!(lines.contains("pe grilă de 30 minute"));
        assert!(lines.contains("intervalele scrise în pontaj, între 00:00 și 07:00") && lines.contains("cel puțin 3:00"));

        settings.interval_mode = IntervalMode::Card;
        settings.night_source = NightSource::Card;
        settings.night_threshold_min = Settings::default().night_threshold_min;
        let lines = audit_rules(&blank, &settings).join("\n");
        assert!(lines.contains("orele reale de pe card"));
        assert!(lines.contains("coloana „Noapte” din raportul de tahograf") && lines.contains("se trec toate orele de noapte"));
    }

    #[test]
    fn audit_remarks_name_what_was_done_by_hand() {
        let mut entry = DayEntry { worked_min: 400, hours: Some(7.0), ..Default::default() };
        assert_eq!(audit_remarks(&entry), "");
        entry.override_hours = Some(8.0);
        entry.override_night = Some(3.0);
        assert_eq!(audit_remarks(&entry), "Ore trecute manual; noapte trecută manual");
        let no_card = DayEntry { hours: Some(8.0), ..Default::default() };
        assert_eq!(audit_remarks(&no_card), "Fără date pe card");
        let absence = DayEntry { code: Some("CO".into()), ..Default::default() };
        assert_eq!(audit_remarks(&absence), "Absență trecută manual");
        assert!(audit_has_data(&absence) && !audit_has_data(&DayEntry::default()));
    }

    #[test]
    fn an_employee_gets_a_pair_of_rows_for_every_interval_of_its_busiest_day() {
        let iv = |start, end| Interval { start, end };
        let mut emp = Employee {
            name: "X".into(),
            function: String::new(),
            location: String::new(),
            activities: Vec::new(),
            days: vec![DayEntry::default(); 3],
            totals: Default::default(),
        };
        assert_eq!(rows_of(&emp), 6, "the six rows of the payroll file");
        emp.days[0].intervals = vec![iv(300, 480)];
        emp.days[1].intervals = vec![iv(300, 480), iv(540, 720), iv(1080, 1260)];
        assert_eq!(rows_of(&emp), 8, "hours, night and three start/end pairs");
    }

    #[test]
    fn cell_names() {
        assert_eq!(cell(0, 0), "A1");
        assert_eq!(cell(10, 33), "AH11");
        assert_eq!(cell(9, 26), "AA10");
    }
}
