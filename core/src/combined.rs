//! One Excel file for everybody: the tachograph drivers (the truck drivers among them), the drivers of Categoria B and the office,
//! garage and city staff, on one *foaie colectivă* in alphabetical order like the payroll file. The colour of a name says where
//! the person comes from: yellow for the tachograph, green for the truck drivers, white for everybody else.
//!
//! - The first sheet ("Pontaj MM.YYYY") has everybody, one norm for the month (working days × hours per day); Categoria B keeps its
//!   own ceiling (see `Employee::norm_hours`), as in its own file, and its hours stay formulas over the sheets that come after.
//! - "Verificare" (what the tachograph shows next to what was written) follows when a tachograph month is part of the file.
//! - "Sinteză", "Pontaj zilnic", "Detaliu" and "Avertizări" follow when a Categoria B month is part of it.
//!
//! A person who is on two of the lists is written once (the tachograph first, then Categoria B, then the staff list) and the
//! warnings of the file say so. A driver of the tachograph who also drove on Categoria B (the category D trips are on the
//! tachograph, the small cars in the API) keeps one row with both: the hours of a day are the sum, and the cell of a day that has
//! tachograph hours is yellow, while what Categoria B adds keeps the colour of the day (white, green on weekends).

use std::collections::HashSet;
use std::path::Path;

use rust_xlsxwriter::Workbook;

use crate::calendar;
use crate::catb::{self, excel::Book};
use crate::excel::{build_audit, pontaj_sheet, xlsx_err};
use crate::settings::Settings;
use crate::rules::compute_totals;
use crate::timesheet::{self, display_name, DayEntry, Employee, Group, TimeNote, Timesheet};
use crate::{Error, Result};

/// What goes into the file of a month.
pub struct Parts<'a> {
    pub year: i32,
    pub month: u32,
    /// The tachograph month on the screen; ignored (with a warning) when it is another month.
    pub tachograph: Option<&'a Timesheet>,
    /// The Categoria B month, as the screen shows it.
    pub catb: Option<&'a catb::Sheet>,
    /// The office, garage and city staff of the month (see [`crate::roster::open_employees`]).
    pub staff: Vec<Employee>,
}

impl Parts<'_> {
    fn tachograph_of_the_month(&self) -> Option<&Timesheet> {
        self.tachograph.filter(|t| (t.year, t.month) == (self.year, self.month))
    }
}

/// The people of the first sheet; for each one, the position of the driver in the Categoria B sheets (when he is one).
struct Assembled {
    sheet: Timesheet,
    catb_at: Vec<Option<usize>>,
}

