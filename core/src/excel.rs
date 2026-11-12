//! Writes the "Foaie colectivă de prezență (pontaj)" workbook, using the same
//! layout as the payroll team's file: six rows per employee (hours, night
//! hours, two start/end pairs) and live formulas for every total.

use std::path::Path;

use rust_xlsxwriter::{Color, ColNum, Format, FormatAlign, FormatBorder, Formula, RowNum, Workbook, Worksheet, XlsxError};

use crate::calendar::month_name;
use crate::settings::Settings;
use crate::time::DAY;
use crate::timesheet::{Employee, Timesheet, CODES, OLP};
use crate::{Error, Result};

const HEADER_ROW: RowNum = 7;
const FIRST_DATA_ROW: RowNum = 9;
const ROWS_PER_EMPLOYEE: RowNum = 6;
const FIRST_DAY_COL: ColNum = 3;

const HEADER_FILL: u32 = 0xD9E1F2;
const OFF_FILL: u32 = 0xEDEDED;
const HOLIDAY_FILL: u32 = 0xFCE4D6;
const NAME_FILL: u32 = 0xFFF2CC;
const TOTAL_FILL: u32 = 0xFFF2CC;
const CODE_FILL: u32 = 0xE2EFDA;
const NIGHT_FONT: u32 = 0x1F4E79;

const LEGEND: [&str; 8] = [
    "CO - concediu de odihnă",
    "CM - concediu medical",
    "CP - concediu parental",
    "CFP - concediu fără plată",
    "OLP - ore libere plătite",
    "N - absențe nemotivate",
    "ZLP - zi liberă plătită",
    "Zilele gri = weekend / sărbătoare legală",
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
fn cell(row: RowNum, col: ColNum) -> String {
    let mut name = String::new();
    let mut n = col as u32 + 1;
    while n > 0 {
        let rem = (n - 1) % 26;
        name.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    format!("{name}{}", row + 1)
}

fn range(row: RowNum, from: ColNum, to: ColNum) -> String {
    format!("{}:{}", cell(row, from), cell(row, to))
}

/// Reusable cell formats.
struct Styles {
    base: Format,
}

impl Styles {
    fn new() -> Self {
        let base = Format::new()
            .set_font_name("Arial Narrow")
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x808080));
        Styles { base }
    }
    fn fill(&self, color: u32) -> Format {
        self.base.clone().set_background_color(Color::RGB(color))
    }
    fn header(&self) -> Format {
        self.fill(HEADER_FILL).set_font_name("Arial").set_bold().set_text_wrap()
    }
    fn day_header(&self, off: bool, holiday: bool) -> Format {
        let color = if holiday { HOLIDAY_FILL } else if off { OFF_FILL } else { HEADER_FILL };
        self.fill(color).set_bold().set_text_wrap()
    }
    fn day(&self, off: bool) -> Format {
        if off { self.fill(OFF_FILL) } else { self.base.clone() }
    }
    fn plain() -> Format {
        Format::new().set_font_name("Arial").set_font_size(10)
    }
}

pub fn write_xlsx(sheet: &Timesheet, settings: &Settings, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    let ws = workbook.add_worksheet();
    build_sheet(ws, sheet, settings).map_err(xlsx_err)?;
    workbook.save(path).map_err(|e| Error::new(format!("Nu pot salva fișierul Excel: {e}")))
}

fn xlsx_err(e: XlsxError) -> Error {
    Error::new(format!("Eroare la generarea Excel: {e}"))
}

fn build_sheet(ws: &mut Worksheet, sheet: &Timesheet, settings: &Settings) -> std::result::Result<(), XlsxError> {
    let st = Styles::new();
    let cols = Cols::new(sheet.days.len());
    let month = month_name(sheet.month);
    ws.set_name(format!("Pontaj {:02}.{}", sheet.month, sheet.year))?;

    write_title(ws, sheet, settings, &cols, month)?;
    write_header(ws, sheet, &st, &cols, month)?;

    let mut row = FIRST_DATA_ROW;
    for (i, employee) in sheet.employees.iter().enumerate() {
        write_employee(ws, sheet, settings, &st, &cols, row, i + 1, employee)?;
        row += ROWS_PER_EMPLOYEE;
    }
    let total_row = row;
    write_totals(ws, sheet, &st, &cols, total_row)?;
    write_signatures(ws, settings, &cols, total_row + 3)?;
    layout(ws, &cols, total_row)?;
    Ok(())
}

