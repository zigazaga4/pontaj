//! The Excel file of the "Categoria B" pontaj, after the office's changes:
//! - **Pontaj MM.YYYY**: the payroll team's *foaie colectivă* (see [`super::pontaj`]); the hours of a day are formulas
//!   over "Pontaj zilnic", the totals are formulas over those;
//! - **Sinteză**: one line per driver (trip hours, waiting hours, total, ceiling, excess, remarks, estimate);
//! - **Pontaj zilnic**: the same hours day by day;
//! - **Detaliu**: every trip and every waiting period, in order, with the reason for each number;
//! - **Avertizări**: what the office should look at, and the suggestions for drivers over the ceiling.
//!
//! The chain of formulas is: lines of "Detaliu" (minutes, real dates) → "Pontaj zilnic" and "Sinteză" (sums by day and by
//! kind) → the day hours of the first sheet (rounded) → its totals. What the rules decide (the days, the legs, the
//! intervals, the night hours) stays a number written by the app.
//!
//! Amounts appear only when `catb.show_money` is on; the hours and the warnings are always there.

use std::path::Path;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use rust_xlsxwriter::{Color, ColNum, Format, FormatAlign, Formula, RowNum, Workbook, Worksheet, XlsxError};

use super::model::*;
use super::compute::{is_station, NO_TARIFF};
use crate::calendar::{month_name, weekday_short};
use crate::excel::{cell, excel_date, range, xlsx_err, Styles, OFF_FILL, TOTAL_FILL};
use crate::settings::Settings;
use crate::time::{fmt_duration, DAY};
use crate::{Error, Result};

type Done = std::result::Result<(), XlsxError>;

const RED_FILL: u32 = 0xFFC7CE;
const AMBER_FILL: u32 = 0xFFEB9C;
const TIME: &str = "[h]:mm";
/// Hours that show nothing while they are zero: the cells of a day without trips are formulas too.
const TIME_BLANK_ZERO: &str = "[h]:mm;;";

const SUMMARY_SHEET: &str = "Sinteză";
const GRID_SHEET: &str = "Pontaj zilnic";
const DETAIL_SHEET: &str = "Detaliu";
/// The kind written in "Detaliu" for a waiting period that counts.
const STATION_KIND: &str = "Staționare";
/// The kind of the lines for the time added to fill a day; everything else with hours is a trip.
const FILL_KIND: &str = "Completare";

/// The three kinds of hours a driver's day is made of, as "Detaliu" tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Trips,
    Waiting,
    Fill,
}

impl Part {
    /// Does a line of this kind belong to this part? (What the SUMIFS conditions of [`kind_conditions`] select.)
    #[cfg(test)]
    fn takes(self, kind: &str) -> bool {
        match self {
            Part::Waiting => kind == STATION_KIND,
            Part::Fill => kind == FILL_KIND,
            Part::Trips => kind != STATION_KIND && kind != FILL_KIND,
        }
    }
}

/// Writes the workbook: the pontaj itself first, laid out like the payroll team's file, then the sheets that show
/// where its numbers come from (the lines of "Detaliu" feed the other sheets through formulas).
pub fn write_xlsx(sheet: &Sheet, settings: &Settings, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    // One order of the drivers for all the sheets: the alphabetical one of the payroll file.
    let sheet = &super::pontaj::sort_drivers(sheet, settings);
    let lines: Vec<Vec<DetailRow>> = sheet.drivers.iter().map(|driver| detail_rows(driver, sheet.year, sheet.month)).collect();
    let blocks = detail_blocks(&lines);

    let round_from = settings.round_up_from_min;
    let hours_from = |index: usize, day: u32| grid_hours_formula(sheet, &blocks, index, day, round_from);
    crate::excel::pontaj_sheet(workbook.add_worksheet(), &super::pontaj::to_timesheet(sheet, settings), settings, Some(&hours_from)).map_err(xlsx_err)?;
    summary(workbook.add_worksheet(), sheet, settings, &blocks).map_err(xlsx_err)?;
    grid(workbook.add_worksheet(), sheet, &blocks).map_err(xlsx_err)?;
    detail(workbook.add_worksheet(), sheet, settings, &lines, &blocks).map_err(xlsx_err)?;
    remarks(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    workbook.save(path).map_err(|e| Error::new(format!("Nu pot salva fișierul Excel: {e}")))
}

/// Where the lines of a driver sit in "Detaliu" (first and last row, zero-based): what the formulas of the other sheets add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetailBlock {
    first: RowNum,
    last: RowNum,
}

const DETAIL_HEADER_ROW: RowNum = 3;
const DETAIL_DATE_COL: ColNum = 0;
const DETAIL_KIND_COL: ColNum = 2;
const DETAIL_HOURS_COL: ColNum = 7;
const DETAIL_MONEY_COL: ColNum = 8;

/// The block of each driver, laid out as [`detail`] writes them: a band with the name, the lines, a total, a blank row.
/// A driver without lines has no block.
fn detail_blocks(lines: &[Vec<DetailRow>]) -> Vec<Option<DetailBlock>> {
    let mut row = DETAIL_HEADER_ROW + 1;
    lines
        .iter()
        .map(|rows| {
            if rows.is_empty() {
                return None;
            }
            let first = row + 1;
            let last = first + rows.len() as RowNum - 1;
            row = last + 3;
            Some(DetailBlock { first, last })
        })
        .collect()
}

/// `Detaliu!$H$5:$H$20`: one column of a block, absolute.
fn detail_span(col: ColNum, block: DetailBlock) -> String {
    let name = cell(0, col).trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    format!("{DETAIL_SHEET}!${name}${}:${name}${}", block.first + 1, block.last + 1)
}

/// The conditions on the kind column of a block that pick one part of the hours (arguments of a SUMIFS).
fn kind_conditions(block: DetailBlock, part: Part) -> String {
    let kinds = detail_span(DETAIL_KIND_COL, block);
    match part {
        Part::Waiting => format!("{kinds},\"{STATION_KIND}\""),
        Part::Fill => format!("{kinds},\"{FILL_KIND}\""),
        Part::Trips => format!("{kinds},\"<>{STATION_KIND}\",{kinds},\"<>{FILL_KIND}\""),
    }
}

/// `=SUMIFS(column, kinds, "<kind>")`: the sum of a column of the block over the lines of one part.
fn sum_by_kind(block: DetailBlock, value_col: ColNum, part: Part) -> String {
    format!("=SUMIFS({},{})", detail_span(value_col, block), kind_conditions(block, part))
}

/// `=SUMIFS(hours, dates, DATE(y,m,d), kinds, "<kind>")`: the minutes of one day, trips, waiting or time added.
fn sum_by_day(block: DetailBlock, date: NaiveDate, part: Part) -> String {
    format!(
        "=SUMIFS({},{},DATE({},{},{}),{})",
        detail_span(DETAIL_HOURS_COL, block),
        detail_span(DETAIL_DATE_COL, block),
        date.year(),
        date.month(),
        date.day(),
        kind_conditions(block, part)
    )
}

/// A day of a driver is calculated (by formula) unless it was taken out of the pontaj or is a leave: those cells hold text.
fn is_calculated(driver: &Driver, date: Option<NaiveDate>) -> bool {
    let day = driver.days.iter().find(|d| d.date == date);
    !day.is_some_and(|d| d.excluded || d.leave.is_some())
}

/// The cell of "Pontaj zilnic" with the total of a day of the driver at `index`.
fn grid_total_cell(index: usize, day: u32) -> String {
    format!("'{GRID_SHEET}'!{}", cell(grid_top(index) + GRID_ROWS - 1, GRID_FIRST_DAY + day as ColNum - 1))
}