fn assemble(parts: &Parts, settings: &Settings, book: Option<&Book>) -> Assembled {
    let tachograph = parts.tachograph_of_the_month();
    let mut warnings: Vec<String> = Vec::new();
    if let Some(other) = parts.tachograph.filter(|t| (t.year, t.month) != (parts.year, parts.month)) {
        warnings.push(format!(
            "Tahograful încărcat este pe {:02}.{}: nu intră în pontajul lunii {:02}.{}.",
            other.month, other.year, parts.month, parts.year
        ));
    }

    let working_days = calendar::working_days(parts.year, parts.month);
    let norm_hours = working_days as i32 * settings.hours_per_day;
    let mut people: Vec<(Employee, Option<usize>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |people: &mut Vec<(Employee, Option<usize>)>, employee: Employee, catb_index: Option<usize>, warnings: &mut Vec<String>| {
        if seen.insert(display_name(&employee.name, true)) {
            people.push((employee, catb_index));
        } else {
            warnings.push(format!("{} este pe mai multe liste (tahograf, Categoria B, birou/garaj/oraș): în pontaj apare o singură dată.", employee.name));
        }
    };
    for employee in tachograph.map(|t| t.employees.as_slice()).unwrap_or_default() {
        // Who is a truck driver is decided by the list in the settings, whatever the month on the screen says.
        let group = if settings.is_tir(&employee.name) { Group::Tir } else { Group::Tachograph };
        add(&mut people, Employee { group, ..employee.clone() }, None, &mut warnings);
    }
    if let Some(book) = book {
        for (index, employee) in book.timesheet(settings).employees.into_iter().enumerate() {
            // A tachograph driver who also drove on Categoria B: one row with both (the category D hours are the tachograph's).
            let key = display_name(&employee.name, true);
            let twin = people.iter().position(|(person, linked)| linked.is_none() && person.group.is_tachograph() && display_name(&person.name, true) == key);
            if let Some(at) = twin {
                let merged = both(&people[at].0, &employee, norm_hours, settings);
                warnings.push(format!("{} a lucrat și pe tahograf (categoria D), și pe Categoria B: apare o singură dată, cu orele de tahograf pe galben.", merged.name));
                people[at] = (merged, Some(index));
            } else {
                add(&mut people, employee, Some(index), &mut warnings);
            }
        }
    }
    for employee in &parts.staff {
        add(&mut people, employee.clone(), None, &mut warnings);
    }
    // Alphabetical like the payroll file; the sort is stable, so the order of the sources decides between equal names.
    people.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    let (employees, catb_at) = people.into_iter().unzip();
    let sheet = Timesheet {
        company: tachograph.map(|t| t.company.clone()).unwrap_or_default(),
        location: tachograph.map(|t| t.location.clone()).unwrap_or_default(),
        year: parts.year,
        month: parts.month,
        working_days,
        norm_hours,
        days: timesheet::month_days(parts.year, parts.month),
        employees,
        warnings: tachograph.map(|t| t.warnings.clone()).unwrap_or_default().into_iter().chain(warnings).collect(),
        notices: tachograph.map(|t| t.notices.clone()).unwrap_or_default(),
        sources: tachograph.map(|t| t.sources.clone()).unwrap_or_default(),
    };
    Assembled { sheet, catb_at }
}

fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    (a.is_some() || b.is_some()).then(|| a.unwrap_or(0.0) + b.unwrap_or(0.0))
}

/// One day of somebody who is on the tachograph and on Categoria B: the hours add up (`tachograph_hours` keeps the part of the
/// tachograph), the night hours add up, the times are the tachograph's and then those of the trips (their remarks follow them).
/// A code of the tachograph stays; one of Categoria B (a leave) only counts on a day with no tachograph hours.
fn day_of_both(tachograph: &DayEntry, catb: &DayEntry) -> DayEntry {
    let mut day = tachograph.clone();
    if day.code.is_none() {
        match (tachograph.hours, &catb.code) {
            (None, Some(code)) => day.code = Some(code.clone()),
            _ => {
                day.hours = sum(tachograph.hours, catb.hours);
                day.tachograph_hours = tachograph.hours.filter(|hours| *hours > 0.0);
            }
        }
    }
    day.night_hours = sum(tachograph.night_hours, catb.night_hours);
    let before = tachograph.intervals.len();
    day.intervals.extend(catb.intervals.iter().cloned());
    day.time_notes.extend(catb.time_notes.iter().map(|note| TimeNote { interval: note.interval + before, ..note.clone() }));
    day
}

/// The row of somebody who worked on the tachograph (category D) and on Categoria B: the tachograph's person with the days of
/// both (see [`day_of_both`]) and the totals of the sum. The norm is the tachograph's.
fn both(tachograph: &Employee, catb: &Employee, sheet_norm: i32, settings: &Settings) -> Employee {
    let days = tachograph.days.iter().enumerate().map(|(at, day)| catb.days.get(at).map_or_else(|| day.clone(), |other| day_of_both(day, other))).collect();
    let mut merged = Employee { days, ..tachograph.clone() };
    merged.totals = compute_totals(&merged, merged.norm_hours.unwrap_or(sheet_norm), settings.hours_per_day);
    merged
}

