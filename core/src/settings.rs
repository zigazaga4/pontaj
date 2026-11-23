//! User-tunable rules. Defaults reproduce how the payroll team builds the
//! timesheet by hand from the tachograph report.

use serde::{Deserialize, Serialize};

use crate::catb::CatbSettings;
use crate::roster::StaffMember;
use crate::time::parse_hm;
use crate::timesheet::display_name;

/// Where the night minutes of a day come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NightSource {
    /// The report's own "Noapte" column (22:00–06:00, computed on the real activity).
    Card,
    /// Overlap of the written work intervals with the night window (00:00–07:00 by default).
    #[default]
    Intervals,
}

/// Bumped whenever a default changes in a way that must also reach settings saved by older versions.
/// 2: night hours are 00:00–07:00, measured on the written intervals.
pub const SETTINGS_VERSION: u32 = 2;

/// Settings files written before the version existed count as version 0.
fn legacy_version() -> u32 {
    0
}

/// How the start/end times of a day are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntervalMode {
    /// Two intervals on a `slot_min` grid that add up to the day's hours.
    #[default]
    Grid,
    /// The real times from the tachograph (first start to last end, minute precision).
    Card,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Which generation of defaults this file was saved with (see [`SETTINGS_VERSION`]).
    #[serde(default = "legacy_version")]
    pub settings_version: u32,
    /// Company name for the header. Empty → taken from the PDF ("NUME DE CONT").
    pub company_name: String,
    pub cui: String,
    pub default_function: String,
    /// Names written in capitals without Romanian diacritics (POPESCU ION).
    pub plain_names: bool,
    /// Leftover minutes from which a day rounds up to the next whole hour.
    pub round_up_from_min: i32,
    /// Granularity of the start/end times written in the timesheet.
    pub slot_min: i32,
    /// Break inserted between the two work intervals when the shift is tight.
    pub break_min: i32,
    /// Activities closer than this are treated as the same shift (cross-midnight).
    pub merge_gap_min: i32,
    /// How start/end times are written in the timesheet.
    pub interval_mode: IntervalMode,
    pub night_source: NightSource,
    /// Night window, used when the night comes from the intervals (or on days without card data).
    pub night_start: String,
    pub night_end: String,
    /// Night hours are written only when the night work reaches this many minutes (0 = always).
    pub night_threshold_min: i32,
    pub hours_per_day: i32,
    /// Start time used when hours are typed by hand on a day without PDF data.
    pub default_start: String,
    pub prepared_by: String,
    pub approved_by: String,
    /// Rules and API access of the "Categoria B" mode (trips read from the ALT API).
    pub catb: CatbSettings,
    /// The truck drivers (tir), by name: a base kept so that it is always known who they are, also in the months when they are
    /// missing from the tachograph. They are green in the foaie colectivă and have their own tab in the tachograph mode.
    /// A file saved before this existed starts with the list of [`crate::roster::seed`].
    pub tir_drivers: Vec<String>,
    /// The office, garage and city staff, each with the plan their hours are inferred from (see [`crate::roster`]).
    /// A file saved before this existed starts with the list of [`crate::roster::seed`].
    pub roster: Vec<StaffMember>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            company_name: String::new(),
            cui: String::new(),
            default_function: "Şofer de autoturisme".into(),
            plain_names: true,
            round_up_from_min: 20,
            slot_min: 30,
            break_min: 60,
            merge_gap_min: 30,
            interval_mode: IntervalMode::default(),
            night_source: NightSource::default(),
            night_start: "00:00".into(),
            night_end: "07:00".into(),
            night_threshold_min: 180,
            hours_per_day: 8,
            default_start: "08:00".into(),
            prepared_by: String::new(),
            approved_by: String::new(),
            catb: CatbSettings::default(),
            tir_drivers: crate::roster::seed::tir_drivers(),
            roster: crate::roster::seed::staff(),
        }
    }
}

impl Settings {
    /// Brings settings saved by an older version up to the current defaults.
    /// Version 2 moved the night to 00:00–07:00 (from the written intervals); everything else the user set is kept.
    pub fn migrated(mut self) -> Self {
        if self.settings_version < 2 {
            let current = Settings::default();
            self.night_source = current.night_source;
            self.night_start = current.night_start;
            self.night_end = current.night_end;
        }
        self.settings_version = SETTINGS_VERSION;
        self
    }

    pub fn night_window(&self) -> (i32, i32) {
        (parse_hm(&self.night_start).unwrap_or(0), parse_hm(&self.night_end).unwrap_or(7 * 60))
    }

    pub fn default_start_min(&self) -> i32 {
        parse_hm(&self.default_start).unwrap_or(8 * 60)
    }

    /// Is this person on the list of truck drivers? Names are compared the way the pontaj writes them (capitals, no diacritics).
    pub fn is_tir(&self, name: &str) -> bool {
        let key = display_name(name, true);
        self.tir_drivers.iter().any(|listed| display_name(listed, true) == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_defaults_to_midnight_until_seven_on_the_written_intervals() {
        let s = Settings::default();
        assert_eq!(s.night_source, NightSource::Intervals);
        assert_eq!(s.night_window(), (0, 7 * 60));
        assert_eq!(s.settings_version, SETTINGS_VERSION);
    }

    #[test]
    fn old_settings_files_are_moved_to_the_new_night_window() {
        // A file saved by 0.2.x: no version, night from the report column, 22:00–06:00; the user's other choices stay.
        let old = r#"{ "nightSource": "card", "nightStart": "22:00", "nightEnd": "06:00", "nightThresholdMin": 240, "companyName": "EXEMPLU SRL" }"#;
        let loaded: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.settings_version, 0, "a file without the version counts as legacy");
        let s = loaded.migrated();
        assert_eq!((s.night_source, s.night_window()), (NightSource::Intervals, (0, 7 * 60)));
        assert_eq!((s.night_threshold_min, s.company_name.as_str()), (240, "EXEMPLU SRL"));
        assert_eq!(s.settings_version, SETTINGS_VERSION);
    }

    #[test]
    fn a_file_saved_before_the_staff_existed_starts_with_the_staff_and_a_list_the_office_emptied_stays_empty() {
        let old: Settings = serde_json::from_str(r#"{ "companyName": "EXEMPLU SRL" }"#).unwrap();
        assert!(!old.roster.is_empty() && !old.tir_drivers.is_empty(), "the people of October are there from the start");
        assert!(old.is_tir(&old.tir_drivers[0].to_lowercase()), "names are compared the way the pontaj writes them");
        let emptied: Settings = serde_json::from_str(r#"{ "roster": [], "tirDrivers": [] }"#).unwrap();
        assert!(emptied.roster.is_empty() && emptied.tir_drivers.is_empty());
    }

    #[test]
    fn current_settings_are_left_alone_by_the_migration() {
        // Someone on the new version who picks the old 22:00–06:00 preset keeps it.
        let chosen = Settings { night_source: NightSource::Card, night_start: "22:00".into(), night_end: "06:00".into(), ..Settings::default() };
        let json = serde_json::to_string(&chosen).unwrap();
        let s: Settings = serde_json::from_str::<Settings>(&json).unwrap().migrated();
        assert_eq!((s.night_source, s.night_window()), (NightSource::Card, (22 * 60, 6 * 60)));
    }
}
