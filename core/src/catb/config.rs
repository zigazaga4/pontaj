//! Settings of the "Categoria B" mode. Every number the rules use can be changed from the settings panel.

use serde::{Deserialize, Serialize};

use super::places;
use super::tariffs::{default_tariffs, Tariff};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatbSettings {
    /// `https://api.alt-go.ro/api`; the trips are read from `{api_base}/external/driver-trips`.
    pub api_base: String,
    /// A key typed by hand. Optional: the program is built with the key of the ALT API inside (see `build.rs`),
    /// and this one, when filled in, is used instead.
    pub api_key: String,
    /// What a pair of regular trips (Constanța → Otopeni and back) pays; each trip counts half.
    pub pair_pay: f64,
    /// Waiting between two trips that is already part of the price of the day.
    pub station_included_min: i32,
    /// Added for every hour of waiting beyond the included time.
    pub station_rate: f64,
    /// From this much waiting the app warns that the driver probably left and came back later.
    pub station_warn_min: i32,
    /// A longer pause is not counted by itself: the office decides what it was (stayed, passenger, day off, excursion).
    pub long_pause_min: i32,
    /// Private/VIP/charter trips: their known driving time counts in the hours of the day (it is never counted as waiting).
    pub count_other_trips: bool,
    /// A private/VIP trip between Constanța and Otopeni, or any trip with the Otopeni/Bucharest end on one side and any place
    /// on the other, even outside the county (Otopeni → Năvodari, Tulcea → Otopeni), is a regular leg (same price, same
    /// waiting rules).
    pub main_route_private_regular: bool,
    /// "Rezolvă suprapunerea": the least a trip keeps of its (estimated) duration when its time is moved to make room
    /// for the next one, in percent.
    pub overlap_keep_pct: i32,
    /// When a regular trip starts somewhere the driver's last one did not end (Otopeni on one side), the app adds the
    /// drive that explains it: an empty car to Otopeni, or the way back from it. Off: nothing is added.
    pub generate_transfers: bool,
    /// How long such a generated drive lasts (the office can still change each one).
    pub transfer_min: i32,
    /// A day with hours is brought up to `hours_per_day` (8): the missing time is added before the first departure and
    /// after the last arrival (getting ready, handing the car over). Off: a day has only the hours the trips and the
    /// waiting give.
    pub fill_day: bool,
    /// The most that is added on one side of a day (before the first departure, after the last arrival), in minutes. The
    /// usual two trips of 3 h get 1 h on each side; a day with a single trip of 3 h gets 2 h 30 min on each side.
    pub fill_max_side_min: i32,
    /// Amounts in the Excel file. Off: the file has hours and warnings only.
    pub show_money: bool,
    /// A driver who drove a category D vehicle is not Categoria B: he is on the tachograph. On: such drivers are left
    /// out of the sheet when the month is downloaded.
    pub exclude_d_drivers: bool,
    /// A vehicle with more passenger seats than this needs a category D licence (8 passengers + the driver is still B).
    pub small_vehicle_max_seats: i32,
    /// Makes/models that are category D whatever the seats say (also when the API gives no seats). Matched inside the
    /// model name, ignoring case and diacritics.
    pub d_models: Vec<String>,
    pub tariffs: Vec<Tariff>,
    /// Places of Constanța county that the built-in list misses (written like the ALT app writes them).
    /// A private trip to a place that is neither on the regular route nor in the county is an excursion.
    pub extra_local_places: Vec<String>,
}

impl Default for CatbSettings {
    fn default() -> Self {
        Self {
            api_base: "https://api.alt-go.ro/api".into(),
            api_key: String::new(),
            pair_pay: 240.0,
            station_included_min: 180,
            station_rate: 10.0,
            station_warn_min: 360,
            long_pause_min: 720,
            count_other_trips: true,
            main_route_private_regular: true,
            overlap_keep_pct: 50,
            generate_transfers: true,
            transfer_min: 180,
            fill_day: true,
            fill_max_side_min: 180,
            show_money: true,
            exclude_d_drivers: true,
            small_vehicle_max_seats: 8,
            d_models: ["Tourismo", "Setra", "Travego"].map(String::from).to_vec(),
            tariffs: default_tariffs(),
            extra_local_places: Vec::new(),
        }
    }
}

/// The key put into the program when it was built; empty if the build had none.
const BUILT_IN_KEY: &str = env!("PONTAJ_BUILT_IN_ALT_KEY");

impl CatbSettings {
    /// The key the requests use: the one typed in the settings, otherwise the one built into the program.
    pub fn effective_key(&self) -> &str {
        let own = self.api_key.trim();
        if own.is_empty() {
            BUILT_IN_KEY
        } else {
            own
        }
    }

    /// Price of one regular leg.
    pub fn leg_pay(&self) -> f64 {
        self.pair_pay / 2.0
    }

    /// Is this a category D vehicle? By its passenger seats (more than `small_vehicle_max_seats`) or by its make/model
    /// (one of `d_models`). Unknown seats (0) and an unknown model never make a vehicle D.
    pub fn is_d_vehicle(&self, model: &str, seats: i32) -> bool {
        if seats > self.small_vehicle_max_seats {
            return true;
        }
        let model = places::normalize(model);
        !model.is_empty() && self.d_models.iter().map(|m| places::normalize(m)).any(|m| !m.is_empty() && model.contains(&m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vehicle_is_category_d_by_its_seats_or_by_its_make() {
        let cfg = CatbSettings::default();
        // The ALT fleet: small vehicles up to 8 passengers, then 17–20 (Sprinter) and the coaches.
        for (model, seats) in [("VITO", 8), ("Vito alb", 8), ("TRAFIC", 8), ("V-CLASS", 6), ("OCTAVIA", 3), ("PASSAT", 3), ("Touran", 4)] {
            assert!(!cfg.is_d_vehicle(model, seats), "{model} {seats} is a small vehicle");
        }
        for (model, seats) in [("SPRINTER", 20), ("SPRINTER", 17), ("TOURISMO", 55), ("SETRA", 50), ("TRAVEGO", 50)] {
            assert!(cfg.is_d_vehicle(model, seats), "{model} {seats} is category D");
        }
        assert!(cfg.is_d_vehicle("Tourismo RHD", 0), "a coach make counts even when the API gives no seats");
        assert!(!cfg.is_d_vehicle("", 0) && !cfg.is_d_vehicle("SPRINTER", 0), "nothing known = not D (Sprinter is D only by its seats)");
        assert!(!cfg.is_d_vehicle("SPRINTER", 8), "a Sprinter with 8 seats is a small vehicle");
        let mut cfg = cfg;
        cfg.small_vehicle_max_seats = 6;
        assert!(cfg.is_d_vehicle("VITO", 8), "the seat limit is a setting");
        cfg.d_models = vec!["  ".into()];
        assert!(!cfg.is_d_vehicle("TOURISMO", 0), "an empty entry in the list matches nothing");
    }

    #[test]
    fn a_key_typed_in_the_settings_wins_over_the_built_in_one() {
        let mut cfg = CatbSettings::default();
        assert_eq!(cfg.effective_key(), BUILT_IN_KEY);
        cfg.api_key = "  cheie-proprie  ".into();
        assert_eq!(cfg.effective_key(), "cheie-proprie");
        cfg.api_key = "   ".into();
        assert_eq!(cfg.effective_key(), BUILT_IN_KEY);
    }
}
