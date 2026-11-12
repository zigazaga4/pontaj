//! Writes the "Foaie colectivă de prezență (pontaj)" workbook, using the same
//! layout as the payroll team's file: six rows per employee (hours, night
//! hours, two start/end pairs) and live formulas for every total.

use std::path::Path;

use rust_xlsxwriter::{Color, ColNum, Format, FormatAlign, FormatBorder, Formula, RowNum, Workbook, Worksheet, XlsxError};

use crate::calendar::month_name;
use crate::settings::{IntervalMode, NightSource, Settings};
use crate::time::{fmt_clock, fmt_duration, DAY};
use crate::timesheet::{DayEntry, Employee, Interval, Timesheet, CODES, OLP};
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

/// Writes the timesheet; a second worksheet ("Verificare") lists, day by day,
/// what the tachograph shows next to what was written.
pub fn write_xlsx(sheet: &Timesheet, settings: &Settings, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    build_sheet(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    build_audit(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
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
            (None, Some(h)) => ws.write_number_with_format(row, col, h, &hours_fmt)?,
            (None, None) => ws.write_blank(row, col, &hours_fmt)?,
        };
        let night_fmt = day.clone().set_italic().set_font_color(Color::RGB(NIGHT_FONT));
        match entry.night_hours {
            Some(n) => ws.write_number_with_format(row + 1, col, n, &night_fmt)?,
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
        let fmt = if col == cols.worked || col == cols.night { total_fmt.clone() } else { thick_top(st.base.clone()) };
        ws.merge_range(row, col, last, col, "", &fmt)?;
        ws.write_formula_with_format(row, col, Formula::new(formula).set_result(value.to_string()), &fmt)?;
    }
    Ok(())
}

fn write_totals(ws: &mut Worksheet, sheet: &Timesheet, st: &Styles, cols: &Cols, row: RowNum) -> std::result::Result<(), XlsxError> {
    let fmt = st.fill(TOTAL_FILL).set_bold().set_border_top(FormatBorder::Medium).set_border_bottom(FormatBorder::Medium);
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
fn write_duration(ws: &mut Worksheet, row: RowNum, col: ColNum, minutes: i32, fmt: &Format) -> std::result::Result<(), XlsxError> {
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
        let mut settings = Settings::default();
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
        assert!(lines.contains("coloana „Noapte” din raportul de tahograf") && lines.contains("cel puțin 3:00"));

        settings.interval_mode = IntervalMode::Card;
        settings.night_source = NightSource::Intervals;
        settings.night_start = "00:00".into();
        settings.night_end = "07:00".into();
        settings.night_threshold_min = 0;
        let lines = audit_rules(&blank, &settings).join("\n");
        assert!(lines.contains("orele reale de pe card"));
        assert!(lines.contains("între 00:00 și 07:00") && lines.contains("se trec toate orele de noapte"));
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
    fn cell_names() {
        assert_eq!(cell(0, 0), "A1");
        assert_eq!(cell(10, 33), "AH11");
        assert_eq!(cell(9, 26), "AA10");
    }
}
