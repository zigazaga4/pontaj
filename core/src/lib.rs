//! Pontaj automat din rapoartele de tahograf (PDF) → foaie colectivă de prezență (Excel).

pub mod calendar;
pub mod catb;
pub mod excel;
pub mod history;
pub mod pdf;
pub mod rules;
pub mod settings;
pub mod store;
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

/// Reads more PDFs into an existing timesheet, keeping its manual edits.
/// Files already loaded (same name) are skipped; a PDF of another month
/// starts a new timesheet.
pub fn add_pdfs<P: AsRef<Path> + Sync>(previous: Option<Timesheet>, paths: &[P], settings: &Settings) -> Result<Timesheet> {
    add_pdfs_with(previous, paths, settings, |_, _| Ok(None))
}

/// [`add_pdfs`] for an app that keeps its months: PDFs of a month other than the open one are added to that month as
/// `saved` has it (see [`timesheet::combine`]) instead of starting it again.
pub fn add_pdfs_with<P: AsRef<Path> + Sync>(
    previous: Option<Timesheet>,
    paths: &[P],
    settings: &Settings,
    saved: impl FnOnce(i32, u32) -> Result<Option<Timesheet>>,
) -> Result<Timesheet> {
    let is_new = |p: &&P| {
        let name = p.as_ref().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        !previous.as_ref().is_some_and(|open| open.sources.contains(&name))
    };
    let new_paths: Vec<&P> = paths.iter().filter(is_new).collect();
    if new_paths.is_empty() {
        return previous.map_or_else(|| process_pdfs(paths, settings), Ok);
    }
    let fresh = process_pdfs(&new_paths, settings)?;
    timesheet::combine(previous, fresh, settings, saved)
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