fn write_title(ws: &mut Worksheet, sheet: &Timesheet, settings: &Settings, cols: &Cols, month: &str) -> std::result::Result<(), XlsxError> {
    let company = if settings.company_name.trim().is_empty() { sheet.company.to_uppercase() } else { settings.company_name.trim().to_string() };
    let bold = Styles::plain().set_bold().set_font_size(11);
    ws.write_string_with_format(0, 0, company, &bold)?;
    if !settings.cui.trim().is_empty() {
        ws.write_string_with_format(1, 0, format!("CUI: {}", settings.cui.trim()), &Styles::plain())?;
    }
    for (i, text) in LEGEND.iter().enumerate() {
        ws.write_string_with_format(i as RowNum, cols.first_code, *text, &Styles::plain().set_font_size(9))?;
    }

    let title = Styles::plain().set_bold().set_font_size(14).set_align(FormatAlign::Center).set_align(FormatAlign::VerticalCenter);
    ws.merge_range(3, FIRST_DAY_COL, 3, cols.last_day, "FOAIE COLECTIVĂ DE PREZENȚĂ (PONTAJ)", &title)?;
    let subtitle = Styles::plain().set_align(FormatAlign::Center);
    let mut info = format!(
        "Luna {month} {} · {} zile lucrătoare · normă {} ore",
        sheet.year, sheet.working_days, sheet.norm_hours
    );
    if !sheet.location.is_empty() {
        info.push_str(&format!(" · {}", sheet.location));
    }
    ws.merge_range(4, FIRST_DAY_COL, 4, cols.last_day, &info, &subtitle)?;
    Ok(())
}

fn write_header(ws: &mut Worksheet, sheet: &Timesheet, st: &Styles, cols: &Cols, month: &str) -> std::result::Result<(), XlsxError> {
    let (r1, r2) = (HEADER_ROW, HEADER_ROW + 1);
    let h = st.header();
    ws.merge_range(r1, 0, r2, 0, "Nr crt", &h)?;
    ws.merge_range(r1, 1, r2, 1, "Numele și prenumele", &h)?;
    ws.merge_range(r1, 2, r2, 2, "Meseria sau funcția", &h)?;
    ws.merge_range(r1, FIRST_DAY_COL, r1, cols.last_day, &format!("{month} {}", sheet.year), &h)?;
    for (i, day) in sheet.days.iter().enumerate() {
        let fmt = st.day_header(day.is_off(), day.holiday.is_some());
        ws.write_string_with_format(r2, FIRST_DAY_COL + i as ColNum, format!("{}\n{}", day.day, day.weekday), &fmt)?;
    }
    ws.merge_range(r1, cols.worked, r2, cols.worked, "Total ore lucrate", &h)?;
    ws.merge_range(r1, cols.night, r2, cols.night, "din care: ore de noapte", &h)?;
    ws.merge_range(r1, cols.not_worked, r2, cols.not_worked, "Total ore nelucrate", &h)?;
    ws.merge_range(r1, cols.first_code, r1, cols.code(CODES.len() - 1), "din care:", &h)?;
    for (i, (_, label)) in CODES.iter().enumerate() {
        ws.write_string_with_format(r2, cols.code(i), *label, &h)?;
    }
    ws.merge_range(r1, cols.overtime, r2, cols.overtime, "Ore supl.", &h)?;
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
) -> std::result::Result<(), XlsxError> {
    let last = row + ROWS_PER_EMPLOYEE - 1;
    let thick_top = |f: Format| f.set_border_top(FormatBorder::Medium);

    ws.merge_range(row, 0, last, 0, "", &thick_top(st.base.clone()))?;
    ws.write_number_with_format(row, 0, nr as f64, &thick_top(st.base.clone()))?;
    ws.merge_range(row, 1, last, 1, &emp.name, &thick_top(st.fill(NAME_FILL).set_font_name("Arial").set_bold().set_text_wrap()))?;

    let label = st.base.clone().set_align(FormatAlign::Left).set_font_size(9);
    let labels = [emp.function.as_str(), "Ore de noapte", "Ora începerii", "Ora terminării", "Ora începerii", "Ora terminării"];
    for (k, text) in labels.iter().enumerate() {
        let fmt = if k == 0 { thick_top(label.clone().set_text_wrap()) } else { label.clone() };
        ws.write_string_with_format(row + k as RowNum, 2, *text, &fmt)?;
    }

    for (i, (info, entry)) in sheet.days.iter().zip(&emp.days).enumerate() {
        let col = FIRST_DAY_COL + i as ColNum;
        let day = st.day(info.is_off());
        let hours_fmt = thick_top(day.clone().set_bold());
        match (&entry.code, entry.hours) {
            (Some(code), _) => ws.write_string_with_format(row, col, code, &thick_top(st.fill(CODE_FILL).set_bold()))?,
            (None, Some(h)) => ws.write_number_with_format(row, col, h as f64, &hours_fmt)?,
            (None, None) => ws.write_blank(row, col, &hours_fmt)?,
        };
        let night_fmt = day.clone().set_italic().set_font_color(Color::RGB(NIGHT_FONT));
        match entry.night_hours {
            Some(n) => ws.write_number_with_format(row + 1, col, n as f64, &night_fmt)?,
            None => ws.write_blank(row + 1, col, &night_fmt)?,
        };
        let time_fmt = day.clone().set_num_format("hh:mm").set_font_size(9);
        for k in 0..4 {
            let r = row + 2 + k as RowNum;
            let value = entry.intervals.get(k / 2).map(|iv| if k % 2 == 0 { iv.start } else { iv.end });
            match value {
                Some(min) => ws.write_number_with_format(r, col, min.rem_euclid(DAY) as f64 / DAY as f64, &time_fmt)?,
                None => ws.write_blank(r, col, &time_fmt)?,
            };
        }
    }

    // Totals: merged over the six rows, formulas with cached results.
    let t = &emp.totals;
    let total_fmt = thick_top(st.fill(TOTAL_FILL).set_bold());
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

    let mut formulas: Vec<(ColNum, String, i32)> = vec![
        (cols.worked, format!("=SUM({hours_range})"), t.worked),
        (cols.night, format!("=SUM({night_range})"), t.night),
        (cols.not_worked, format!("=MAX(0,{}-{worked_ref})", sheet.norm_hours), t.not_worked),
        (cols.overtime, format!("=MAX(0,{worked_ref}+{absences}-{})", sheet.norm_hours), t.overtime),
    ];
    for (i, (code, _)) in CODES.iter().enumerate() {
        let value = t.code_hours.get(i).copied().unwrap_or(0);
        let formula = if i == olp_idx {
            format!("=MAX(0,{not_worked_ref}-{absences})")
        } else {
            format!("=COUNTIF({hours_range},\"{code}\")*{}", settings.hours_per_day)
        };
        formulas.push((cols.code(i), formula, value));
    }
    for (col, formula, value) in formulas {
        let fmt = if col == cols.worked || col == cols.night { total_fmt.clone() } else { thick_top(st.base.clone()) };
        ws.merge_range(row, col, last, col, "", &fmt)?;
        ws.write_formula_with_format(row, col, Formula::new(formula).set_result(value.to_string()), &fmt)?;
    }
    Ok(())
}