/// Suggested name of the file, e.g. `EXEMPLU SRL_Pontaj_11_2026.xlsx`.
pub fn suggested_file_name(parts: &Parts, settings: &Settings) -> String {
    crate::suggested_file_name(&assemble(parts, settings, None).sheet, settings)
}

/// Writes the workbook of the month with everybody in it.
pub fn write_xlsx(parts: &Parts, settings: &Settings, path: &Path) -> Result<()> {
    let book = parts.catb.map(|sheet| Book::new(sheet, settings));
    let Assembled { sheet, catb_at } = assemble(parts, settings, book.as_ref());
    if sheet.employees.is_empty() {
        return Err(Error::new(format!("Nu este nimeni de pus în pontajul lunii {:02}.{}.", parts.month, parts.year)));
    }
    let mut workbook = Workbook::new();
    let hours_from = |index: usize, day: u32| catb_at.get(index).copied().flatten().and_then(|at| book.as_ref()?.hours_formula(at, day, settings));
    pontaj_sheet(workbook.add_worksheet(), &sheet, settings, Some(&hours_from)).map_err(xlsx_err)?;
    if let Some(tachograph) = parts.tachograph_of_the_month() {
        build_audit(workbook.add_worksheet(), tachograph, settings).map_err(xlsx_err)?;
    }
    if let Some(book) = &book {
        book.add_sheets(&mut workbook, settings).map_err(xlsx_err)?;
    }
    workbook.save(path).map_err(|e| Error::new(format!("Nu pot salva fișierul Excel: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{self, Plan, StaffMember};

    fn people(names: &[&str], group: Group, year: i32, month: u32, settings: &Settings) -> Timesheet {
        let staff: Vec<StaffMember> = names
            .iter()
            .map(|name| StaffMember { name: (*name).into(), function: "Șofer".into(), group, plan: Plan::standard_weekdays(), note: String::new(), moves: Vec::new(), changes: Vec::new(), starts_on: None, stops_on: None })
            .collect();
        roster::build_sheet(&staff, group, year, month, settings)
    }

    fn settings() -> Settings {
        Settings { tir_drivers: vec!["Zaharia Ion".into()], ..Settings::default() }
    }

    fn names(parts: &Parts, settings: &Settings) -> Vec<(String, Group)> {
        assemble(parts, settings, None).sheet.employees.iter().map(|e| (e.name.clone(), e.group)).collect()
    }

    #[test]
    fn the_tachograph_the_truck_list_and_the_staff_come_together_in_alphabetical_order() {
        let settings = settings();
        let tachograph = people(&["Zaharia Ion", "Andrei Vasile"], Group::Tachograph, 2026, 11, &settings);
        let office = people(&["Birou Ana"], Group::Office, 2026, 11, &settings);
        let parts = Parts { year: 2026, month: 11, tachograph: Some(&tachograph), catb: None, staff: office.employees };
        assert_eq!(
            names(&parts, &settings),
            [("ANDREI VASILE".to_string(), Group::Tachograph), ("BIROU ANA".to_string(), Group::Office), ("ZAHARIA ION".to_string(), Group::Tir)],
            "a name on the list of truck drivers is green whatever the tachograph month said"
        );
        let assembled = assemble(&parts, &settings, None).sheet;
        assert_eq!((assembled.working_days, assembled.norm_hours), (20, 160), "the norm of the month: 30 November is a holiday");
    }

    #[test]
    fn a_tachograph_of_another_month_is_left_out_and_said_so() {
        let settings = settings();
        let october = people(&["Andrei Vasile"], Group::Tachograph, 2026, 10, &settings);
        let office = people(&["Birou Ana"], Group::Office, 2026, 11, &settings);
        let parts = Parts { year: 2026, month: 11, tachograph: Some(&october), catb: None, staff: office.employees };
        let assembled = assemble(&parts, &settings, None).sheet;
        assert_eq!(assembled.employees.len(), 1);
        assert!(assembled.warnings.iter().any(|w| w.contains("10.2026") && w.contains("11.2026")), "{:?}", assembled.warnings);
    }

    #[test]
    fn somebody_on_two_lists_is_written_once_from_the_first_one() {
        let settings = settings();
        let tachograph = people(&["Andrei Vasile"], Group::Tachograph, 2026, 11, &settings);
        let garage = people(&["andrei vasile", "Mecanic Dan"], Group::Garage, 2026, 11, &settings);
        let parts = Parts { year: 2026, month: 11, tachograph: Some(&tachograph), catb: None, staff: garage.employees };
        let assembled = assemble(&parts, &settings, None).sheet;
        let listed: Vec<(&str, Group)> = assembled.employees.iter().map(|e| (e.name.as_str(), e.group)).collect();
        assert_eq!(listed, [("ANDREI VASILE", Group::Tachograph), ("MECANIC DAN", Group::Garage)]);
        assert!(assembled.warnings.iter().any(|w| w.contains("ANDREI VASILE") && w.contains("o singură dată")), "{:?}", assembled.warnings);
    }

    /// A tachograph driver (8 hours on every working day, the plan of the helper above stands for the tachograph) who also drove two
    /// trips of 3 hours on Categoria B: on Tuesday 3 November, a working day, and on Saturday 7 November.
    fn amza_on_both(settings: &Settings) -> (Timesheet, catb::Sheet) {
        let tachograph = people(&["Amza Ion", "Birou Ana"], Group::Tachograph, 2026, 11, settings);
        let body = r#"{"data":[
            {"driver_id":5,"driver_name":"Amza Ion","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Aeroport Otopeni Bucuresti","to":"Constanta"},"departure":"2026-11-03 18:00","duration_minutes":180},
            {"driver_id":5,"driver_name":"Amza Ion","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-11-07 09:00","duration_minutes":180},
            {"driver_id":6,"driver_name":"Anghel Dan","trip":{"type":"eco","id":"eco-3"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-11-04 09:00","duration_minutes":180}
        ]}"#;
        let sheet = catb::apply_download(None, 2026, 11, &catb::api::parse_rows(body).unwrap(), "x", settings);
        (tachograph, sheet)
    }

    #[test]
    fn somebody_on_the_tachograph_and_on_categoria_b_is_one_row_with_both_and_the_tachograph_hours_marked() {
        let settings = settings();
        let (tachograph, sheet) = amza_on_both(&settings);
        let parts = Parts { year: 2026, month: 11, tachograph: Some(&tachograph), catb: Some(&sheet), staff: Vec::new() };
        let book = Book::new(&sheet, &settings);
        let assembled = assemble(&parts, &settings, Some(&book));
        let names: Vec<&str> = assembled.sheet.employees.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["AMZA ION", "ANGHEL DAN", "BIROU ANA"], "Amza is there once");
        let amza = &assembled.sheet.employees[0];
        assert_eq!(assembled.catb_at[0], Some(0), "his Categoria B hours are linked to the sheets of Categoria B");
        assert_eq!(assembled.catb_at[1], Some(1));
        assert_eq!(amza.group, Group::Tachograph, "the tachograph's person: his name is yellow");
        // What Categoria B itself writes for Amza (a trip of 3 hours with the time added to the day).
        let own = book.timesheet(&settings).employees[0].days.iter().map(|d| d.hours.unwrap_or(0.0)).collect::<Vec<f64>>();
        assert!(own[2] >= 3.0 && own[6] >= 3.0 && own[1] == 0.0, "{own:?}");
        // Tuesday 3: 8 hours of the tachograph and the hours of Categoria B. Saturday 7: Categoria B only. Monday 2: the tachograph only.
        assert_eq!((amza.days[2].hours, amza.days[2].tachograph_hours), (Some(8.0 + own[2]), Some(8.0)));
        assert_eq!((amza.days[6].hours, amza.days[6].tachograph_hours), (Some(own[6]), None), "Categoria B alone keeps the colour of the day");
        assert_eq!((amza.days[1].hours, amza.days[1].tachograph_hours), (Some(8.0), Some(8.0)));
        assert_eq!(amza.totals.worked, 20.0 * 8.0 + own[2] + own[6], "the totals are those of the sum");
        assert_eq!(amza.days[2].intervals.len(), tachograph.employees[0].days[2].intervals.len() + 1, "the times of the tachograph, then the trip of Categoria B");
        assert!(assembled.sheet.warnings.iter().any(|w| w.contains("AMZA ION") && w.contains("galben")), "{:?}", assembled.sheet.warnings);
        assert!(!assembled.sheet.warnings.iter().any(|w| w.contains("pe mai multe liste")), "it is one person on purpose, not a mistake of the lists");
    }

    #[test]
    fn in_the_file_the_cell_of_a_tachograph_day_is_yellow_and_the_hours_of_categoria_b_are_a_formula_added_to_it() {
        use calamine::{open_workbook, Data, Reader, Xlsx};
        let settings = settings();
        let (tachograph, sheet) = amza_on_both(&settings);
        let parts = Parts { year: 2026, month: 11, tachograph: Some(&tachograph), catb: Some(&sheet), staff: Vec::new() };
        let path = std::env::temp_dir().join(format!("pontaj-combined-both-{}.xlsx", std::process::id()));
        write_xlsx(&parts, &settings, &path).unwrap();
        let mut book: Xlsx<_> = open_workbook(&path).unwrap();
        let first = book.sheet_names()[0].clone();
        let formulas = book.worksheet_formula(&first).unwrap();
        let styles = book.worksheet_style(&first).unwrap();
        let values = book.worksheet_range(&first).unwrap();
        let own = Book::new(&sheet, &settings).timesheet(&settings).employees[0].days.iter().map(|d| d.hours.unwrap_or(0.0)).collect::<Vec<f64>>();
        // The first person (Amza) has his hours on the first row of data, one column per day from column E.
        let (row, col) = (crate::excel::FIRST_DATA_ROW, |day: u32| u32::from(crate::excel::FIRST_DAY_COL) + day - 1);
        let fill = |day: u32| styles.get((row, col(day))).and_then(|s| s.fill.as_ref()).and_then(|f| f.get_color()).map(|c| (c.red, c.green, c.blue));
        let formula = |day: u32| formulas.get_value((row, col(day))).cloned().unwrap_or_default();
        assert_eq!(fill(3), Some((0xFF, 0xFF, 0x00)), "Tuesday 3: tachograph hours: yellow");
        assert!(formula(3).trim_start_matches('=').starts_with("8+N(IF("), "{}", formula(3));
        assert_eq!(values.get_value((row, col(3))), Some(&Data::Float(8.0 + own[2])));
        assert_eq!(fill(7), Some((0xC3, 0xD6, 0x9B)), "Saturday 7: Categoria B only: the green of the weekend");
        assert!(formula(7).trim_start_matches('=').starts_with("IF("), "{}", formula(7));
        assert_eq!(values.get_value((row, col(7))), Some(&Data::Float(own[6])));
        assert_eq!(fill(2), Some((0xFF, 0xFF, 0x00)), "Monday 2: only the tachograph: yellow");
        assert_eq!(values.get_value((row, col(2))), Some(&Data::Float(8.0)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_month_with_nobody_is_not_written() {
        let parts = Parts { year: 2026, month: 11, tachograph: None, catb: None, staff: Vec::new() };
        let path = std::env::temp_dir().join(format!("pontaj-combined-empty-{}.xlsx", std::process::id()));
        let error = write_xlsx(&parts, &Settings::default(), &path).unwrap_err().to_string();
        assert!(error.contains("11.2026"), "{error}");
        assert!(!path.exists());
    }
}
