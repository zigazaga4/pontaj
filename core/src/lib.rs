//! Pontaj automat din rapoartele de tahograf (PDF) → foaie colectivă de prezență (Excel).

pub mod calendar;
pub mod excel;
pub mod pdf;
pub mod rules;
pub mod settings;
pub mod time;
pub mod timesheet;

use std::path::{Path, PathBuf};

pub use excel::write_xlsx;
pub use rules::recompute;
pub use settings::Settings;
pub use timesheet::Timesheet;

#[derive(Debug, Clone)]
pub struct Error(String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Parses all PDFs in parallel (one thread per file) and builds the timesheet.
/// Files that fail are reported as warnings as long as at least one succeeds.
pub fn process_pdfs<P: AsRef<Path> + Sync>(paths: &[P], settings: &Settings) -> Result<Timesheet> {
    if paths.is_empty() {
        return Err(Error::new("Nu a fost selectat niciun PDF."));
    }
    let results: Vec<Result<Vec<pdf::DriverReport>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths.iter().map(|p| scope.spawn(move || pdf::parse_file(p.as_ref()))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(Error::new("Citirea PDF-ului a eșuat neașteptat."))))
            .collect()
    });

    let mut reports = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(list) => reports.extend(list),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if reports.is_empty() {
        return Err(Error::new(errors.join("\n")));
    }
    let mut sheet = timesheet::build(reports, settings).ok_or_else(|| Error::new("PDF-urile nu conțin nicio activitate."))?;
    sheet.warnings.splice(0..0, errors);
    Ok(sheet)
}

/// Suggested output file name, e.g. `EXEMPLU SRL_Pontaj_09_2026.xlsx`.
pub fn suggested_file_name(sheet: &Timesheet, settings: &Settings) -> String {
    let company = if settings.company_name.trim().is_empty() { sheet.company.to_uppercase() } else { settings.company_name.trim().to_string() };
    let company: String = company.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    let prefix = if company.trim().is_empty() { "Pontaj".to_string() } else { format!("{}_Pontaj", company.trim()) };
    format!("{prefix}_{:02}_{}.xlsx", sheet.month, sheet.year)
}

pub fn default_output_path(dir: &Path, sheet: &Timesheet, settings: &Settings) -> PathBuf {
    dir.join(suggested_file_name(sheet, settings))
}