fn write_totals(ws: &mut Worksheet, sheet: &Timesheet, st: &Styles, cols: &Cols, row: RowNum) -> std::result::Result<(), XlsxError> {
    let fmt = st.fill(TOTAL_FILL).set_bold().set_border_top(FormatBorder::Medium).set_border_bottom(FormatBorder::Medium);
    ws.merge_range(row, 0, row, cols.last_day, "TOTAL", &fmt)?;
    let mut summary: Vec<(ColNum, i32)> = vec![
        (cols.worked, sheet.employees.iter().map(|e| e.totals.worked).sum()),
        (cols.night, sheet.employees.iter().map(|e| e.totals.night).sum()),
        (cols.not_worked, sheet.employees.iter().map(|e| e.totals.not_worked).sum()),
        (cols.overtime, sheet.employees.iter().map(|e| e.totals.overtime).sum()),
    ];
    for i in 0..CODES.len() {
        summary.push((cols.code(i), sheet.employees.iter().map(|e| e.totals.code_hours.get(i).copied().unwrap_or(0)).sum()));
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

fn layout(ws: &mut Worksheet, cols: &Cols, total_row: RowNum) -> std::result::Result<(), XlsxError> {
    ws.set_column_width(0, 4.5)?;
    ws.set_column_width(1, 18)?;
    ws.set_column_width(2, 13)?;
    for col in FIRST_DAY_COL..=cols.last_day {
        ws.set_column_width(col, 5.3)?;
    }
    for col in [cols.worked, cols.night, cols.not_worked] {
        ws.set_column_width(col, 8.5)?;
    }
    for i in 0..CODES.len() {
        ws.set_column_width(cols.code(i), 5.3)?;
    }
    ws.set_column_width(cols.overtime, 7)?;
    ws.set_row_height(3, 22)?;
    ws.set_row_height(HEADER_ROW, 30)?;
    ws.set_row_height(HEADER_ROW + 1, 28)?;
    for r in FIRST_DATA_ROW..total_row {
        ws.set_row_height(r, 13.5)?;
    }
    ws.set_row_height(total_row, 18)?;

    ws.set_freeze_panes(FIRST_DATA_ROW, FIRST_DAY_COL)?;
    ws.set_repeat_rows(HEADER_ROW, HEADER_ROW + 1)?;
    ws.set_landscape();
    ws.set_paper_size(8); // A3
    ws.set_print_fit_to_pages(1, 0);
    ws.set_print_center_horizontally(true);
    ws.set_margins(0.3, 0.3, 0.5, 0.5, 0.3, 0.3);
    ws.set_footer("&CPagina &P din &N");
    ws.set_zoom(90);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_names() {
        assert_eq!(cell(0, 0), "A1");
        assert_eq!(cell(10, 33), "AH11");
        assert_eq!(cell(9, 26), "AA10");
    }
}
