//! Trips on category D vehicles are not Categoria B: they are on the tachograph, so a download leaves them out.
//!
//! It is decided trip by trip, by the car the trip was driven with — its make/model and passenger seats as the API gives
//! them (see [`CatbSettings::is_d_vehicle`]). A driver who drives both has his small-car trips in the sheet and only the
//! category D ones leave; one whose trips of the month are all category D is not in the sheet at all. It is decided again
//! by every download, so a trip whose car is corrected in the API comes back (or leaves) at the next one.

use std::collections::BTreeMap;

use super::api::ApiRow;
use super::config::CatbSettings;

/// A driver with trips left out of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Excluded {
    pub id: i64,
    pub name: String,
    /// Trips on category D vehicles (left out) / on small vehicles (they stay in the sheet).
    pub d_trips: usize,
    pub small_trips: usize,
}

/// Is this trip on a category D vehicle (and the rule on)?
fn is_d(row: &ApiRow, cfg: &CatbSettings) -> bool {
    cfg.exclude_d_drivers && cfg.is_d_vehicle(&row.car_model, row.car_seats)
}

/// Splits the rows of a download: the rows that stay in Categoria B, and the drivers who have rows that leave.
pub fn split(rows: &[ApiRow], cfg: &CatbSettings) -> (Vec<ApiRow>, Vec<Excluded>) {
    if !cfg.exclude_d_drivers {
        return (rows.to_vec(), Vec::new());
    }
    let mut drivers: BTreeMap<i64, Excluded> = BTreeMap::new();
    for row in rows {
        let entry = drivers.entry(row.driver_id).or_insert_with(|| Excluded { id: row.driver_id, name: String::new(), d_trips: 0, small_trips: 0 });
        if entry.name.is_empty() {
            entry.name = row.driver_name.clone();
        }
        if is_d(row, cfg) {
            entry.d_trips += 1;
        } else {
            entry.small_trips += 1;
        }
    }
    drivers.retain(|_, d| d.d_trips > 0);
    let kept = keep_small(rows, cfg);
    let mut excluded: Vec<Excluded> = drivers.into_values().collect();
    for driver in &mut excluded {
        if driver.name.is_empty() {
            driver.name = format!("șofer {}", driver.id);
        }
    }
    excluded.sort_by_cached_key(|d| d.name.to_lowercase());
    (kept, excluded)
}

/// The rows on small cars (also used for the days around the month: a coach trip says nothing about where a Categoria B
/// driver was).
pub fn keep_small(rows: &[ApiRow], cfg: &CatbSettings) -> Vec<ApiRow> {
    rows.iter().filter(|row| !is_d(row, cfg)).cloned().collect()
}

fn trips_text(count: usize) -> String {
    if count == 1 { "1 cursă".into() } else { format!("{count} curse") }
}

