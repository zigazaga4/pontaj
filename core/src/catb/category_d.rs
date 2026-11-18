//! Category D drivers are not Categoria B: they work on the tachograph, so a download leaves them out.
//!
//! A driver counts as category D when he drove at least one category D vehicle in the month — judged by the make/model
//! and the passenger seats the API gives for the car (see [`CatbSettings::is_d_vehicle`]). All of his trips leave, also
//! those on small cars: a notice lists them so nothing disappears unseen.

use std::collections::BTreeMap;

use super::api::ApiRow;
use super::config::CatbSettings;

/// A driver left out of the sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Excluded {
    pub id: i64,
    pub name: String,
    /// Trips on category D vehicles / on small vehicles.
    pub d_trips: usize,
    pub small_trips: usize,
}

/// Splits the rows of a download: the rows of drivers who stay in Categoria B, and the drivers who leave.
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
        if cfg.is_d_vehicle(&row.car_model, row.car_seats) {
            entry.d_trips += 1;
        } else {
            entry.small_trips += 1;
        }
    }
    drivers.retain(|_, d| d.d_trips > 0);
    let kept = rows.iter().filter(|r| !drivers.contains_key(&r.driver_id)).cloned().collect();
    let mut excluded: Vec<Excluded> = drivers.into_values().collect();
    for driver in &mut excluded {
        if driver.name.is_empty() {
            driver.name = format!("șofer {}", driver.id);
        }
    }
    excluded.sort_by_cached_key(|d| d.name.to_lowercase());
    (kept, excluded)
}

fn trips_text(count: usize) -> String {
    if count == 1 { "1 cursă".into() } else { format!("{count} curse") }
}

/// The notices for the office: who left and why, and which of their trips on small cars are not counted here either.
pub fn notices(excluded: &[Excluded], cfg: &CatbSettings) -> Vec<String> {
    if excluded.is_empty() {
        return Vec::new();
    }
    let names = excluded.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ");
    let who = if excluded.len() == 1 { "1 șofer de categoria D nu intră".to_string() } else { format!("{} șoferi de categoria D nu intră", excluded.len()) };
    let mut out = vec![format!(
        "{who} în Categoria B (au condus mașini cu peste {} locuri sau de modelele {}): se pontează din tahograf. {names}.",
        cfg.small_vehicle_max_seats,
        cfg.d_models.join(", ")
    )];
    let mixed: Vec<String> = excluded.iter().filter(|d| d.small_trips > 0).map(|d| format!("{} ({})", d.name, trips_text(d.small_trips))).collect();
    if !mixed.is_empty() {
        out.push(format!("Dintre ei, au condus și mașini mici (aceste curse nu intră nici în Categoria B, verifică-le la tahograf sau oprește excluderea din Setări): {}.", mixed.join(", ")));
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

    #[test]
    fn a_driver_of_category_d_leaves_with_all_his_trips() {
        let (kept, excluded) = split(&fleet(), &CatbSettings::default());
        let ids: Vec<i64> = kept.iter().map(|r| r.driver_id).collect();
        assert_eq!(ids, vec![1, 1, 4], "only the drivers of small cars (and the one the API says nothing about) stay");
        let names: Vec<&str> = excluded.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Amestecat Gabriel", "Mare Vasile"], "sorted by name");
        let mixed = excluded.iter().find(|d| d.id == 3).unwrap();
        assert_eq!((mixed.d_trips, mixed.small_trips), (1, 2));
    }

    #[test]
    fn a_download_leaves_the_d_drivers_out_of_the_sheet_and_says_so() {
        let parsed = super::super::api::Parsed { rows: fleet(), ..Default::default() };
        let sheet = super::super::apply_download(None, 2026, 10, &parsed, "17.11.2026 10:00", &Settings::default());
        let names: Vec<&str> = sheet.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Fără Detalii", "Mic Ion"]);
        assert_eq!(sheet.notices.len(), 2, "{:?}", sheet.notices);
        assert!(sheet.notices[0].contains("2 șoferi de categoria D"), "{:?}", sheet.notices);
    }

    #[test]
    fn a_d_driver_the_office_already_changed_stays_with_a_clear_notice() {
        // First download with the exclusion off (as before this feature): everybody is in the sheet.
        let off = Settings { catb: CatbSettings { exclude_d_drivers: false, ..CatbSettings::default() }, ..Settings::default() };
        let parsed = super::super::api::Parsed { rows: fleet(), ..Default::default() };
        let mut sheet = super::super::apply_download(None, 2026, 10, &parsed, "1", &off);
        assert_eq!(sheet.drivers.len(), 4);
        sheet.drivers.iter_mut().find(|d| d.id == 2).unwrap().excluded_days.push(chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());

        // Download again with the exclusion on: Mare Vasile (D) leaves, except that he has a change by the office.
        let sheet = super::super::apply_download(Some(sheet), 2026, 10, &parsed, "2", &Settings::default());
        let names: Vec<&str> = sheet.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Fără Detalii", "Mare Vasile", "Mic Ion"], "Amestecat Gabriel had no changes, so he is gone");
        assert!(sheet.notices.iter().any(|n| n == "Mare Vasile: e de categoria D (nu intră în Categoria B), dar ai modificări la el; l-am păstrat."), "{:?}", sheet.notices);
        assert!(sheet.notices.iter().all(|n| !n.contains("nu mai apare în API")), "{:?}", sheet.notices);
    }

    #[test]
    fn with_the_setting_off_everybody_stays() {
        let cfg = CatbSettings { exclude_d_drivers: false, ..CatbSettings::default() };
        let (kept, excluded) = split(&fleet(), &cfg);
        assert_eq!((kept.len(), excluded.len()), (8, 0));
        assert!(notices(&excluded, &cfg).is_empty());
    }

    #[test]
    fn the_notices_name_everybody_and_the_small_trips_that_go_with_them() {
        let cfg = CatbSettings::default();
        let (_, excluded) = split(&fleet(), &cfg);
        let text = notices(&excluded, &cfg);
        assert_eq!(text.len(), 2);
        assert!(text[0].starts_with("2 șoferi de categoria D nu intră în Categoria B"), "{}", text[0]);
        assert!(text[0].contains("Mare Vasile") && text[0].contains("peste 8 locuri") && text[0].contains("Tourismo"), "{}", text[0]);
        assert!(text[1].contains("Amestecat Gabriel (2 curse)") && !text[1].contains("Mare Vasile"), "{}", text[1]);
        // One driver, none with small trips: a single notice, in the singular.
        let one = vec![excluded[1].clone()];
        let single = notices(&one, &cfg);
        assert_eq!(single.len(), 1);
        assert!(single[0].starts_with("1 șofer de categoria D nu intră"), "{}", single[0]);
    }
}
