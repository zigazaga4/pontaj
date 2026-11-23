//! The one Excel file of everybody (tachograph, truck drivers, Categoria B, office, garage, city), written and read back:
//! the order of the names, the colour of each name, the norm of each person and the hours of the Categoria B drivers
//! (formulas over their own sheets, recomputed here) against the file the Categoria B mode writes alone.

mod support;

use std::path::{Path, PathBuf};

use calamine::{open_workbook, Reader, Xlsx};
use pontaj_core::combined::{self, Parts};
use pontaj_core::roster::{self, Plan, StaffMember};
use pontaj_core::settings::Settings;
use pontaj_core::timesheet::{display_name, Group, Timesheet};
use pontaj_core::{catb, recompute};

use support::formula::Engine;
use support::month;
use support::workbook::Workbook;

const YEAR: i32 = 2026;
const MONTH: u32 = 11;
/// The first row of the people on the first sheet (zero-based) and the columns the test reads.
const FIRST_ROW: u32 = 10;
const NAME_COL: u32 = 1;
const FIRST_DAY_COL: u32 = 4;
/// November has 30 days: the totals start on the column after the last day.
const NOT_WORKED_COL: u32 = FIRST_DAY_COL + 30 + 2;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pontaj-combined-{name}-{}.xlsx", std::process::id()))
}

fn staff(names: &[&str], group: Group, plan: Plan) -> Vec<StaffMember> {
    names.iter().map(|name| StaffMember { name: (*name).into(), function: "Funcție".into(), group, plan: plan.clone(), note: String::new() }).collect()
}

fn settings() -> Settings {
    Settings { tir_drivers: vec!["Zaharia Ion".into()], ..Settings::default() }
}

/// A tachograph month made of people with typed hours (the reports themselves are not needed to see how the file is put together).
fn tachograph(settings: &Settings) -> Timesheet {
    let drivers = staff(&["Zaharia Ion", "Andrei Vasile"], Group::Office, Plan::standard_weekdays());
    let mut sheet = roster::build_sheet(&drivers, Group::Office, YEAR, MONTH, settings);
    for employee in &mut sheet.employees {
        employee.group = Group::Tachograph;
    }
    recompute(&mut sheet, settings);
    sheet
}

/// (row, name) of every person of the first sheet.
fn people(sheet: &support::workbook::Sheet) -> Vec<(u32, String)> {
    let last = sheet.values.keys().map(|(row, _)| *row).max().unwrap_or(0);
    let total = (FIRST_ROW..=last).find(|row| sheet.text(*row, 0).is_some_and(|text| text.trim() == "TOTAL")).unwrap_or(last + 1);
    (FIRST_ROW..total).filter_map(|row| sheet.text(row, NAME_COL).map(|name| (row, name))).collect()
}

/// The colour of the name cell, as (red, green, blue).
fn name_color(path: &Path, row: u32) -> (u8, u8, u8) {
    let mut workbook: Xlsx<_> = open_workbook(path).expect("the file opens");
    let name = workbook.sheet_names()[0].clone();
    let styles = workbook.worksheet_style(&name).expect("the styles of the first sheet");
    let color = styles.get((row, NAME_COL)).and_then(|style| style.fill.as_ref()).and_then(|fill| fill.get_color()).expect("a filled name");
    (color.red, color.green, color.blue)
}