/// The hours of a day on the first sheet: the minutes of "Pontaj zilnic" rounded to whole hours, the leftover minutes
/// counting as an hour from `round_from` (the same rule as [`crate::time::round_hours`]). A day that rounds to nothing
/// is an empty text, like the empty cell of the payroll file (the sums skip it).
fn grid_hours_formula(sheet: &Sheet, blocks: &[Option<DetailBlock>], index: usize, day: u32, round_from: i32) -> Option<String> {
    blocks.get(index).copied().flatten()?;
    let driver = sheet.drivers.get(index)?;
    if !is_calculated(driver, NaiveDate::from_ymd_opt(sheet.year, sheet.month, day)) {
        return None;
    }
    // Under `from` minutes (a minute at least) the day is worth no hour at all; from it, the leftover rounds up.
    let from = round_from.clamp(1, 60);
    let minutes = format!("ROUND({}*{DAY},0)", grid_total_cell(index, day));
    Some(format!("=IF({minutes}<{from},\"\",INT({minutes}/60)+IF(MOD({minutes},60)>={from},1,0))"))
}

fn time_value(minutes: i32) -> f64 {
    f64::from(minutes) / f64::from(DAY)
}

/// `=…` with the value Excel would compute, so viewers that do not calculate still show the numbers.
fn formula(text: String, value: f64) -> Formula {
    Formula::new(text).set_result(value.to_string())
}

fn title(ws: &mut Worksheet, text: String) -> Done {
    ws.write_string_with_format(0, 0, text, &Styles::plain().set_bold().set_font_size(14))?;
    Ok(())
}

/// The remarks counted for a driver: (date, remark), without the days the office took out of the pontaj.
fn driver_remarks(driver: &Driver) -> Vec<(Option<NaiveDate>, &Alert)> {
    let out_of_pontaj = |date: Option<NaiveDate>| date.is_some_and(|d| driver.excluded_days.contains(&d));
    let mut list: Vec<(Option<NaiveDate>, &Alert)> = driver.alerts.iter().map(|a| (None, a)).collect();
    for pause in &driver.pauses {
        let date = pause.start.map(|s| s.date());
        if !out_of_pontaj(date) {
            list.extend(pause.alerts.iter().map(|a| (date, a)));
        }
    }
    for day in driver.days.iter().filter(|d| !d.excluded) {
        list.extend(day.alerts.iter().map(|a| (day.date, a)));
    }
    list
}

/// Remark codes in the order the short summary lists them, with their singular and plural.
const PIECES: [(&str, &str, &str); 6] = [
    ("station_long", "staționare lungă", "staționări lungi"),
    ("long_day", "zi peste norma zilnică", "zile peste norma zilnică"),
    ("place_differs", "loc diferit între curse", "locuri diferite între curse"),
    ("overlap", "suprapunere", "suprapuneri"),
    ("no_tariff", "cursă fără tarif", "curse fără tarif"),
    ("excursion_done", "excursie în tahograf", "excursii în tahograf"),
];

fn warning_summary(driver: &Driver) -> String {
    let mut parts: Vec<String> = Vec::new();
    if driver.totals.over_min > 0 {
        parts.push(format!("peste plafon cu {}", fmt_duration(driver.totals.over_min)));
    }
    let remarks = driver_remarks(driver);
    for (code, one, many) in PIECES {
        let count = remarks.iter().filter(|(_, a)| a.code == code).count();
        if count > 0 {
            parts.push(format!("{count} {}", if count == 1 { one } else { many }));
        }
    }
    if !driver.excluded_days.is_empty() {
        let n = driver.excluded_days.len();
        parts.push(format!("{n} {} scoase din pontaj", if n == 1 { "zi" } else { "zile" }));
    }
    parts.join(" · ")
}

// ---------------------------------------------------------------------------------------------------------------------
// Sinteză
// ---------------------------------------------------------------------------------------------------------------------

const SUMMARY_HEADERS: [&str; 12] = ["Nr.", "Șofer", "Zile lucrate", "Curse regulate", "Ore curse", "Ore staționare", "Ore completare", "Total ore", "Plafon lunar", "Depășire", "Concediu (zile)", "Avertizări"];
const MONEY_HEADERS: [&str; 4] = ["Curse (lei)", "Exterioare (lei)", "Staționare (lei)", "Total estimat (lei)"];

// Columns of "Sinteză" (zero-based); the amounts, when shown, follow the warnings.
const S_TRIPS: ColNum = 4;
const S_WAITING: ColNum = 5;
const S_FILL: ColNum = 6;
const S_TOTAL: ColNum = 7;
const S_CAP: ColNum = 8;
const S_OVER: ColNum = 9;
const S_LEAVE: ColNum = 10;
const S_WARNINGS: ColNum = 11;
const S_MONEY: ColNum = 12;

