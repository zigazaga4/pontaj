//! "Categoria B": the pontaj of the drivers whose trips come from the ALT API instead of a tachograph PDF.

pub mod api;
mod cap;
mod category_d;
mod compute;
mod config;
pub mod excel;
mod hidden;
mod merge;
mod model;
mod overlap;
mod places;
mod pontaj;
pub mod reconcile;
mod reconcile_trips;
pub mod store;
pub mod tariffs;
mod transfers;
mod weeks;

pub use compute::{monthly_cap_min, recompute, recompute_driver};
pub use config::CatbSettings;
pub use hidden::HiddenDriver;
pub use merge::{borrow_neighbors, merge_rows, new_sheet};
pub use model::*;
pub use overlap::{resolve_overlap, Resolved};
pub use transfers::move_transfer;

use crate::settings::Settings;

/// Puts an answer of the API into the month's sheet (a new one when `previous` is another month) and recomputes it.
/// The API's version of each trip replaces the sheet's (corrections of a car, an hour, a place; cancelled trips leave), but
/// whatever the office changed stays as the office left it (see [`merge`]).
pub fn apply_download(previous: Option<Sheet>, year: i32, month: u32, parsed: &api::Parsed, fetched_at: &str, settings: &Settings) -> Sheet {
    let mut sheet = previous.filter(|s| (s.year, s.month) == (year, month)).unwrap_or_else(|| new_sheet(year, month));
    // The drivers taken out of the pontaj stay out: the sheet is brought in line with the list first (one added back since
    // is merged like any other below), and their trips go aside instead of into the sheet.
    let taken_out = &settings.catb.hidden_drivers;
    hidden::apply(&mut sheet, taken_out);
    // Only the trips that leave in the month belong to the sheet. Those of the days around it tell where each driver was
    // when the month began and where he went when it ended (see `Driver::before`).
    let (inside, around) = merge::split_month(&parsed.rows, year, month);
    // Trips on category D vehicles (by the make and seats of the car) are on the tachograph: they do not enter Categoria B.
    // It is decided trip by trip on every download: a trip whose car was corrected in the API comes back, one that is now on
    // a category D car leaves (unless the office changed it), and a driver with no trip left leaves the list.
    let (rows, mut excluded) = category_d::split(&inside, &settings.catb);
    excluded.retain(|d| !hidden::is_hidden(taken_out, d.id));
    let around = category_d::keep_small(&around, &settings.catb);
    let (rows_aside, rows): (Vec<_>, Vec<_>) = rows.into_iter().partition(|row| hidden::is_hidden(taken_out, row.driver_id));
    // An answer with no rows at all is a hiccup of the API, not a month without trips: nothing is taken out of the sheet then.
    merge::sync_rows(&mut sheet, &rows, parsed.skipped, fetched_at, !parsed.rows.is_empty());
    hidden::keep_rows(&mut sheet, &rows_aside, fetched_at);
    if parsed.without_neighbors {
        sheet.notices.push("API-ul a refuzat zilele din jurul lunii, așa că nu știu unde era fiecare șofer la începutul și la sfârșitul lunii: deplasările dintre luni nu se generează.".into());
    } else {
        merge::set_neighbors(&mut sheet, &around);
    }
    sheet.notices.extend(category_d::notices(&excluded, &settings.catb));
    if inside.is_empty() {
        sheet.notices.push("API-ul nu a trimis nicio cursă pentru luna aceasta.".into());
    }
    recompute(&mut sheet, settings);
    sheet
}

/// Makes a saved month ready to show: the list of drivers taken out of the pontaj is applied (to the neighbors too, so a
/// driver added back finds the drives between months), the drives of the days around the month are borrowed from the months
/// saved beside it, and everything is recomputed with the current settings.
pub fn open_saved(sheet: &mut Sheet, mut previous: Option<Sheet>, mut next: Option<Sheet>, settings: &Settings) {
    let taken_out = &settings.catb.hidden_drivers;
    for neighbor in previous.iter_mut().chain(next.iter_mut()) {
        hidden::apply(neighbor, taken_out);
    }
    hidden::apply(sheet, taken_out);
    borrow_neighbors(sheet, previous.as_ref(), next.as_ref());
    recompute(sheet, settings);
}

/// Suggested name of the Excel file, e.g. `Pontaj_CategoriaB_10_2026.xlsx`.
pub fn suggested_file_name(sheet: &Sheet) -> String {
    format!("Pontaj_CategoriaB_{:02}_{}.xlsx", sheet.month, sheet.year)
}

/// Merges an answer of the API (see [`api::fetch_month`]) into `previous`, keeping everything the office changed. The
/// download and the merge are two steps so that the screen can merge into the sheet as it is when the answer arrives.
pub fn merge_download(previous: Option<Sheet>, year: i32, month: u32, parsed: &api::Parsed, settings: &Settings) -> Sheet {
    let now = chrono::Local::now().format("%d.%m.%Y %H:%M").to_string();
    apply_download(previous, year, month, parsed, &now, settings)
}
