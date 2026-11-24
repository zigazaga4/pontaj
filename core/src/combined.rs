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
//! warnings of the file say so.

use std::collections::HashSet;
use std::path::Path;

use rust_xlsxwriter::Workbook;

use crate::calendar;
use crate::catb::{self, excel::Book};
use crate::excel::{build_audit, pontaj_sheet, xlsx_err};
use crate::settings::Settings;
use crate::timesheet::{self, display_name, Employee, Group, Timesheet};
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

    let mut people: Vec<(Employee, Option<usize>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |employee: Employee, catb_index: Option<usize>, warnings: &mut Vec<String>| {
        if seen.insert(display_name(&employee.name, true)) {
            people.push((employee, catb_index));
        } else {
            warnings.push(format!("{} este pe mai multe liste (tahograf, Categoria B, birou/garaj/oraș): în pontaj apare o singură dată.", employee.name));
        }
    };
    for employee in tachograph.map(|t| t.employees.as_slice()).unwrap_or_default() {
        // Who is a truck driver is decided by the list in the settings, whatever the month on the screen says.
        let group = if settings.is_tir(&employee.name) { Group::Tir } else { Group::Tachograph };
        add(Employee { group, ..employee.clone() }, None, &mut warnings);
    }
    if let Some(book) = book {
        for (index, employee) in book.timesheet(settings).employees.into_iter().enumerate() {
            add(employee, Some(index), &mut warnings);
        }
    }
    for employee in &parts.staff {
        add(employee.clone(), None, &mut warnings);
    }
    // Alphabetical like the payroll file; the sort is stable, so the order of the sources decides between equal names.
    people.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    let working_days = calendar::working_days(parts.year, parts.month);
    let (employees, catb_at) = people.into_iter().unzip();
    let sheet = Timesheet {
        company: tachograph.map(|t| t.company.clone()).unwrap_or_default(),
        location: tachograph.map(|t| t.location.clone()).unwrap_or_default(),
        year: parts.year,
        month: parts.month,
        working_days,
        norm_hours: working_days as i32 * settings.hours_per_day,
        days: timesheet::month_days(parts.year, parts.month),
        employees,
        warnings: tachograph.map(|t| t.warnings.clone()).unwrap_or_default().into_iter().chain(warnings).collect(),
        notices: tachograph.map(|t| t.notices.clone()).unwrap_or_default(),
        sources: tachograph.map(|t| t.sources.clone()).unwrap_or_default(),
    };
    Assembled { sheet, catb_at }
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
            .map(|name| StaffMember { name: (*name).into(), function: "Șofer".into(), group, plan: Plan::standard_weekdays(), note: String::new(), moves: Vec::new() })
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

    #[test]
    fn a_month_with_nobody_is_not_written() {
        let parts = Parts { year: 2026, month: 11, tachograph: None, catb: None, staff: Vec::new() };
        let path = std::env::temp_dir().join(format!("pontaj-combined-empty-{}.xlsx", std::process::id()));
        let error = write_xlsx(&parts, &Settings::default(), &path).unwrap_err().to_string();
        assert!(error.contains("11.2026"), "{error}");
        assert!(!path.exists());
    }
}
