//! Drivers the office took out of the pontaj ("scoși", the "×" next to a driver). The list is in the settings
//! ([`CatbSettings::hidden_drivers`]), so it holds for every month, past and future; each sheet moves the listed drivers into
//! [`Sheet::removed_drivers`] (with everything the office did to them) and moves them back when they leave the list. Nothing
//! is thrown away: adding a driver back brings him as he was, and a month downloaded while he was out still has his trips.

use serde::{Deserialize, Serialize};

use super::api::ApiRow;
use super::merge;
use super::model::{Driver, Sheet};

/// A driver taken out of the pontaj. The name is only for showing the list (the id is what the API knows him by).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HiddenDriver {
    pub id: i64,
    pub name: String,
}

pub fn is_hidden(hidden: &[HiddenDriver], id: i64) -> bool {
    hidden.iter().any(|d| d.id == id)
}

/// A driver put aside keeps what the office decided and what the API said, not what was computed from it (that is
/// recomputed when he comes back).
fn stripped(mut driver: Driver) -> Driver {
    driver.free_days.clear();
    driver.pauses.clear();
    driver.days.clear();
    driver.totals = Default::default();
    driver.alerts.clear();
    driver.proposals.clear();
    driver
}

/// Brings the sheet in line with the list: listed drivers go to `removed_drivers`, drivers no longer listed come back.
/// Safe to call any number of times; the sheet must be recomputed afterwards (the drivers that came back have no totals).
pub fn apply(sheet: &mut Sheet, hidden: &[HiddenDriver]) {
    let (back, still_out): (Vec<Driver>, Vec<Driver>) = std::mem::take(&mut sheet.removed_drivers).into_iter().partition(|d| !is_hidden(hidden, d.id));
    sheet.removed_drivers = still_out;
    for driver in back {
        // He cannot be in both places; whoever is already in the sheet stays.
        if !sheet.drivers.iter().any(|d| d.id == driver.id) {
            sheet.drivers.push(driver);
        }
    }

    let (out, staying): (Vec<Driver>, Vec<Driver>) = std::mem::take(&mut sheet.drivers).into_iter().partition(|d| is_hidden(hidden, d.id));
    sheet.drivers = staying;
    for driver in out {
        sheet.removed_drivers.retain(|d| d.id != driver.id);
        sheet.removed_drivers.push(stripped(driver));
    }
    sheet.removed_drivers.sort_by_cached_key(|d| d.name.to_lowercase());
}

/// The trips a download brings for drivers who are out: they join what the sheet kept of those drivers (new ones only, as
/// everywhere else), so adding one back later needs no new download. They are not reported in the notices of the sheet.
pub(super) fn keep_rows(sheet: &mut Sheet, rows: &[ApiRow], fetched_at: &str) {
    if rows.is_empty() {
        return;
    }
    let mut aside = Sheet { year: sheet.year, month: sheet.month, drivers: std::mem::take(&mut sheet.removed_drivers), ..Default::default() };
    merge::merge_rows(&mut aside, rows, 0, fetched_at);
    sheet.removed_drivers = aside.drivers.into_iter().map(stripped).collect();
    sheet.removed_drivers.sort_by_cached_key(|d| d.name.to_lowercase());
}

#[cfg(test)]
mod tests {
    use super::super::api::Parsed;
    use super::super::compute::recompute;
    use super::super::config::CatbSettings;
    use super::super::merge::tests::{back, row};
    use super::super::model::{PauseEdit, PauseState};
    use super::super::{apply_download, new_sheet, open_saved};
    use super::*;
    use crate::settings::Settings;

    fn who(id: i64, name: &str) -> HiddenDriver {
        HiddenDriver { id, name: name.into() }
    }

    fn october_rows() -> Vec<ApiRow> {
        vec![
            row(1, "Popescu Ion", "eco-1", "2026-10-05 05:00", "2026-10-05 08:00"),
            back(row(1, "Popescu Ion", "eco-2", "2026-10-05 10:00", "2026-10-05 13:00")),
            row(2, "Ionescu Vasile", "eco-3", "2026-10-06 05:00", "2026-10-06 08:00"),
        ]
    }

