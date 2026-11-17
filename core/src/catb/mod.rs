//! "Categoria B": the pontaj of the drivers whose trips come from the ALT API instead of a tachograph PDF.

pub mod api;
mod cap;
mod compute;
mod config;
pub mod excel;
mod merge;
mod model;
mod places;
pub mod store;
pub mod tariffs;

pub use compute::{monthly_cap_min, recompute, recompute_driver};
pub use config::CatbSettings;
pub use merge::{merge_rows, new_sheet};
pub use model::*;

use crate::Result;
use crate::settings::Settings;

/// Puts an answer of the API into the month's sheet (a new one when `previous` is another month) and recomputes it.
pub fn apply_download(previous: Option<Sheet>, year: i32, month: u32, parsed: &api::Parsed, fetched_at: &str, settings: &Settings) -> Sheet {
    let mut sheet = previous.filter(|s| (s.year, s.month) == (year, month)).unwrap_or_else(|| new_sheet(year, month));
    merge_rows(&mut sheet, &parsed.rows, parsed.skipped, fetched_at);
    if parsed.rows.is_empty() {
        sheet.notices.push("API-ul nu a trimis nicio cursă pentru luna aceasta.".into());
    }
    recompute(&mut sheet, settings);
    sheet
}

/// Suggested name of the Excel file, e.g. `Pontaj_CategoriaB_10_2026.xlsx`.
pub fn suggested_file_name(sheet: &Sheet) -> String {
    format!("Pontaj_CategoriaB_{:02}_{}.xlsx", sheet.month, sheet.year)
}

/// Downloads a month from the API and merges it into `previous`, keeping everything the office changed.
pub fn download_month(previous: Option<Sheet>, year: i32, month: u32, settings: &Settings) -> Result<Sheet> {
    let parsed = api::fetch_month(&settings.catb, year, month)?;
    let now = chrono::Local::now().format("%d.%m.%Y %H:%M").to_string();
    Ok(apply_download(previous, year, month, &parsed, &now, settings))
}