fn summary(ws: &mut Worksheet, sheet: &Sheet, settings: &Settings, blocks: &[Option<DetailBlock>]) -> Done {
    let cfg = &settings.catb;
    let st = Styles::new();
    let plain = Styles::plain();
    ws.set_name(SUMMARY_SHEET)?;
    title(ws, format!("PONTAJ CATEGORIA B · {} {}", month_name(sheet.month), sheet.year))?;

    let mut lines = vec![
        match &sheet.fetched_at {
            Some(at) => format!("Curse descărcate din API la {at}, cu modificările făcute în aplicație."),
            None => "Curse introduse în aplicație.".to_string(),
        },
        format!("Plafon lunar: {} ore × {} zile (luni–vineri, sărbătorile legale incluse) = {}.", settings.hours_per_day, sheet.weekdays, fmt_duration(sheet.cap_min)),
        format!(
            "Staționarea (așteptarea la Otopeni între două curse regulate) intră în ore; pauza (șoferul e acasă, în Constanța) nu. De la {} de staționare se avertizează că probabil a plecat și s-a întors mai târziu.",
            fmt_duration(cfg.station_warn_min)
        ),
    ];
    if cfg.fill_day {
        lines.push(format!(
            "Fiecare zi lucrată se aduce la {} ore: timpul care lipsește se adaugă înainte de prima plecare și după ultima sosire (cel mult {} pe fiecare parte). Apare ca „Completare” în „Detaliu” și în „Pontaj zilnic”.",
            settings.hours_per_day,
            fmt_duration(cfg.fill_max_side_min)
        ));
    }
    if cfg.show_money {
        lines.push(format!(
            "Plata este doar o estimare: {:.0} lei pe cursă regulată, primele {} de staționare incluse, apoi {:.0} lei pe oră; cursele private se plătesc după destinație.",
            cfg.leg_pay(),
            fmt_duration(cfg.station_included_min),
            cfg.station_rate
        ));
    }
    for (i, line) in lines.iter().enumerate() {
        ws.write_string_with_format(1 + i as RowNum, 0, line, &plain)?;
    }

    let header_row = 2 + lines.len() as RowNum + 1;
    let head = st.header();
    let mut headers: Vec<&str> = SUMMARY_HEADERS.to_vec();
    if cfg.show_money {
        headers.extend(MONEY_HEADERS);
    }
    for (col, text) in headers.iter().enumerate() {
        ws.write_string_with_format(header_row, col as ColNum, *text, &head)?;
    }
    ws.set_row_height(header_row, 32)?;

    let number = st.base.clone();
    let left = st.base.clone().set_align(FormatAlign::Left);
    let left_wrap = left.clone().set_text_wrap();
    let time = st.base.clone().set_num_format(TIME);
    let money = st.base.clone().set_num_format("#,##0");
    let bad = time.clone().set_background_color(Color::RGB(RED_FILL)).set_bold();
    let total = st.fill(TOTAL_FILL).set_bold();
    let total_time = total.clone().set_num_format(TIME);
    let total_money = total.clone().set_num_format("#,##0");

    let first = header_row + 1;
    let mut row = first;
    for (index, driver) in sheet.drivers.iter().enumerate() {
        let t = &driver.totals;
        ws.write_number_with_format(row, 0, (index + 1) as f64, &number)?;
        ws.write_string_with_format(row, 1, &driver.name, &left)?;
        ws.write_number_with_format(row, 2, f64::from(t.days), &number)?;
        ws.write_number_with_format(row, 3, f64::from(t.legs), &number)?;
        // Hours of trips, of waiting and of the time added: the sums of the driver's lines in "Detaliu" (nothing to sum without lines).
        for (col, part, minutes) in [(S_TRIPS, Part::Trips, t.trip_min), (S_WAITING, Part::Waiting, t.station_min), (S_FILL, Part::Fill, t.fill_min)] {
            match blocks[index] {
                Some(block) => ws.write_formula_with_format(row, col, formula(sum_by_kind(block, DETAIL_HOURS_COL, part), time_value(minutes)), &time)?,
                None => ws.write_number_with_format(row, col, time_value(minutes), &time)?,
            };
        }
        ws.write_formula_with_format(row, S_TOTAL, formula(format!("=SUM({})", range(row, S_TRIPS, S_FILL)), time_value(t.total_min)), &time)?;
        ws.write_number_with_format(row, S_CAP, time_value(sheet.cap_min), &time)?;
        let over = formula(format!("=MAX(0,{}-{})", cell(row, S_TOTAL), cell(row, S_CAP)), time_value(t.over_min));
        ws.write_formula_with_format(row, S_OVER, over, if t.over_min > 0 { &bad } else { &time })?;
        ws.write_string_with_format(row, S_LEAVE, leave_summary(t), &left_wrap)?;
        ws.write_string_with_format(row, S_WARNINGS, warning_summary(driver), &left_wrap)?;
        if cfg.show_money {
            ws.write_number_with_format(row, S_MONEY, t.pay_legs, &money)?;
            ws.write_number_with_format(row, S_MONEY + 1, t.pay_exterior, &money)?;
            match blocks[index] {
                Some(block) => ws.write_formula_with_format(row, S_MONEY + 2, formula(sum_by_kind(block, DETAIL_MONEY_COL, Part::Waiting), t.pay_station), &money)?,
                None => ws.write_number_with_format(row, S_MONEY + 2, t.pay_station, &money)?,
            };
            ws.write_formula_with_format(row, S_MONEY + 3, formula(format!("=SUM({})", range(row, S_MONEY, S_MONEY + 2)), t.pay_total), &money)?;
        }
        row += 1;
    }

    // Totals row.
    let last = row;
    ws.merge_range(last, 0, last, 1, "Total", &total)?;
    let sum = |col: ColNum, value: f64, fmt: &Format, ws: &mut Worksheet| -> Done {
        let text = if row > first { format!("=SUM({}:{})", cell(first, col), cell(last - 1, col)) } else { "=0".to_string() };
        ws.write_formula_with_format(last, col, formula(text, value), fmt)?;
        Ok(())
    };
    let all = |f: fn(&Totals) -> f64| sheet.drivers.iter().map(|d| f(&d.totals)).sum::<f64>();
    sum(2, all(|t| f64::from(t.days)), &total, ws)?;
    sum(3, all(|t| f64::from(t.legs)), &total, ws)?;
    sum(S_TRIPS, all(|t| time_value(t.trip_min)), &total_time, ws)?;
    sum(S_WAITING, all(|t| time_value(t.station_min)), &total_time, ws)?;
    sum(S_FILL, all(|t| time_value(t.fill_min)), &total_time, ws)?;
    sum(S_TOTAL, all(|t| time_value(t.total_min)), &total_time, ws)?;
    ws.write_blank(last, S_CAP, &total)?;
    sum(S_OVER, all(|t| time_value(t.over_min)), &total_time, ws)?;
    ws.write_blank(last, S_LEAVE, &total)?;
    ws.write_blank(last, S_WARNINGS, &total)?;
    if cfg.show_money {
        sum(S_MONEY, all(|t| t.pay_legs), &total_money, ws)?;
        sum(S_MONEY + 1, all(|t| t.pay_exterior), &total_money, ws)?;
        sum(S_MONEY + 2, all(|t| t.pay_station), &total_money, ws)?;
        sum(S_MONEY + 3, all(|t| t.pay_total), &total_money, ws)?;
    }

    if !sheet.notices.is_empty() {
        ws.write_string_with_format(last + 2, 0, "De reținut de la descărcarea din API", &Styles::plain().set_bold())?;
        for (i, note) in sheet.notices.iter().enumerate() {
            ws.write_string_with_format(last + 3 + i as RowNum, 0, note, &plain)?;
        }
    }

    let widths = [5.0, 28.0, 9.0, 9.0, 10.0, 11.0, 11.0, 10.0, 10.0, 10.0, 18.0, 60.0, 12.0, 13.0, 13.0, 15.0];
    for (col, width) in widths.iter().enumerate() {
        ws.set_column_width(col as ColNum, *width)?;
    }
    ws.set_freeze_panes(header_row + 1, 2)?;
    ws.set_repeat_rows(header_row, header_row)?;
    ws.set_landscape();
    ws.set_paper_size(9);
    ws.set_print_fit_to_pages(1, 0);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// Pontaj zilnic
// ---------------------------------------------------------------------------------------------------------------------

const GRID_FIRST_DAY: ColNum = 3;
const GRID_HEADER_ROW: RowNum = 3;
const GRID_LABELS: [&str; 4] = ["Cursă", "Staționare", "Completare", "Total"];
/// Rows of one driver in "Pontaj zilnic": trips, waiting, time added, total (the last one).
const GRID_ROWS: RowNum = GRID_LABELS.len() as RowNum;
const LEAVE_FILL: u32 = 0xE2EFDA;

/// First of the rows of the driver at `index`.
fn grid_top(index: usize) -> RowNum {
    GRID_HEADER_ROW + 1 + (index as RowNum) * GRID_ROWS
}

/// "CO 4 zile · CM 1 zi": the days of leave of a driver, by kind of leave (empty without leave).
fn leave_summary(totals: &Totals) -> String {
    let parts: Vec<String> = LEAVE_TYPES
        .iter()
        .filter_map(|(code, _)| totals.leave_days.get(*code).map(|n| format!("{code} {n} {}", if *n == 1 { "zi" } else { "zile" })))
        .collect();
    parts.join(" · ")
}

fn grid(ws: &mut Worksheet, sheet: &Sheet, blocks: &[Option<DetailBlock>]) -> Done {
    let st = Styles::new();
    ws.set_name(GRID_SHEET)?;
    title(ws, format!("ORE PE ZI · {} {}", month_name(sheet.month), sheet.year))?;
    let codes: Vec<&str> = LEAVE_TYPES.iter().map(|(code, _)| *code).collect();
    ws.write_string_with_format(
        1,
        0,
        format!(
            "Orele sunt formule peste foaia „Detaliu”. Cursa = durata curselor cunoscute; staționarea se trece în ziua în care începe; completarea = timpul adăugat înainte de prima plecare și după ultima sosire ca ziua să aibă orele unei zile întregi. Zilele gri sunt weekend sau sărbătoare legală; „scos” = ziua a fost scoasă din pontaj; {} = zi de concediu (vezi „Detaliu”).",
            codes.join(", ")
        ),
        &Styles::plain(),
    )?;

    let days = sheet.days.len() as ColNum;
    let (last_day, total_col, cap_col, over_col, leave_col) = (GRID_FIRST_DAY + days - 1, GRID_FIRST_DAY + days, GRID_FIRST_DAY + days + 1, GRID_FIRST_DAY + days + 2, GRID_FIRST_DAY + days + 3);
    let head = st.header();
    for (col, text) in ["Nr.", "Șofer", ""].into_iter().enumerate() {
        ws.write_string_with_format(GRID_HEADER_ROW, col as ColNum, text, &head)?;
    }
    for (index, info) in sheet.days.iter().enumerate() {
        let text = format!("{}\n{}", info.day, info.weekday);
        ws.write_string_with_format(GRID_HEADER_ROW, GRID_FIRST_DAY + index as ColNum, text, &st.day_header(info.is_off(), info.holiday.is_some()))?;
    }
    for (col, text) in [(total_col, "Total"), (cap_col, "Plafon"), (over_col, "Depășire"), (leave_col, "Concediu (zile)")] {
        ws.write_string_with_format(GRID_HEADER_ROW, col, text, &head)?;
    }
    ws.set_row_height(GRID_HEADER_ROW, 30)?;

    let time_fmt = |off: bool| st.day(off).set_num_format(TIME_BLANK_ZERO);
    let label = st.base.clone().set_align(FormatAlign::Left);
    let name = st.fill(TOTAL_FILL).set_bold().set_align(FormatAlign::Left).set_text_wrap();
    let strong = st.base.clone().set_num_format(TIME).set_bold();
    let bad = strong.clone().set_background_color(Color::RGB(RED_FILL));
    let out = st.fill(OFF_FILL).set_italic();
    let leave = st.fill(LEAVE_FILL).set_bold();
    let leave_total = st.base.clone().set_text_wrap();

    for (index, driver) in sheet.drivers.iter().enumerate() {
        let top = grid_top(index);
        let bottom = top + GRID_ROWS - 1;
        ws.merge_range(top, 0, bottom, 0, &(index + 1).to_string(), &st.base)?;
        ws.merge_range(top, 1, bottom, 1, &driver.name, &name)?;
        for (i, text) in GRID_LABELS.iter().enumerate() {
            ws.write_string_with_format(top + i as RowNum, 2, *text, &label)?;
        }
        for (k, info) in sheet.days.iter().enumerate() {
            let col = GRID_FIRST_DAY + k as ColNum;
            let fmt = time_fmt(info.is_off());
            let date = NaiveDate::from_ymd_opt(sheet.year, sheet.month, info.day);
            let day = driver.days.iter().find(|d| d.date == date);
            match (day, date, blocks[index]) {
                (Some(day), _, _) if day.excluded => {
                    for r in 0..GRID_ROWS - 1 {
                        ws.write_blank(top + r, col, &out)?;
                    }
                    ws.write_string_with_format(bottom, col, "scos", &out)?;
                }
                (Some(DayRow { leave: Some(code), .. }), _, _) => {
                    ws.merge_range(top, col, bottom, col, code, &leave)?;
                }
                // Every other day is a formula, empty or not: a line added in "Detaliu" shows up here.
                (day, Some(date), Some(block)) => {
                    let (trip_min, station_min, fill_min) = day.map_or((0, 0, 0), |d| (d.trip_min, d.station_min, d.fill_min));
                    ws.write_formula_with_format(top, col, formula(sum_by_day(block, date, Part::Trips), time_value(trip_min)), &fmt)?;
                    ws.write_formula_with_format(top + 1, col, formula(sum_by_day(block, date, Part::Waiting), time_value(station_min)), &fmt)?;
                    ws.write_formula_with_format(top + 2, col, formula(sum_by_day(block, date, Part::Fill), time_value(fill_min)), &fmt)?;
                    let total = formula(format!("=SUM({}:{})", cell(top, col), cell(bottom - 1, col)), time_value(trip_min + station_min + fill_min));
                    ws.write_formula_with_format(bottom, col, total, &fmt.clone().set_bold())?;
                }
                _ => {
                    for r in 0..GRID_ROWS {
                        ws.write_blank(top + r, col, &fmt)?;
                    }
                }
            }
        }
        let t = &driver.totals;
        for (i, minutes) in [t.trip_min, t.station_min, t.fill_min, t.total_min].into_iter().enumerate() {
            let r = top + i as RowNum;
            ws.write_formula_with_format(r, total_col, formula(format!("=SUM({})", range(r, GRID_FIRST_DAY, last_day)), time_value(minutes)), &strong)?;
        }
        ws.write_number_with_format(bottom, cap_col, time_value(sheet.cap_min), &st.base.clone().set_num_format(TIME))?;
        let over = formula(format!("=MAX(0,{}-{})", cell(bottom, total_col), cell(bottom, cap_col)), time_value(t.over_min));
        ws.write_formula_with_format(bottom, over_col, over, if t.over_min > 0 { &bad } else { &strong })?;
        for r in 0..GRID_ROWS - 1 {
            ws.write_blank(top + r, cap_col, &st.base)?;
            ws.write_blank(top + r, over_col, &st.base)?;
        }
        ws.merge_range(top, leave_col, bottom, leave_col, &leave_summary(t), &leave_total)?;
    }

    ws.set_column_width(0, 5)?;
    ws.set_column_width(1, 26)?;
    ws.set_column_width(2, 11)?;
    ws.set_column_range_width(GRID_FIRST_DAY, last_day, 6.5)?;
    ws.set_column_width(total_col, 9)?;
    ws.set_column_width(cap_col, 9)?;
    ws.set_column_width(over_col, 9)?;
    ws.set_column_width(leave_col, 16)?;
    ws.set_freeze_panes(GRID_HEADER_ROW + 1, GRID_FIRST_DAY)?;
    ws.set_landscape();
    ws.set_paper_size(9);
    ws.set_print_fit_to_pages(1, 0);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// Detaliu
// ---------------------------------------------------------------------------------------------------------------------

/// One line of the detail sheet: a trip or a waiting period.
#[derive(Debug, Clone, PartialEq)]
struct DetailRow {
    date: NaiveDate,
    kind: String,
    start: Option<NaiveDateTime>,
    end: Option<NaiveDateTime>,
    from: String,
    to: String,
    /// Counted in the hours (None: not counted, e.g. a removed trip or a day taken out).
    minutes: Option<i32>,
    /// Counted in the estimate.
    pay: Option<f64>,
    note: String,
    remarks: String,
    /// Counts nowhere: shown grey.
    out: bool,
    /// Place among the lines of one day: 0 before the trips (time added before the first departure), 1 the trips, 2 after
    /// them (time added after the last arrival).
    order: u8,
}

fn pause_state_text(pause: &Pause) -> String {
    let state = match (pause.state, is_station(pause)) {
        _ if pause.kind == PauseKind::Overlap => String::new(),
        (PauseState::Auto, true) => "staționare, după reguli (la Otopeni)".to_string(),
        (PauseState::Auto, false) => "pauză, după reguli: acasă în Constanța sau peste limita pauzei lungi, nu se numără".to_string(),
        (_, true) => "staționare, aleasă în aplicație".to_string(),
        (_, false) => "pauză, aleasă în aplicație: nu se numără".to_string(),
    };
    let mut parts = vec![state];
    if let Some(hours) = pause.hours_override.filter(|_| is_station(pause)) {
        parts.push(format!("ore trecute manual: {}", fmt_duration((hours * 60.0).round() as i32)));
    }
    if !pause.note.trim().is_empty() {
        parts.push(pause.note.trim().to_string());
    }
    parts.retain(|p| !p.is_empty());
    parts.join("; ")
}

/// A waiting period is listed when it counts, needs a look, or the office decided something about it.
fn worth_listing(pause: &Pause) -> bool {
    pause.counted_min > 0
        || !pause.alerts.is_empty()
        || pause.kind == PauseKind::Overlap
        || pause.state != PauseState::Auto
        || pause.hours_override.is_some()
        || !pause.note.trim().is_empty()
}

fn remark_text(alerts: &[Alert]) -> String {
    alerts.iter().map(|a| a.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// The lines of one driver, in time order. What is summed in them equals the driver's totals.
fn detail_rows(driver: &Driver, year: i32, month: u32) -> Vec<DetailRow> {
    let counts = |date: NaiveDate| date.year() == year && date.month() == month && !driver.excluded_days.contains(&date);
    let why_not = |date: NaiveDate| -> &'static str {
        if driver.excluded_days.contains(&date) {
            "zi scoasă din pontaj"
        } else {
            "în afara lunii"
        }
    };
    let mut rows = Vec::new();
    for trip in &driver.trips {
        let eff = &trip.eff;
        let Some(departure) = eff.departure else { continue };
        let date = departure.date();
        let counted = !trip.edit.removed && !eff.excursion && counts(date);
        let mut notes: Vec<String> = Vec::new();
        if trip.edit.removed {
            notes.push("scoasă din pontaj".into());
        } else if !counts(date) {
            notes.push(why_not(date).into());
        } else if !eff.price_note.is_empty() {
            notes.push(eff.price_note.clone());
        }
        if trip.manual {
            notes.push("adăugată manual".into());
        } else if trip.edit != TripEdit::default() && !trip.edit.removed {
            notes.push("modificată de birou".into());
        }
        if !trip.car.is_empty() {
            notes.push(format!("mașina {}", trip.car));
        }
        let remarks = if counted && eff.price_note == NO_TARIFF { "Fără tarif: trece prețul în aplicație.".to_string() } else { String::new() };
        rows.push(DetailRow {
            date,
            kind: if trip.edit.removed { format!("{} (scoasă)", trip.kind.label()) } else { trip.kind.label().to_string() },
            start: Some(departure),
            end: eff.arrival,
            from: eff.from.clone(),
            to: eff.to.clone(),
            minutes: (counted && eff.counts_hours).then_some(eff.minutes),
            pay: counted.then_some(eff.price),
            note: notes.join("; "),
            remarks,
            out: !counted,
            order: 1,
        });
        for pause in driver.pauses.iter().filter(|p| p.prev == trip.id && worth_listing(p)) {
            let Some(start) = pause.start else { continue };
            let counted = counts(start.date());
            let mut note = pause_state_text(pause);
            if !counted {
                note = format!("{}; {}", why_not(start.date()), note);
            }
            rows.push(DetailRow {
                date: start.date(),
                kind: match (pause.kind, is_station(pause)) {
                    (PauseKind::Overlap, _) => "Suprapunere",
                    (_, true) => "Staționare",
                    (_, false) => "Pauză",
                }
                .into(),
                start: pause.start,
                end: pause.end,
                from: pause.place_from.clone(),
                to: pause.place_to.clone(),
                minutes: counted.then_some(pause.counted_min),
                pay: counted.then_some(pause.extra_pay),
                note,
                remarks: if counted { remark_text(&pause.alerts) } else { String::new() },
                out: !counted,
                order: 1,
            });
        }
    }
    // The days of leave and the time added to the days: no trip behind them, but the office must see where the hours come from.
    let before = rows.len();
    for day in driver.days.iter().filter(|d| d.fill_min > 0) {
        let Some(date) = day.date else { continue };
        let Some((first, last)) = super::pontaj::day_span(driver, date) else { continue };
        let counted = counts(date);
        let at = |minute: i32| date.and_time(NaiveTime::MIN) + Duration::minutes(i64::from(minute));
        let sides = [(0, day.fill_before_min, first - day.fill_before_min, first, "la plecare"), (2, day.fill_after_min, last, last + day.fill_after_min, "la sosire")];
        for (order, minutes, from, to, side) in sides.into_iter().filter(|side| side.1 > 0) {
            let mut note = format!("completare {side}: ziua se aduce la orele unei zile întregi");
            if day.fill_manual {
                note.push_str("; ore trecute de birou");
            }
            if !counted {
                note = format!("{}; {note}", why_not(date));
            }
            rows.push(DetailRow {
                date,
                kind: FILL_KIND.into(),
                start: Some(at(from)),
                end: Some(at(to)),
                from: String::new(),
                to: String::new(),
                minutes: counted.then_some(minutes),
                pay: None,
                note,
                remarks: String::new(),
                out: !counted,
                order,
            });
        }
    }
    for day in driver.days.iter().filter(|d| d.leave.is_some()) {
        let (Some(date), Some(code)) = (day.date, day.leave.as_deref()) else { continue };
        let counted = counts(date);
        rows.push(DetailRow {
            date,
            kind: "Concediu".into(),
            start: None,
            end: None,
            from: String::new(),
            to: String::new(),
            minutes: None,
            pay: None,
            note: if counted { format!("{} ({code})", leave_name(code)) } else { format!("{} ({code}); {}", leave_name(code), why_not(date)) },
            remarks: String::new(),
            out: !counted,
            order: 1,
        });
    }
    if rows.len() > before {
        rows.sort_by_key(|r| (r.date, r.order)); // stable: the lines of one day keep their order
    }
    rows
}

/// `05:00`, or `05:00 (+1)` when it falls on another day than `reference`.
fn clock(at: Option<NaiveDateTime>, reference: NaiveDate) -> String {
    match at {
        None => String::new(),
        Some(at) => match (at.date() - reference).num_days() {
            0 => at.format("%H:%M").to_string(),
            n if n > 0 => format!("{} (+{n})", at.format("%H:%M")),
            n => format!("{} ({n})", at.format("%H:%M")),
        },
    }
}

const DETAIL_HEADERS: [&str; 10] = ["Data", "Zi", "Tip", "Plecare", "Sosire", "Din", "Spre", "Ore", "Observații despre rând", "Atenție"];

fn detail(ws: &mut Worksheet, sheet: &Sheet, settings: &Settings, all_lines: &[Vec<DetailRow>], blocks: &[Option<DetailBlock>]) -> Done {
    let money = settings.catb.show_money;
    let st = Styles::new();
    ws.set_name(DETAIL_SHEET)?;
    title(ws, format!("DETALIU CURSE ȘI STAȚIONĂRI · {} {}", month_name(sheet.month), sheet.year))?;
    ws.write_string_with_format(1, 0, "Rândurile gri nu se numără (cursă scoasă, zi scoasă din pontaj sau în afara lunii).", &Styles::plain())?;

    // Columns: Data, Zi, Tip, Plecare, Sosire, Din, Spre, Ore, [Lei], Observații, Atenție.
    let mut headers: Vec<&str> = DETAIL_HEADERS[..8].to_vec();
    if money {
        headers.push("Lei (estimat)");
    }
    headers.extend(&DETAIL_HEADERS[8..]);
    let (hours_col, money_col) = (DETAIL_HOURS_COL, DETAIL_MONEY_COL);
    let (note_col, remark_col) = if money { (9 as ColNum, 10 as ColNum) } else { (8, 9) };
    let last_col = remark_col;
    let header_row = DETAIL_HEADER_ROW;
    let head = st.header();
    for (col, text) in headers.iter().enumerate() {
        ws.write_string_with_format(header_row, col as ColNum, *text, &head)?;
    }
    ws.set_row_height(header_row, 30)?;

    let text = st.base.clone().set_align(FormatAlign::Left);
    let wrap = text.clone().set_text_wrap();
    let centered = st.base.clone();
    let dated = st.base.clone().set_num_format("dd.mm.yyyy");
    let time = st.base.clone().set_num_format(TIME);
    let lei = st.base.clone().set_num_format("#,##0");
    let band = st.fill(TOTAL_FILL).set_bold().set_align(FormatAlign::Left);
    let total = st.fill(TOTAL_FILL).set_bold();
    let total_time = total.clone().set_num_format(TIME);
    let total_lei = total.clone().set_num_format("#,##0");
    let grey = |f: &Format| f.clone().set_background_color(Color::RGB(OFF_FILL)).set_italic();
    let attention = |f: &Format| f.clone().set_background_color(Color::RGB(AMBER_FILL));

    let mut row = header_row + 1;
    for ((driver, lines), block) in sheet.drivers.iter().zip(all_lines).zip(blocks) {
        let Some(block) = block else { continue };
        ws.merge_range(row, 0, row, last_col, &driver.name, &band)?;
        row += 1;
        debug_assert_eq!(row, block.first, "the formulas of the other sheets point at these rows");
        let first = row;
        for line in lines {
            let (text, wrap, centered, dated, time, lei) =
                if line.out { (grey(&text), grey(&wrap), grey(&centered), grey(&dated), grey(&time), grey(&lei)) } else { (text.clone(), wrap.clone(), centered.clone(), dated.clone(), time.clone(), lei.clone()) };
            // A real date, so "Pontaj zilnic" can add the lines of a day.
            ws.write_number_with_format(row, DETAIL_DATE_COL, excel_date(line.date.year(), line.date.month(), line.date.day()), &dated)?;
            ws.write_string_with_format(row, 1, weekday_short(line.date), &centered)?;
            ws.write_string_with_format(row, 2, &line.kind, &text)?;
            ws.write_string_with_format(row, 3, clock(line.start, line.date), &centered)?;
            ws.write_string_with_format(row, 4, clock(line.end, line.date), &centered)?;
            ws.write_string_with_format(row, 5, &line.from, &text)?;
            ws.write_string_with_format(row, 6, &line.to, &text)?;
            match line.minutes {
                Some(m) if m > 0 => {
                    ws.write_number_with_format(row, hours_col, time_value(m), &time)?;
                }
                _ => {
                    ws.write_blank(row, hours_col, &time)?;
                }
            }
            if money {
                match line.pay {
                    Some(pay) => ws.write_number_with_format(row, money_col, pay, &lei)?,
                    None => ws.write_blank(row, money_col, &lei)?,
                };
            }
            ws.write_string_with_format(row, note_col, &line.note, &wrap)?;
            let remark_fmt = if line.remarks.is_empty() || line.out { wrap.clone() } else { attention(&wrap) };
            ws.write_string_with_format(row, remark_col, &line.remarks, &remark_fmt)?;
            row += 1;
        }
        // Driver total.
        ws.merge_range(row, 0, row, 6, &format!("Total {}", driver.name), &total)?;
        let minutes: i32 = lines.iter().filter_map(|l| l.minutes).sum();
        let pay: f64 = lines.iter().filter_map(|l| l.pay).sum();
        ws.write_formula_with_format(row, hours_col, formula(format!("=SUM({}:{})", cell(first, hours_col), cell(row - 1, hours_col)), time_value(minutes)), &total_time)?;
        if money {
            ws.write_formula_with_format(row, money_col, formula(format!("=SUM({}:{})", cell(first, money_col), cell(row - 1, money_col)), pay), &total_lei)?;
        }
        ws.write_blank(row, note_col, &total)?;
        ws.write_blank(row, remark_col, &total)?;
        row += 2;
    }

    let widths: Vec<f64> = if money { vec![11.0, 5.0, 11.0, 12.0, 12.0, 24.0, 24.0, 9.0, 11.0, 38.0, 60.0] } else { vec![11.0, 5.0, 11.0, 12.0, 12.0, 24.0, 24.0, 9.0, 38.0, 60.0] };
    for (col, width) in widths.iter().enumerate() {
        ws.set_column_width(col as ColNum, *width)?;
    }
    ws.set_freeze_panes(header_row + 1, 0)?;
    ws.set_repeat_rows(header_row, header_row)?;
    ws.set_landscape();
    ws.set_paper_size(9);
    ws.set_print_fit_to_pages(1, 0);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// Avertizări
// ---------------------------------------------------------------------------------------------------------------------

fn remark_label(code: &str) -> &'static str {
    match code {
        "over_cap" => "Peste plafon",
        "station_long" => "Staționare lungă",
        "long_day" => "Zi peste norma zilnică",
        "place_differs" => "Loc diferit între curse",
        "excursion_done" => "Excursie (în tahograf)",
        "overlap" => "Curse suprapuse",
        "no_tariff" => "Fără tarif",
        _ => "Observație",
    }
}

/// One line of the warnings sheet.
#[derive(Debug, Clone, PartialEq)]
struct RemarkRow {
    driver: String,
    /// 0 = over the ceiling, 1 = how to fix it, 2 = warning, 3 = information.
    rank: u8,
    label: &'static str,
    date: Option<NaiveDate>,
    text: String,
}

fn remark_rows(sheet: &Sheet, settings: &Settings) -> Vec<RemarkRow> {
    let mut rows = Vec::new();
    for driver in &sheet.drivers {
        for (date, alert) in driver_remarks(driver) {
            let rank = match alert.level {
                Level::Alert => 0,
                Level::Warn => 2,
                Level::Info => 3,
            };
            rows.push(RemarkRow { driver: driver.name.clone(), rank, label: remark_label(&alert.code), date, text: alert.text.clone() });
        }
        for proposal in &driver.proposals {
            let mut text = proposal.text.clone();
            if settings.catb.show_money && proposal.pay.abs() >= 0.5 {
                text.push_str(&format!(" (−{:.0} lei din estimare)", proposal.pay));
            }
            rows.push(RemarkRow { driver: driver.name.clone(), rank: 1, label: "Cum se poate încadra", date: Some(proposal.date), text });
        }
    }
    rows.sort_by(|a, b| (a.rank, a.driver.to_lowercase(), a.date).cmp(&(b.rank, b.driver.to_lowercase(), b.date)));
    rows
}

fn remarks(ws: &mut Worksheet, sheet: &Sheet, settings: &Settings) -> Done {
    let st = Styles::new();
    ws.set_name("Avertizări")?;
    title(ws, format!("AVERTIZĂRI ȘI PROPUNERI · {} {}", month_name(sheet.month), sheet.year))?;
    ws.write_string_with_format(
        1,
        0,
        "Propunerile sunt doar sugestii: biroul hotărăște ce se scoate. Staționarea este dedusă din orele API-ului, nu citită de acolo; foaia șoferului are ultimul cuvânt.",
        &Styles::plain(),
    )?;
    let header_row: RowNum = 3;
    let head = st.header();
    for (col, text) in ["Șofer", "Nivel", "Subiect", "Data", "Ce s-a găsit"].iter().enumerate() {
        ws.write_string_with_format(header_row, col as ColNum, *text, &head)?;
    }
    let text = st.base.clone().set_align(FormatAlign::Left);
    let wrap = text.clone().set_text_wrap();
    let level_fmt = |rank: u8| match rank {
        0 => st.fill(RED_FILL).set_bold(),
        1 => st.fill(TOTAL_FILL),
        2 => st.fill(AMBER_FILL),
        _ => st.base.clone(),
    };
    let level_text = |rank: u8| ["De rezolvat", "Propunere", "Atenție", "Informare"][rank as usize];

    let rows = remark_rows(sheet, settings);
    let mut row = header_row + 1;
    for item in &rows {
        ws.write_string_with_format(row, 0, &item.driver, &text)?;
        ws.write_string_with_format(row, 1, level_text(item.rank), &level_fmt(item.rank))?;
        ws.write_string_with_format(row, 2, item.label, &text)?;
        ws.write_string_with_format(row, 3, item.date.map(|d| d.format("%d.%m.%Y").to_string()).unwrap_or_default(), &st.base)?;
        ws.write_string_with_format(row, 4, &item.text, &wrap)?;
        row += 1;
    }
    if rows.is_empty() {
        ws.write_string_with_format(row, 0, "Nicio avertizare.", &Styles::plain())?;
    } else {
        ws.autofilter(header_row, 0, row - 1, 4)?;
    }

    for (col, width) in [28.0, 11.0, 24.0, 12.0, 110.0].iter().enumerate() {
        ws.set_column_width(col as ColNum, *width)?;
    }
    ws.set_freeze_panes(header_row + 1, 0)?;
    ws.set_repeat_rows(header_row, header_row)?;
    ws.set_landscape();
    ws.set_paper_size(9);
    ws.set_print_fit_to_pages(1, 0);
    ws.set_footer("&CPagina &P din &N");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::api::{parse_rows, parse_time};
    use super::super::apply_download;
    use super::*;

    /// Two drivers: one with a normal day, a 7-hour wait, a private trip to Tulcea, a removed trip and a day taken out.
    fn sample() -> Sheet {
        let body = r#"{"data":[
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-05 05:00","duration_minutes":180,"car":{"tag":"CT-22-XDD"}},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-05 15:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"prv","id":"private-3"},"route":{"from":"Constanta","to":"Tulcea"},"departure":"2026-10-06 09:00","duration_minutes":null},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-4"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-07 05:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-5"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-08 05:00","duration_minutes":180},
          {"driver_id":2,"driver_name":"Ionescu Vasile","trip":{"type":"eco","id":"eco-6"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-06 05:00","duration_minutes":180}
        ]}"#;
        let parsed = parse_rows(body).unwrap();
        let mut sheet = apply_download(None, 2026, 10, &parsed, "17.11.2026 10:00", &Settings::default());
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.trips.iter_mut().find(|t| t.id == "eco-4").unwrap().edit.removed = true;
        popescu.excluded_days.push(NaiveDate::from_ymd_opt(2026, 10, 8).unwrap());
        super::super::recompute(&mut sheet, &Settings::default());
        sheet
    }

    #[test]
    fn the_detail_rows_add_up_to_the_totals() {
        let sheet = sample();
        for driver in &sheet.drivers {
            let rows = detail_rows(driver, sheet.year, sheet.month);
            let minutes: i32 = rows.iter().filter_map(|r| r.minutes).sum();
            let pay: f64 = rows.iter().filter_map(|r| r.pay).sum();
            assert_eq!(minutes, driver.totals.total_min, "{}", driver.name);
            assert_eq!(pay, driver.totals.pay_total, "{}", driver.name);
        }
    }

    /// What the SUMIF/SUMIFS of the other sheets add over the lines of "Detaliu": equal to what the app computed.
    #[test]
    fn the_sums_over_the_lines_equal_the_totals_and_the_days() {
        let sheet = sample();
        for driver in &sheet.drivers {
            let rows = detail_rows(driver, 2026, 10);
            let sum = |part: Part, date: Option<NaiveDate>| -> i32 {
                rows.iter().filter(|r| part.takes(&r.kind) && date.is_none_or(|d| r.date == d)).filter_map(|r| r.minutes).sum()
            };
            assert_eq!(sum(Part::Trips, None), driver.totals.trip_min, "trip minutes of {}", driver.name);
            assert_eq!(sum(Part::Waiting, None), driver.totals.station_min, "waiting minutes of {}", driver.name);
            assert_eq!(sum(Part::Fill, None), driver.totals.fill_min, "minutes added to the days of {}", driver.name);
            let pay_station: f64 = rows.iter().filter(|r| r.kind == STATION_KIND).filter_map(|r| r.pay).sum();
            assert_eq!(pay_station, driver.totals.pay_station, "waiting pay of {}", driver.name);
            for day in driver.days.iter().filter(|d| !d.excluded) {
                let date = day.date.unwrap();
                let parts = (sum(Part::Trips, Some(date)), sum(Part::Waiting, Some(date)), sum(Part::Fill, Some(date)));
                assert_eq!(parts, (day.trip_min, day.station_min, day.fill_min), "{} on {date}", driver.name);
                assert_eq!(day.trip_min + day.station_min + day.fill_min, day.total_min, "{} on {date}", driver.name);
            }
        }
    }

    #[test]
    fn the_blocks_follow_the_layout_of_the_detail_sheet() {
        let sheet = sample();
        let lines: Vec<Vec<DetailRow>> = sheet.drivers.iter().map(|d| detail_rows(d, 2026, 10)).collect();
        let blocks = detail_blocks(&lines);
        let (a, b) = (blocks[0].unwrap(), blocks[1].unwrap());
        assert_eq!(a.first, DETAIL_HEADER_ROW + 2, "a band with the name comes first");
        assert_eq!(a.last - a.first + 1, lines[0].len() as RowNum);
        assert_eq!(b.first, a.last + 4, "total row, blank row, band, then the lines");
        assert_eq!(detail_span(DETAIL_HOURS_COL, a), format!("Detaliu!$H${}:$H${}", a.first + 1, a.last + 1));
        assert!(sum_by_day(a, NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(), Part::Waiting).contains("DATE(2026,10,5)"));
        let trips = sum_by_kind(a, DETAIL_HOURS_COL, Part::Trips);
        assert!(trips.contains("\"<>Staționare\"") && trips.contains("\"<>Completare\""), "trips are what is neither waiting nor time added: {trips}");
        assert_eq!(detail_blocks(&[Vec::new()]), [None], "no lines, no block");
    }

    #[test]
    fn the_hours_of_the_first_sheet_point_at_the_total_row_of_their_driver_and_day() {
        let sheet = super::super::pontaj::sort_drivers(&sample(), &Settings::default());
        let lines: Vec<Vec<DetailRow>> = sheet.drivers.iter().map(|d| detail_rows(d, 2026, 10)).collect();
        let blocks = detail_blocks(&lines);
        let formula = grid_hours_formula(&sheet, &blocks, 1, 3, 20).expect("a calculated day");
        // Second driver: rows 9–12 of the grid (trips, waiting, time added, total), day 3 in the sixth column.
        assert!(formula.contains("'Pontaj zilnic'!F12"), "{formula}");
        assert!(formula.contains(">=20"), "{formula}");
        let popescu = sheet.drivers.iter().position(|d| d.id == 1).unwrap();
        assert!(grid_hours_formula(&sheet, &blocks, popescu, 8, 20).is_none(), "a day taken out of the pontaj stays empty");
        assert!(grid_hours_formula(&sheet, &blocks, 9, 3, 20).is_none(), "no such driver");
    }

    #[test]
    fn removed_trips_and_excluded_days_are_listed_but_not_counted() {
        let sheet = sample();
        let popescu = sheet.drivers.iter().find(|d| d.id == 1).unwrap();
        let rows = detail_rows(popescu, 2026, 10);
        let removed = rows.iter().find(|r| r.kind.contains("scoasă")).expect("removed trip is shown");
        assert!(removed.out && removed.minutes.is_none() && removed.note.contains("scoasă din pontaj"));
        let day_out = rows.iter().find(|r| r.date == NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()).expect("excluded day is shown");
        assert!(day_out.out && day_out.note.contains("zi scoasă din pontaj"));
        let wait = rows.iter().find(|r| r.kind == "Staționare").expect("the 7 hour wait is listed");
        assert_eq!(wait.minutes, Some(7 * 60));
        assert!(wait.remarks.contains("Staționare de 7:00"));
    }

    #[test]
    fn a_drive_made_by_the_app_is_listed_in_the_detail_and_summed_with_the_trips() {
        let body = r#"{"data":[
          {"driver_id":7,"driver_name":"Badea Lucian","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-31 11:00","duration_minutes":180},
          {"driver_id":7,"driver_name":"Badea Lucian","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-11-01 05:30","duration_minutes":180}
        ]}"#;
        let sheet = apply_download(None, 2026, 11, &parse_rows(body).unwrap(), "x", &Settings::default());
        let driver = &sheet.drivers[0];
        let rows = detail_rows(driver, 2026, 11);
        let drive = rows.iter().find(|r| r.kind == "Deplasare").expect("the drive is a line of the detail");
        assert_eq!((drive.minutes, drive.pay, drive.date), (Some(180), Some(120.0), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()));
        assert!(drive.note.contains("adăugată de aplicație"), "{}", drive.note);
        assert_eq!(rows.iter().filter_map(|r| r.minutes).sum::<i32>(), driver.totals.total_min);
        assert_eq!(driver.totals.legs, 2);
        let path = std::env::temp_dir().join(format!("pontaj-catb-drive-{}.xlsx", std::process::id()));
        write_xlsx(&sheet, &Settings::default(), &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 3000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_time_added_to_a_day_is_listed_around_its_trips_and_summed_apart_from_them() {
        let sheet = sample();
        // Ionescu Vasile has one trip of 3 h on the 6th (05:00–08:00): 2 h 30 min are added on each side.
        let ionescu = sheet.drivers.iter().find(|d| d.id == 2).unwrap();
        let rows = detail_rows(ionescu, 2026, 10);
        let day = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        assert_eq!(rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(), [FILL_KIND, "Eco", FILL_KIND], "the time before comes first, the time after last");
        assert_eq!((clock(rows[0].start, day), clock(rows[0].end, day)), ("02:30".to_string(), "05:00".to_string()));
        assert_eq!((clock(rows[2].start, day), clock(rows[2].end, day)), ("08:00".to_string(), "10:30".to_string()));
        assert!(rows[0].note.contains("la plecare") && rows[2].note.contains("la sosire"), "{:?} {:?}", rows[0].note, rows[2].note);
        assert!(rows.iter().filter(|r| r.kind == FILL_KIND).all(|r| r.minutes == Some(150) && r.pay.is_none() && !r.out));
        assert_eq!(rows.iter().filter_map(|r| r.minutes).sum::<i32>(), ionescu.totals.total_min);
        assert_eq!(ionescu.totals.total_min, 8 * 60);

        // A day taken out of the pontaj lists its added time grey and counts none of it.
        let mut out = ionescu.clone();
        out.excluded_days.push(day);
        super::super::compute::recompute_driver(&mut out, 2026, 10, &Settings::default());
        let rows = detail_rows(&out, 2026, 10);
        assert!(rows.iter().filter(|r| r.kind == FILL_KIND).all(|r| r.out && r.minutes.is_none() && r.note.contains("zi scoasă din pontaj")));

        // The long day (13 h) of the other driver is not filled and carries the warning in the summary.
        let popescu = sheet.drivers.iter().find(|d| d.id == 1).unwrap();
        assert!(warning_summary(popescu).contains("zi peste norma zilnică"), "{}", warning_summary(popescu));
        let path = std::env::temp_dir().join(format!("pontaj-catb-fill-{}.xlsx", std::process::id()));
        write_xlsx(&sheet, &Settings::default(), &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 3000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remarks_put_the_serious_ones_first() {
        let sheet = sample();
        let rows = remark_rows(&sheet, &Settings::default());
        assert!(rows.windows(2).all(|w| w[0].rank <= w[1].rank));
        assert!(rows.iter().any(|r| r.label == "Staționare lungă"));
        // The excluded day raises no remark of its own.
        assert!(rows.iter().all(|r| r.date != NaiveDate::from_ymd_opt(2026, 10, 8)));
    }

    #[test]
    fn the_workbook_is_written_with_and_without_money() {
        let sheet = sample();
        let dir = std::env::temp_dir().join(format!("pontaj-catb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for show_money in [true, false] {
            let mut settings = Settings::default();
            settings.catb.show_money = show_money;
            let path = dir.join(format!("catb-{show_money}.xlsx"));
            write_xlsx(&sheet, &settings, &path).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 3000);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_leave_is_named_in_the_detail_and_summed_by_kind() {
        let body = r#"{"data":[
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-10-05 11:00","duration_minutes":180},
          {"driver_id":1,"driver_name":"Popescu Ion","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-10-12 05:00","duration_minutes":180}
        ]}"#;
        let mut sheet = apply_download(None, 2026, 10, &parse_rows(body).unwrap(), "x", &Settings::default());
        for day in [6, 7, 8, 9] {
            sheet.drivers[0].leaves.insert(NaiveDate::from_ymd_opt(2026, 10, day).unwrap(), "CM".into());
        }
        sheet.drivers[0].leaves.insert(NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(), "CO".into());
        sheet.drivers[0].excluded_days.push(NaiveDate::from_ymd_opt(2026, 10, 9).unwrap());
        super::super::recompute(&mut sheet, &Settings::default());

        let driver = &sheet.drivers[0];
        assert_eq!(leave_summary(&driver.totals), "CO 1 zi · CM 3 zile", "the day taken out of the pontaj is not summed");
        let rows = detail_rows(driver, 2026, 10);
        let leaves: Vec<&DetailRow> = rows.iter().filter(|r| r.kind == "Concediu").collect();
        assert_eq!(leaves.len(), 5, "every leave day is listed");
        assert!(leaves[0].note.contains("Concediu medical (CM)") && leaves[0].minutes.is_none() && !leaves[0].out, "{:?}", leaves[0]);
        assert!(leaves[3].out && leaves[3].note.contains("zi scoasă din pontaj"), "{:?}", leaves[3]);
        assert!(leaves[4].note.contains("Concediu de odihnă (CO)"));
        let dates: Vec<NaiveDate> = rows.iter().map(|r| r.date).collect();
        assert!(dates.windows(2).all(|w| w[0] <= w[1]), "the lines stay in date order: {dates:?}");
        let dir = std::env::temp_dir().join(format!("pontaj-catb-leave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_xlsx(&sheet, &Settings::default(), &dir.join("leave.xlsx")).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn times_on_another_day_are_marked() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(clock(parse_time("2026-10-05 05:00"), day), "05:00");
        assert_eq!(clock(parse_time("2026-10-06 02:30"), day), "02:30 (+1)");
        assert_eq!(clock(None, day), "");
    }
}
