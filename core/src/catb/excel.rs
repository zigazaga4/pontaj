//! The Excel file of the "Categoria B" pontaj, after the office's changes:
//! - **Sinteză**: one line per driver (trip hours, waiting hours, total, ceiling, excess, remarks, estimate);
//! - **Pontaj zilnic**: the same hours day by day;
//! - **Detaliu**: every trip and every waiting period, in order, with the reason for each number;
//! - **Avertizări**: what the office should look at, and the suggestions for drivers over the ceiling.
//!
//! Amounts appear only when `catb.show_money` is on; the hours and the warnings are always there.

use std::path::Path;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use rust_xlsxwriter::{Color, ColNum, Format, FormatAlign, Formula, RowNum, Workbook, Worksheet, XlsxError};

use super::model::*;
use super::compute::NO_TARIFF;
use crate::calendar::{month_name, weekday_short};
use crate::excel::{cell, range, write_duration, xlsx_err, Styles, OFF_FILL, TOTAL_FILL};
use crate::settings::Settings;
use crate::time::{fmt_duration, DAY};
use crate::{Error, Result};

type Done = std::result::Result<(), XlsxError>;

const RED_FILL: u32 = 0xFFC7CE;
const AMBER_FILL: u32 = 0xFFEB9C;
const TIME: &str = "[h]:mm";

