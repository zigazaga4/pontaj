//! Settings of the "Categoria B" mode. Every number the rules use can be changed from the settings panel.

use serde::{Deserialize, Serialize};

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
    /// A private/VIP trip between Constanța and Otopeni is a regular leg (same price, same waiting rules).
    pub main_route_private_regular: bool,
    /// Amounts in the Excel file. Off: the file has hours and warnings only.
    pub show_money: bool,
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
            show_money: true,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