    fn with_hidden(ids: &[(i64, &str)]) -> Settings {
        let mut settings = Settings::default();
        settings.catb.hidden_drivers = ids.iter().map(|(id, name)| who(*id, name)).collect();
        settings
    }

    fn download(previous: Option<Sheet>, month: u32, rows: Vec<ApiRow>, settings: &Settings) -> Sheet {
        apply_download(previous, 2026, month, &Parsed { rows, ..Default::default() }, "x", settings)
    }

    fn ids(drivers: &[Driver]) -> Vec<i64> {
        drivers.iter().map(|d| d.id).collect()
    }

    #[test]
    fn a_driver_taken_out_leaves_the_totals_and_comes_back_exactly_as_he_was() {
        let mut sheet = download(None, 10, october_rows(), &Settings::default());
        let popescu = sheet.drivers.iter_mut().find(|d| d.id == 1).unwrap();
        popescu.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, note: "foaia: a stat".into(), ..Default::default() });
        popescu.leaves.insert(chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(), "CO".into());
        recompute(&mut sheet, &Settings::default());
        let before = sheet.drivers.iter().find(|d| d.id == 1).unwrap().clone();
        assert!(before.totals.total_min > 0);

        // Out: not in the list of the sheet, kept aside without anything computed.
        recompute(&mut sheet, &with_hidden(&[(1, "Popescu Ion")]));
        assert_eq!(ids(&sheet.drivers), [2]);
        assert_eq!(ids(&sheet.removed_drivers), [1]);
        let aside = &sheet.removed_drivers[0];
        assert_eq!((&aside.trips, &aside.pause_edits, &aside.leaves), (&before.trips, &before.pause_edits, &before.leaves));
        assert!(aside.days.is_empty() && aside.pauses.is_empty());

        // Doing it again changes nothing.
        let once = sheet.clone();
        recompute(&mut sheet, &with_hidden(&[(1, "Popescu Ion")]));
        assert_eq!(sheet, once);

        // Back: all of it, recomputed.
        recompute(&mut sheet, &Settings::default());
        assert!(sheet.removed_drivers.is_empty());
        assert_eq!(ids(&sheet.drivers), [2, 1], "sorted by name: Ionescu, Popescu");
        assert_eq!(sheet.drivers.iter().find(|d| d.id == 1), Some(&before));
    }

    #[test]
    fn a_download_while_a_driver_is_out_keeps_his_trips_aside_and_says_nothing_about_him() {
        let settings = with_hidden(&[(1, "Popescu Ion")]);
        let mut sheet = download(None, 10, october_rows(), &settings);
        assert_eq!(ids(&sheet.drivers), [2]);
        assert_eq!(ids(&sheet.removed_drivers), [1]);
        assert_eq!(sheet.removed_drivers[0].trips.len(), 2, "a month never opened before has his trips too");
        assert!(sheet.notices.iter().all(|n| !n.contains("Popescu")), "{:?}", sheet.notices);

        // The next download brings one more trip for him and nothing for the others; still nothing about him in the notices.
        let mut rows = october_rows();
        rows.push(row(1, "Popescu Ion", "eco-4", "2026-10-07 05:00", "2026-10-07 08:00"));
        sheet = download(Some(sheet), 10, rows, &settings);
        assert_eq!(sheet.removed_drivers[0].trips.len(), 3);
        assert!(sheet.notices.iter().all(|n| !n.contains("Popescu")), "{:?}", sheet.notices);

        // Added back, he has all three trips without downloading again.
        recompute(&mut sheet, &Settings::default());
        let popescu = sheet.drivers.iter().find(|d| d.id == 1).unwrap();
        assert_eq!(popescu.trips.len(), 3);
        assert!(popescu.totals.total_min > 0);
        assert!(sheet.removed_drivers.is_empty());
    }

    #[test]
    fn a_driver_added_back_before_a_download_is_merged_like_any_other() {
        let mut sheet = download(None, 10, october_rows(), &with_hidden(&[(1, "Popescu Ion")]));
        let mut rows = october_rows();
        rows.push(row(1, "Popescu Ion", "eco-4", "2026-10-07 05:00", "2026-10-07 08:00"));
        // The list is empty now: the download first brings him back, then adds only what is new.
        sheet = download(Some(sheet), 10, rows, &Settings::default());
        assert!(sheet.removed_drivers.is_empty());
        assert_eq!(ids(&sheet.drivers), [2, 1]);
        assert_eq!(sheet.drivers.iter().find(|d| d.id == 1).unwrap().trips.len(), 3);
    }

    #[test]
    fn the_list_holds_for_every_month() {
        let settings = with_hidden(&[(2, "Ionescu Vasile")]);
        let october = download(None, 10, october_rows(), &settings);
        let november = download(None, 11, vec![row(2, "Ionescu Vasile", "eco-8", "2026-11-03 05:00", "2026-11-03 08:00"), row(3, "Nou Gheorghe", "eco-9", "2026-11-04 05:00", "2026-11-04 08:00")], &settings);
        assert_eq!((ids(&october.drivers), ids(&october.removed_drivers)), (vec![1], vec![2]));
        assert_eq!((ids(&november.drivers), ids(&november.removed_drivers)), (vec![3], vec![2]));

        // A month saved before the driver was taken out has him in the list: opening it applies the list.
        let mut saved = download(None, 12, vec![row(2, "Ionescu Vasile", "eco-7", "2026-12-03 05:00", "2026-12-03 08:00")], &Settings::default());
        assert_eq!(ids(&saved.drivers), [2]);
        open_saved(&mut saved, None, None, &settings);
        assert!(saved.drivers.is_empty());
        assert_eq!(ids(&saved.removed_drivers), [2]);
    }

    #[test]
    fn a_driver_added_back_finds_the_drives_between_months_again() {
        // Popescu's last trip of October ends in Constanța; his first of November leaves from Otopeni.
        let october_rows = vec![back(row(1, "Popescu Ion", "eco-30", "2026-10-31 11:00", "2026-10-31 14:00"))];
        let november_rows = vec![back(row(1, "Popescu Ion", "eco-31", "2026-11-01 05:30", "2026-11-01 08:30"))];
        let october = download(None, 10, october_rows, &Settings::default());
        let november = download(None, 11, november_rows, &Settings::default());
        let generated = |sheet: &Sheet| sheet.drivers.iter().flat_map(|d| d.trips.iter()).filter(|t| t.generated).count();

        // Out of the list in the saved files: nothing to generate and nothing shown.
        let out = with_hidden(&[(1, "Popescu Ion")]);
        let mut opened = november.clone();
        open_saved(&mut opened, Some(october.clone()), None, &out);
        assert!(opened.drivers.is_empty());

        // Back in the list, the drives between months are found again from the sheets beside it.
        let mut opened = opened;
        open_saved(&mut opened, Some(october), None, &Settings::default());
        assert_eq!(generated(&opened), 1);
    }

    #[test]
    fn the_list_is_part_of_the_saved_settings_and_old_settings_have_none() {
        let old: CatbSettings = serde_json::from_str("{}").unwrap();
        assert!(old.hidden_drivers.is_empty());
        let json = serde_json::to_string(&with_hidden(&[(7, "Ionescu Vasile")]).catb).unwrap();
        assert!(json.contains("\"hiddenDrivers\":[{\"id\":7,\"name\":\"Ionescu Vasile\"}]"), "{json}");
        let again: CatbSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(again.hidden_drivers, [who(7, "Ionescu Vasile")]);
    }

    #[test]
    fn a_sheet_without_hidden_drivers_is_not_touched() {
        let mut sheet = new_sheet(2026, 10);
        apply(&mut sheet, &[]);
        assert_eq!(sheet, new_sheet(2026, 10));
    }
}