/// Writes the workbook.
pub fn write_xlsx(sheet: &Sheet, settings: &Settings, path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    summary(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    grid(workbook.add_worksheet(), sheet).map_err(xlsx_err)?;
    detail(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    remarks(workbook.add_worksheet(), sheet, settings).map_err(xlsx_err)?;
    workbook.save(path).map_err(|e| Error::new(format!("Nu pot salva fișierul Excel: {e}")))
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
const PIECES: [(&str, &str, &str); 7] = [
    ("station_long", "staționare lungă", "staționări lungi"),
    ("place_differs", "loc diferit între curse", "locuri diferite între curse"),
    ("to_confirm", "pauză de confirmat", "pauze de confirmat"),
    ("stays_bucharest", "rămas în București", "rămas în București"),
    ("overlap", "suprapunere", "suprapuneri"),
    ("no_tariff", "cursă fără tarif", "curse fără tarif"),
    ("excursion", "probabilă excursie", "probabile excursii"),
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

const SUMMARY_HEADERS: [&str; 10] = ["Nr.", "Șofer", "Zile lucrate", "Curse regulate", "Ore curse", "Ore staționare", "Total ore", "Plafon lunar", "Depășire", "Avertizări"];
const MONEY_HEADERS: [&str; 4] = ["Curse (lei)", "Exterioare (lei)", "Staționare (lei)", "Total estimat (lei)"];

fn summary(ws: &mut Worksheet, sheet: &Sheet, settings: &Settings) -> Done {
    let cfg = &settings.catb;
    let st = Styles::new();
    let plain = Styles::plain();
    ws.set_name("Sinteză")?;
    title(ws, format!("PONTAJ CATEGORIA B · {} {}", month_name(sheet.month), sheet.year))?;

    let mut lines = vec![
        match &sheet.fetched_at {
            Some(at) => format!("Curse descărcate din API la {at}, cu modificările făcute în aplicație."),
            None => "Curse introduse în aplicație.".to_string(),
        },
        format!("Plafon lunar: {} ore × {} zile (luni–vineri, sărbătorile legale incluse) = {}.", settings.hours_per_day, sheet.weekdays, fmt_duration(sheet.cap_min)),
        format!(
            "Staționarea (timpul dintre două curse regulate) intră în ore. De la {} se avertizează că probabil a plecat și s-a întors mai târziu.",
            fmt_duration(cfg.station_warn_min)
        ),
    ];
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
        ws.write_number_with_format(row, 4, time_value(t.trip_min), &time)?;
        ws.write_number_with_format(row, 5, time_value(t.station_min), &time)?;
        ws.write_formula_with_format(row, 6, formula(format!("={}+{}", cell(row, 4), cell(row, 5)), time_value(t.total_min)), &time)?;
        ws.write_number_with_format(row, 7, time_value(sheet.cap_min), &time)?;
        let over = formula(format!("=MAX(0,{}-{})", cell(row, 6), cell(row, 7)), time_value(t.over_min));
        ws.write_formula_with_format(row, 8, over, if t.over_min > 0 { &bad } else { &time })?;
        ws.write_string_with_format(row, 9, warning_summary(driver), &left_wrap)?;
        if cfg.show_money {
            for (i, value) in [t.pay_legs, t.pay_exterior, t.pay_station].into_iter().enumerate() {
                ws.write_number_with_format(row, 10 + i as ColNum, value, &money)?;
            }
            ws.write_formula_with_format(row, 13, formula(format!("=SUM({})", range(row, 10, 12)), t.pay_total), &money)?;
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
    sum(4, all(|t| time_value(t.trip_min)), &total_time, ws)?;
    sum(5, all(|t| time_value(t.station_min)), &total_time, ws)?;
    sum(6, all(|t| time_value(t.total_min)), &total_time, ws)?;
    ws.write_blank(last, 7, &total)?;
    sum(8, all(|t| time_value(t.over_min)), &total_time, ws)?;
    ws.write_blank(last, 9, &total)?;
    if cfg.show_money {
        sum(10, all(|t| t.pay_legs), &total_money, ws)?;
        sum(11, all(|t| t.pay_exterior), &total_money, ws)?;
        sum(12, all(|t| t.pay_station), &total_money, ws)?;
        sum(13, all(|t| t.pay_total), &total_money, ws)?;
    }

    if !sheet.notices.is_empty() {
        ws.write_string_with_format(last + 2, 0, "De reținut de la descărcarea din API", &Styles::plain().set_bold())?;
        for (i, note) in sheet.notices.iter().enumerate() {
            ws.write_string_with_format(last + 3 + i as RowNum, 0, note, &plain)?;
        }
    }

    let widths = [5.0, 28.0, 9.0, 9.0, 10.0, 11.0, 10.0, 10.0, 10.0, 60.0, 12.0, 13.0, 13.0, 15.0];
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
const GRID_LABELS: [&str; 3] = ["Cursă", "Staționare", "Total"];

fn grid(ws: &mut Worksheet, sheet: &Sheet) -> Done {
    let st = Styles::new();
    ws.set_name("Pontaj zilnic")?;
    title(ws, format!("ORE PE ZI · {} {}", month_name(sheet.month), sheet.year))?;
    ws.write_string_with_format(
        1,
        0,
        "Cursa = durata curselor cunoscute; staționarea se trece în ziua în care începe. Zilele gri sunt weekend sau sărbătoare legală; „scos” = ziua a fost scoasă din pontaj.",
        &Styles::plain(),
    )?;

    let days = sheet.days.len() as ColNum;
    let (last_day, total_col, cap_col, over_col) = (GRID_FIRST_DAY + days - 1, GRID_FIRST_DAY + days, GRID_FIRST_DAY + days + 1, GRID_FIRST_DAY + days + 2);
    let head = st.header();
    for (col, text) in ["Nr.", "Șofer", ""].into_iter().enumerate() {
        ws.write_string_with_format(GRID_HEADER_ROW, col as ColNum, text, &head)?;
    }
    for (index, info) in sheet.days.iter().enumerate() {
        let text = format!("{}\n{}", info.day, info.weekday);
        ws.write_string_with_format(GRID_HEADER_ROW, GRID_FIRST_DAY + index as ColNum, text, &st.day_header(info.is_off(), info.holiday.is_some()))?;
    }
    for (col, text) in [(total_col, "Total"), (cap_col, "Plafon"), (over_col, "Depășire")] {
        ws.write_string_with_format(GRID_HEADER_ROW, col, text, &head)?;
    }
    ws.set_row_height(GRID_HEADER_ROW, 30)?;

    let time_fmt = |off: bool| st.day(off).set_num_format(TIME);
    let label = st.base.clone().set_align(FormatAlign::Left);
    let name = st.fill(TOTAL_FILL).set_bold().set_align(FormatAlign::Left).set_text_wrap();
    let strong = st.base.clone().set_num_format(TIME).set_bold();
    let bad = strong.clone().set_background_color(Color::RGB(RED_FILL));
    let out = st.fill(OFF_FILL).set_italic();

    for (index, driver) in sheet.drivers.iter().enumerate() {
        let top = GRID_HEADER_ROW + 1 + (index as RowNum) * 3;
        ws.merge_range(top, 0, top + 2, 0, &(index + 1).to_string(), &st.base)?;
        ws.merge_range(top, 1, top + 2, 1, &driver.name, &name)?;
        for (i, text) in GRID_LABELS.iter().enumerate() {
            ws.write_string_with_format(top + i as RowNum, 2, *text, &label)?;
        }
        for (k, info) in sheet.days.iter().enumerate() {
            let col = GRID_FIRST_DAY + k as ColNum;
            let fmt = time_fmt(info.is_off());
            let date = NaiveDate::from_ymd_opt(sheet.year, sheet.month, info.day);
            let day = driver.days.iter().find(|d| d.date == date);
            match day {
                Some(day) if day.excluded => {
                    ws.write_blank(top, col, &out)?;
                    ws.write_blank(top + 1, col, &out)?;
                    ws.write_string_with_format(top + 2, col, "scos", &out)?;
                }
                Some(day) => {
                    write_duration(ws, top, col, day.trip_min, &fmt)?;
                    write_duration(ws, top + 1, col, day.station_min, &fmt)?;
                    if day.total_min > 0 {
                        ws.write_formula_with_format(top + 2, col, formula(format!("={}+{}", cell(top, col), cell(top + 1, col)), time_value(day.total_min)), &fmt.clone().set_bold())?;
                    } else {
                        ws.write_blank(top + 2, col, &fmt)?;
                    }
                }
                None => {
                    for r in 0..3 {
                        ws.write_blank(top + r, col, &fmt)?;
                    }
                }
            }
        }
        let t = &driver.totals;
        for (i, minutes) in [t.trip_min, t.station_min, t.total_min].into_iter().enumerate() {
            let r = top + i as RowNum;
            ws.write_formula_with_format(r, total_col, formula(format!("=SUM({})", range(r, GRID_FIRST_DAY, last_day)), time_value(minutes)), &strong)?;
        }
        ws.write_number_with_format(top + 2, cap_col, time_value(sheet.cap_min), &st.base.clone().set_num_format(TIME))?;
        let over = formula(format!("=MAX(0,{}-{})", cell(top + 2, total_col), cell(top + 2, cap_col)), time_value(t.over_min));
        ws.write_formula_with_format(top + 2, over_col, over, if t.over_min > 0 { &bad } else { &strong })?;
        for r in 0..2 {
            ws.write_blank(top + r, cap_col, &st.base)?;
            ws.write_blank(top + r, over_col, &st.base)?;
        }
    }

    ws.set_column_width(0, 5)?;
    ws.set_column_width(1, 26)?;
    ws.set_column_width(2, 11)?;
    ws.set_column_range_width(GRID_FIRST_DAY, last_day, 6.5)?;
    ws.set_column_width(total_col, 9)?;
    ws.set_column_width(cap_col, 9)?;
    ws.set_column_width(over_col, 9)?;
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
}

fn pause_state_text(pause: &Pause) -> String {
    let state = match (pause.state, pause.kind) {
        (PauseState::Stayed, _) => "a stat".to_string(),
        (PauseState::Passenger, _) => "întors ca pasager".to_string(),
        (PauseState::Off, _) => "zi liberă".to_string(),
        (PauseState::Excursion, _) => "excursie".to_string(),
        (PauseState::Auto, PauseKind::Normal) => "numărată automat".to_string(),
        (PauseState::Auto, PauseKind::Confirm) => "nehotărâtă: nu se numără".to_string(),
        (PauseState::Auto, PauseKind::Rest) => "pauză între ture".to_string(),
        (PauseState::Auto, PauseKind::Overlap) => String::new(),
    };
    let mut parts = vec![state];
    if let Some(hours) = pause.hours_override {
        parts.push(format!("ore trecute manual: {}", fmt_duration((hours * 60.0).round() as i32)));
    }
    if let Some(left) = pause.left {
        parts.push(format!("a plecat {}", left.format("%d.%m %H:%M")));
    }
    if let Some(back) = pause.back {
        parts.push(format!("s-a întors {}", back.format("%d.%m %H:%M")));
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
        || pause.kind == PauseKind::Confirm
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
        let counted = !trip.edit.removed && counts(date);
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
                kind: "Staționare".into(),
                start: pause.start,
                end: pause.end,
                from: pause.place_from.clone(),
                to: pause.place_to.clone(),
                minutes: counted.then_some(pause.counted_min),
                pay: counted.then_some(pause.extra_pay),
                note,
                remarks: if counted { remark_text(&pause.alerts) } else { String::new() },
                out: !counted,
            });
        }
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

fn detail(ws: &mut Worksheet, sheet: &Sheet, settings: &Settings) -> Done {
    let money = settings.catb.show_money;
    let st = Styles::new();
    ws.set_name("Detaliu")?;
    title(ws, format!("DETALIU CURSE ȘI STAȚIONĂRI · {} {}", month_name(sheet.month), sheet.year))?;
    ws.write_string_with_format(1, 0, "Rândurile gri nu se numără (cursă scoasă, zi scoasă din pontaj sau în afara lunii).", &Styles::plain())?;

    // Columns: Data, Zi, Tip, Plecare, Sosire, Din, Spre, Ore, [Lei], Observații, Atenție.
    let mut headers: Vec<&str> = DETAIL_HEADERS[..8].to_vec();
    if money {
        headers.push("Lei (estimat)");
    }
    headers.extend(&DETAIL_HEADERS[8..]);
    let (hours_col, money_col) = (7 as ColNum, 8 as ColNum);
    let (note_col, remark_col) = if money { (9 as ColNum, 10 as ColNum) } else { (8, 9) };
    let last_col = remark_col;
    let header_row: RowNum = 3;
    let head = st.header();
    for (col, text) in headers.iter().enumerate() {
        ws.write_string_with_format(header_row, col as ColNum, *text, &head)?;
    }
    ws.set_row_height(header_row, 30)?;

    let text = st.base.clone().set_align(FormatAlign::Left);
    let wrap = text.clone().set_text_wrap();
    let centered = st.base.clone();
    let time = st.base.clone().set_num_format(TIME);
    let lei = st.base.clone().set_num_format("#,##0");
    let band = st.fill(TOTAL_FILL).set_bold().set_align(FormatAlign::Left);
    let total = st.fill(TOTAL_FILL).set_bold();
    let total_time = total.clone().set_num_format(TIME);
    let total_lei = total.clone().set_num_format("#,##0");
    let grey = |f: &Format| f.clone().set_background_color(Color::RGB(OFF_FILL)).set_italic();
    let attention = |f: &Format| f.clone().set_background_color(Color::RGB(AMBER_FILL));

    let mut row = header_row + 1;
    for driver in &sheet.drivers {
        let lines = detail_rows(driver, sheet.year, sheet.month);
        if lines.is_empty() {
            continue;
        }
        ws.merge_range(row, 0, row, last_col, &driver.name, &band)?;
        row += 1;
        let first = row;
        for line in &lines {
            let (text, wrap, centered, time, lei) = if line.out { (grey(&text), grey(&wrap), grey(&centered), grey(&time), grey(&lei)) } else { (text.clone(), wrap.clone(), centered.clone(), time.clone(), lei.clone()) };
            ws.write_string_with_format(row, 0, line.date.format("%d.%m.%Y").to_string(), &centered)?;
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
        "place_differs" => "Loc diferit între curse",
        "to_confirm" => "De confirmat",
        "excursion" => "Probabilă excursie",
        "stays_bucharest" => "A rămas în București",
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
    let level_text = |rank: u8| ["Depășire", "Propunere", "Atenție", "Informare"][rank as usize];

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
    fn times_on_another_day_are_marked() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(clock(parse_time("2026-10-05 05:00"), day), "05:00");
        assert_eq!(clock(parse_time("2026-10-06 02:30"), day), "02:30 (+1)");
        assert_eq!(clock(None, day), "");
    }
}