#[test]
fn everybody_is_on_the_first_sheet_in_alphabetical_order_with_the_colour_of_the_group_and_the_hours_of_categoria_b_are_still_formulas() {
    let settings = settings();
    let tachograph = tachograph(&settings);
    let (catb_sheet, _) = month::downloaded(7, YEAR, MONTH, 3, &settings);
    let mut staff = roster::build_sheet(&staff(&["Birou Ana"], Group::Office, Plan::standard_weekdays()), Group::Office, YEAR, MONTH, &settings).employees;
    staff.extend(roster::build_sheet(&staff_city(), Group::City, YEAR, MONTH, &settings).employees);

    let parts = Parts { year: YEAR, month: MONTH, tachograph: Some(&tachograph), catb: Some(&catb_sheet), staff };
    let path = temp("all");
    combined::write_xlsx(&parts, &settings, &path).expect("the combined file is written");
    let workbook = Workbook::read(&path);

    let sheet_names: Vec<&str> = workbook.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(sheet_names[..2], ["Pontaj 11.2026", "Verificare"], "{sheet_names:?}");
    for name in ["Sinteză", "Pontaj zilnic", "Detaliu"] {
        assert!(sheet_names.contains(&name), "{name} is in the file: {sheet_names:?}");
    }

    let listed = people(&workbook.sheets[0]);
    let names: Vec<&str> = listed.iter().map(|(_, name)| name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "alphabetical, like the payroll file");
    let expected = ["ALFA DOI", "ANDREI VASILE", "BIROU ANA", "CITADIN DAN", "STEFANESCU PROBA", "TESTESCU UNU", "ZAHARIA ION"];
    assert_eq!(names.iter().map(|n| display_name(n, true)).collect::<Vec<_>>(), expected);

    // Yellow for the tachograph, green for the truck driver, white for Categoria B, the office and the city.
    let yellow = (0xFF, 0xFF, 0x00);
    let green = (0x92, 0xD0, 0x50);
    let white = (0xFF, 0xFF, 0xFF);
    let colors: Vec<(u8, u8, u8)> = listed.iter().map(|(row, _)| name_color(&path, *row)).collect();
    assert_eq!(colors, [white, yellow, white, white, white, white, green]);

    // The norm: the month's for the staff and the tachograph, the ceiling of Categoria B (weekdays, holidays included) for its drivers.
    let engine = Engine::new(&workbook);
    let norm_of = |name: &str| {
        let (row, _) = listed.iter().find(|(_, n)| display_name(n, true) == name).unwrap();
        workbook.sheets[0].formulas.get(&(*row, NOT_WORKED_COL)).cloned().unwrap_or_default()
    };
    assert!(norm_of("BIROU ANA").starts_with("MAX(0,160-"), "{}", norm_of("BIROU ANA"));
    assert!(norm_of("ANDREI VASILE").starts_with("MAX(0,160-"));
    assert!(norm_of("ALFA DOI").starts_with("MAX(0,168-"), "{}", norm_of("ALFA DOI"));

    // The hours of the Categoria B drivers are the ones of the file Categoria B writes alone, day by day.
    let alone_path = temp("alone");
    catb::excel::write_xlsx(&catb_sheet, &settings, &alone_path).expect("the Categoria B file is written");
    let alone = Workbook::read(&alone_path);
    let alone_engine = Engine::new(&alone);
    let alone_people = people(&alone.sheets[0]);
    assert_eq!(alone_people.len(), 3);
    for (alone_row, name) in &alone_people {
        let (row, _) = listed.iter().find(|(_, n)| n == name).unwrap_or_else(|| panic!("{name} is in the combined file"));
        for col in FIRST_DAY_COL..FIRST_DAY_COL + 30 {
            let (together, apart) = (engine.value(0, *row, col), alone_engine.value(0, *alone_row, col));
            assert!(together == apart || (together.is_blank() && apart.is_blank()), "{name}, column {col}: {} in the combined file, {} alone", together.show(), apart.show());
        }
    }
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&alone_path).ok();
}

fn staff_city() -> Vec<StaffMember> {
    let plan = Plan::standard_rotation(chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
    staff(&["Citadin Dan"], Group::City, plan)
}

#[test]
fn without_a_tachograph_or_categoria_b_the_staff_is_still_a_file() {
    let settings = settings();
    let staff = roster::build_sheet(&staff_city(), Group::City, YEAR, MONTH, &settings).employees;
    let parts = Parts { year: YEAR, month: MONTH, tachograph: None, catb: None, staff };
    let path = temp("staff");
    combined::write_xlsx(&parts, &settings, &path).expect("the file is written");
    let workbook = Workbook::read(&path);
    assert_eq!(workbook.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Pontaj 11.2026"], "no tachograph and no Categoria B: only the pontaj");
    assert_eq!(people(&workbook.sheets[0]).len(), 1);
    std::fs::remove_file(&path).ok();
}