/// The notices for the office: which trips are on the tachograph, and who is not in the sheet at all because of it.
pub fn notices(excluded: &[Excluded], cfg: &CatbSettings) -> Vec<String> {
    if excluded.is_empty() {
        return Vec::new();
    }
    let trips = excluded.iter().map(|d| d.d_trips).sum::<usize>();
    let who = excluded.iter().map(|d| format!("{} ({})", d.name, trips_text(d.d_trips))).collect::<Vec<_>>().join(", ");
    let mut out = vec![format!(
        "{} cu mașini de categoria D (peste {} locuri sau de modelele {}) nu intră în Categoria B, se pontează din tahograf: {who}.",
        trips_text(trips),
        cfg.small_vehicle_max_seats,
        cfg.d_models.join(", ")
    )];
    let only: Vec<&str> = excluded.iter().filter(|d| d.small_trips == 0).map(|d| d.name.as_str()).collect();
    if !only.is_empty() {
        let lead = if only.len() == 1 { "1 șofer a condus în lună doar mașini de categoria D și nu apare în foaie".to_string() } else { format!("{} șoferi au condus în lună doar mașini de categoria D și nu apar în foaie", only.len()) };
        out.push(format!("{lead}: {}.", only.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::super::model::TripKind;
    use super::*;
    use crate::settings::Settings;

    fn row(driver: i64, name: &str, trip: &str, model: &str, seats: i32) -> ApiRow {
        ApiRow {
            driver_id: driver,
            driver_name: name.into(),
            trip_id: trip.into(),
            kind: TripKind::Eco,
            label: String::new(),
            from: "Constanta".into(),
            to: "Aeroport Otopeni Bucuresti".into(),
            departure: parse_time("2026-10-05 05:00").unwrap(),
            arrival: parse_time("2026-10-05 08:00"),
            return_at: None,
            car: "CT-22-XDD".into(),
            car_model: model.into(),
            car_seats: seats,
            car_category: String::new(),
            passengers: 3,
        }
    }

    fn fleet() -> Vec<ApiRow> {
        vec![
            row(1, "Mic Ion", "eco-1", "VITO", 8),
            row(1, "Mic Ion", "eco-2", "PASSAT", 3),
            row(2, "Mare Vasile", "eco-3", "TOURISMO", 55),
            row(2, "Mare Vasile", "eco-4", "SPRINTER", 20),
            row(3, "Amestecat Gabriel", "eco-5", "SPRINTER", 20),
            row(3, "Amestecat Gabriel", "eco-6", "VITO", 8),
            row(3, "Amestecat Gabriel", "eco-7", "VITO", 8),
            row(4, "Fără Detalii", "eco-8", "", 0),
        ]
    }

    fn trip_ids(sheet: &super::super::model::Sheet, driver: i64) -> Vec<String> {
        sheet.drivers.iter().find(|d| d.id == driver).map(|d| d.trips.iter().map(|t| t.id.clone()).collect()).unwrap_or_default()
    }

    fn download(previous: Option<super::super::model::Sheet>, rows: Vec<ApiRow>, settings: &Settings) -> super::super::model::Sheet {
        super::super::apply_download(previous, 2026, 10, &super::super::api::Parsed { rows, ..Default::default() }, "x", settings)
    }

    #[test]
    fn only_the_trips_on_category_d_cars_leave() {
        let (kept, excluded) = split(&fleet(), &CatbSettings::default());
        let ids: Vec<&str> = kept.iter().map(|r| r.trip_id.as_str()).collect();
        assert_eq!(ids, ["eco-1", "eco-2", "eco-6", "eco-7", "eco-8"], "the small-car trips of everybody stay, also those of a mixed driver");
        let names: Vec<&str> = excluded.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Amestecat Gabriel", "Mare Vasile"], "sorted by name");
        let mixed = excluded.iter().find(|d| d.id == 3).unwrap();
        assert_eq!((mixed.d_trips, mixed.small_trips), (1, 2));
        let only_d = excluded.iter().find(|d| d.id == 2).unwrap();
        assert_eq!((only_d.d_trips, only_d.small_trips), (2, 0));
    }

    #[test]
    fn a_download_keeps_the_small_car_trips_of_a_mixed_driver_and_drops_one_who_drove_only_d() {
        let sheet = download(None, fleet(), &Settings::default());
        let names: Vec<&str> = sheet.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Amestecat Gabriel", "Fără Detalii", "Mic Ion"], "Mare Vasile drove only category D cars this month");
        assert_eq!(trip_ids(&sheet, 3), ["eco-6", "eco-7"]);
        assert_eq!(sheet.notices.len(), 2, "{:?}", sheet.notices);
        assert!(sheet.notices[0].starts_with("3 curse cu mașini de categoria D") && sheet.notices[0].contains("Amestecat Gabriel (1 cursă)") && sheet.notices[0].contains("Mare Vasile (2 curse)"), "{:?}", sheet.notices);
        assert!(sheet.notices[1].starts_with("1 șofer a condus în lună doar mașini de categoria D") && sheet.notices[1].contains("Mare Vasile"), "{:?}", sheet.notices);
    }

    #[test]
    fn a_trip_whose_car_is_corrected_in_the_api_comes_back_and_one_that_becomes_d_leaves() {
        let mut sheet = download(None, fleet(), &Settings::default());
        assert_eq!(trip_ids(&sheet, 3), ["eco-6", "eco-7"]);
        // Somebody in the other application put the Sprinter by mistake: eco-5 is really on a Vito (it comes back), eco-4 of
        // the driver who had only D cars too (he is back in the list), and eco-6 was moved to a Sprinter (it leaves).
        let mut corrected = fleet();
        for row in &mut corrected {
            match row.trip_id.as_str() {
                "eco-5" | "eco-4" => (row.car_model, row.car_seats) = ("VITO".into(), 8),
                "eco-6" => (row.car_model, row.car_seats) = ("SPRINTER".into(), 20),
                _ => {}
            }
        }
        sheet = download(Some(sheet), corrected.clone(), &Settings::default());
        assert_eq!(trip_ids(&sheet, 3), ["eco-5", "eco-7"].map(String::from), "{:?}", trip_ids(&sheet, 3));
        assert_eq!(trip_ids(&sheet, 2), ["eco-4"], "Mare Vasile has a small-car trip now, so he is in the list again");

        // A trip the office changed stays even if its car becomes D: what the office did is never taken away.
        sheet.drivers.iter_mut().find(|d| d.id == 3).unwrap().trips.iter_mut().find(|t| t.id == "eco-7").unwrap().edit.price = Some(100.0);
        for row in &mut corrected {
            if row.trip_id == "eco-7" {
                (row.car_model, row.car_seats) = ("SPRINTER".into(), 20);
            }
        }
        sheet = download(Some(sheet), corrected, &Settings::default());
        assert_eq!(trip_ids(&sheet, 3), ["eco-5", "eco-7"].map(String::from));
        assert!(sheet.notices.iter().any(|n| n.contains("modificată de tine") && n.contains("eco-7")), "{:?}", sheet.notices);
    }

    #[test]
    fn a_driver_whose_only_trips_become_d_leaves_the_list_unless_the_office_changed_something() {
        let mut sheet = download(None, vec![row(5, "Schimbat Dan", "eco-1", "VITO", 8), row(6, "Ținut Radu", "eco-2", "VITO", 8)], &Settings::default());
        sheet.drivers.iter_mut().find(|d| d.id == 6).unwrap().excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());
        let now_d = vec![row(5, "Schimbat Dan", "eco-1", "SPRINTER", 20), row(6, "Ținut Radu", "eco-2", "SPRINTER", 20)];
        sheet = download(Some(sheet), now_d, &Settings::default());
        let names: Vec<&str> = sheet.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Ținut Radu"], "the one with a day taken out by the office stays, with his notice");
        assert!(sheet.notices.iter().any(|n| n.starts_with("Ținut Radu:") && n.contains("l-am păstrat")), "{:?}", sheet.notices);
    }

    #[test]
    fn with_the_setting_off_everybody_stays() {
        let cfg = CatbSettings { exclude_d_drivers: false, ..CatbSettings::default() };
        let (kept, excluded) = split(&fleet(), &cfg);
        assert_eq!((kept.len(), excluded.len()), (8, 0));
        assert!(notices(&excluded, &cfg).is_empty());
    }

    #[test]
    fn the_notices_count_the_trips_per_driver_and_name_those_who_are_not_in_the_sheet() {
        let cfg = CatbSettings::default();
        let (_, excluded) = split(&fleet(), &cfg);
        let text = notices(&excluded, &cfg);
        assert_eq!(text.len(), 2);
        assert!(text[0].starts_with("3 curse cu mașini de categoria D"), "{}", text[0]);
        assert!(text[0].contains("peste 8 locuri") && text[0].contains("Tourismo") && text[0].contains("Amestecat Gabriel (1 cursă)") && text[0].contains("Mare Vasile (2 curse)"), "{}", text[0]);
        assert_eq!(text[1], "1 șofer a condus în lună doar mașini de categoria D și nu apare în foaie: Mare Vasile.");
        // Only a mixed driver: a single notice.
        let mixed = vec![excluded[0].clone()];
        assert_eq!(notices(&mixed, &cfg).len(), 1);
        assert!(notices(&[], &cfg).is_empty());
    }
}
