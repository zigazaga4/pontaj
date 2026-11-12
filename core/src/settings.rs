//! User-tunable rules. Defaults reproduce how the payroll team builds the
//! timesheet by hand from the tachograph report.

use serde::{Deserialize, Serialize};

use crate::time::parse_hm;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
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
    pub night_start: String,
    pub night_end: String,
    /// Night hours are written only when the night overlap reaches this many minutes.
    pub night_threshold_min: i32,
    pub hours_per_day: i32,
    /// Start time used when hours are typed by hand on a day without PDF data.
    pub default_start: String,
    pub prepared_by: String,
    pub approved_by: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            company_name: String::new(),
            cui: String::new(),
            default_function: "Şofer de autoturisme".into(),
            plain_names: true,
            round_up_from_min: 20,
            slot_min: 30,
            break_min: 60,
            merge_gap_min: 30,
            night_start: "22:00".into(),
            night_end: "07:00".into(),
            night_threshold_min: 180,
            hours_per_day: 8,
            default_start: "08:00".into(),
            prepared_by: String::new(),
            approved_by: String::new(),
        }
    }
}

impl Settings {
    pub fn night_window(&self) -> (i32, i32) {
        (parse_hm(&self.night_start).unwrap_or(22 * 60), parse_hm(&self.night_end).unwrap_or(7 * 60))
    }

    pub fn default_start_min(&self) -> i32 {
        parse_hm(&self.default_start).unwrap_or(8 * 60)
    }
}
