//! "Categoria B": the pontaj of the drivers whose trips come from the ALT API instead of a tachograph PDF.

pub mod api;
mod cap;
mod category_d;
mod compute;
mod config;
pub mod excel;
mod merge;
mod model;
mod overlap;
mod places;
mod pontaj;
pub mod store;
pub mod tariffs;
mod transfers;

pub use compute::{monthly_cap_min, recompute, recompute_driver};
pub use config::CatbSettings;
pub use merge::{borrow_neighbors, merge_rows, new_sheet};
pub use model::*;
pub use overlap::{resolve_overlap, Resolved};

use crate::Result;
use crate::settings::Settings;

/// Puts an answer of the API into the month's sheet (a new one when `previous` is another month) and recomputes it.
/// Only what the sheet does not have yet comes in: whatever is already there stays as the office left it.
pub fn apply_download(previous: Option<Sheet>, year: i32, month: u32, parsed: &api::Parsed, fetched_at: &str, settings: &Settings) -> Sheet {
    let mut sheet = previous.filter(|s| (s.year, s.month) == (year, month)).unwrap_or_else(|| new_sheet(year, month));
    // Only the trips that leave in the month belong to the sheet. Those of the days around it tell where each driver was
    // when the month began and where he went when it ended (see `Driver::before`).
    let (inside, around) = merge::split_month(&parsed.rows, year, month);
    // Drivers of category D (by the make and seats of their cars) are on the tachograph: they do not enter Categoria B.
    // One already in the sheet leaves with them, unless the office changed something about him (see the notice below).
    let (rows, excluded) = category_d::split(&inside, &settings.catb);
    sheet.drivers.retain(|driver| !excluded.iter().any(|d| d.id == driver.id) || merge::has_decisions(driver));
    merge_rows(&mut sheet, &rows, parsed.skipped, fetched_at);
    if parsed.without_neighbors {
        sheet.notices.push("API-ul a refuzat zilele din jurul lunii, așa că nu știu unde era fiecare șofer la începutul și la sfârșitul lunii: deplasările dintre luni nu se generează.".into());
    } else {
        merge::set_neighbors(&mut sheet, &around);
    }
    // One who was in the sheet with changes of the office stays; the notice says why instead of "gone from the API".
    for notice in &mut sheet.notices {
        if let Some(driver) = excluded.iter().find(|d| *notice == format!("{}: nu mai apare în API, dar are modificări; l-am păstrat.", d.name)) {
            *notice = format!("{}: e de categoria D (nu intră în Categoria B), dar ai modificări la el; l-am păstrat.", driver.name);
        }
    }
    sheet.notices.extend(category_d::notices(&excluded, &settings.catb));
    if inside.is_empty() {
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
