//! Data of the "Categoria B" mode. Everything the API says (`Trip` base fields) is kept apart from what the
//! office changed (`TripEdit`, `PauseEdit`, `excluded_days`): a new download replaces the first and keeps the second.
//! The derived parts (`eff`, `pauses`, `days`, `totals`, `alerts`, `proposals`) are rebuilt by [`super::recompute`].

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::timesheet::DayInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TripKind {
    /// Regular timetable trip Constanța ↔ Otopeni.
    #[default]
    Eco,
    Private,
    Vip,
    /// Rented coach/minibus.
    Bus,
    /// Added by hand: the driver came back as a passenger (counts as a regular leg).
    Passenger,
    /// Made by the app: the drive (empty car, or as a passenger) that explains how the driver got from where one regular
    /// trip ended to where the next one starts (see [`super::transfers`]). Counts as a regular leg.
    Transfer,
}

impl TripKind {
    pub fn from_api(code: &str) -> Self {
        match code.trim().to_lowercase().as_str() {
            "prv" | "private" => Self::Private,
            "vip" => Self::Vip,
            "bus" => Self::Bus,
            _ => Self::Eco,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Eco => "Eco",
            Self::Private => "Privată",
            Self::Vip => "VIP",
            Self::Bus => "Autocar",
            Self::Passenger => "Pasager",
            Self::Transfer => "Deplasare",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    #[default]
    Info,
    Warn,
    Alert,
}

/// A remark for the office, in plain Romanian. `code` lets the screen group or filter them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Alert {
    pub code: String,
    pub level: Level,
    pub text: String,
}

impl Alert {
    pub fn new(code: &str, level: Level, text: impl Into<String>) -> Self {
        Self { code: code.into(), level, text: text.into() }
    }
}

/// What the office changed on a trip. Empty = the trip is as the API says.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TripEdit {
    pub departure: Option<NaiveDateTime>,
    pub arrival: Option<NaiveDateTime>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Price typed by hand (a private trip whose destination is not in the list, or a corrected amount).
    pub price: Option<f64>,
    /// The trip does not count (not made, or wrongly assigned).
    pub removed: bool,
}

/// A trip as it counts after the office's changes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Effective {
    pub departure: Option<NaiveDateTime>,
    pub arrival: Option<NaiveDateTime>,
    pub from: String,
    pub to: String,
    /// Known driving time (0 when the arrival is unknown).
    pub minutes: i32,
    /// A leg of the regular route: paid per leg and followed by waiting time.
    pub regular: bool,
    pub counts_hours: bool,
    /// Part of an excursion the office marked: it counts nowhere in Categoria B (the tachograph has it).
    pub excursion: bool,
    /// Estimated amount for this trip (leg price or exterior tariff).
    pub price: f64,
    /// Why the amount is what it is: "cursă", "exterior: Tulcea", "fără tarif"…
    pub price_note: String,
    /// The day the trip counts on in the pontaj: the day it leaves, except the way back of a trip to Bucharest/Otopeni
    /// (left at 14:30, arrived at 17:30, left again at 03:30 the next day: by the clock it is the next day, but the
    /// accountants write it on the day of the way there) and a trip that follows a staționare going over midnight. Both
    /// count on the day of the trip before them. Derived by [`super::recompute`].
    pub work_day: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trip {
    /// `trip.id` of the API (`eco-117599`); `manual-N` for trips added by hand.
    pub id: String,
    pub kind: TripKind,
    pub label: String,
    pub from: String,
    pub to: String,
    pub departure: NaiveDateTime,
    pub arrival: Option<NaiveDateTime>,
    pub return_at: Option<NaiveDateTime>,
    pub car: String,
    /// Make/model and passenger seats of the car, as the API gave them (shown next to the car; 0 seats = unknown).
    pub car_model: String,
    pub car_seats: i32,
    pub car_category: String,
    pub passengers: i32,
    pub manual: bool,
    /// Made by the app, not read from the API ([`TripKind::Transfer`]): rebuilt on every recompute, the office's edits on it stay.
    pub generated: bool,
    /// Whole days the office moved a generated drive from where the rules put it (negative = earlier). The rules keep their
    /// own times up to date on every recompute, and this shift is added to them (see [`super::transfers::move_transfer`]).
    pub moved_days: i32,
    pub edit: TripEdit,
    pub eff: Effective,
}

/// What the office decided about the time between two regular trips: a *staționare* (waiting at Otopeni, it counts) or
/// a *pauză* (the driver is at home, it counts nothing). Without a decision the rules choose by where he waits.
/// (Months saved with the older choices keep opening: "passenger" and "excursion" are read as a pause.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PauseState {
    /// Nothing decided: the rules choose (see [`PauseKind`]).
    #[default]
    Auto,
    /// Staționare: the whole pause is waiting time.
    Stayed,
    /// Pauză: no waiting time.
    #[serde(alias = "passenger", alias = "excursion")]
    Off,
}

/// The kinds of leave the office can pick for a day without any trip: (code of the timesheet, name). Same codes as the
/// columns of the monthly timesheet (`CO`, `CM`, `CP`, `CFP`, `CC`, `CIM`).
pub const LEAVE_TYPES: [(&str, &str); 6] = [
    ("CO", "Concediu de odihnă"),
    ("CM", "Concediu medical"),
    ("CP", "Concediu parental"),
    ("CFP", "Concediu fără plată"),
    ("CC", "Concediu maternal"),
    ("CIM", "Contract individual de muncă"),
];

/// Name of a leave code ("CO" → "Concediu de odihnă"); the code itself when it is not known.
pub fn leave_name(code: &str) -> &str {
    LEAVE_TYPES.iter().find(|(c, _)| *c == code).map_or(code, |(_, name)| name)
}

/// What the office typed for the time added to one day (minutes). A side left empty keeps what the rules give it; `0` adds
/// nothing on that side.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FillEdit {
    pub before: Option<i32>,
    pub after: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PauseEdit {
    /// Ids of the trips before and after the pause.
    pub prev: String,
    pub next: String,
    pub state: PauseState,
    /// Waiting time typed by hand, in hours (half hours or minutes allowed).
    pub hours: Option<f64>,
    pub note: String,
    /// Only read from months saved by 0.3.11–0.3.12, which chose a leave for a whole pause: the next recompute turns it
    /// into a leave on each free working day of the pause (see [`Driver::leaves`]) and clears it.
    pub leave: Option<String>,
}

/// What the rules make of the time between two regular trips, before any decision of the office.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PauseKind {
    /// Staționare: he waits at Otopeni (Bucharest side) for less than the long limit: counted as waiting.
    #[default]
    Normal,
    /// Pauză: he is at home (Constanța and the county), or the break is longer than the long limit, or an excursion lies
    /// inside it: counts nothing. The office can still mark it as a staționare.
    #[serde(alias = "confirm")]
    Rest,
    /// The next trip starts before the previous one ends.
    Overlap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pause {
    pub prev: String,
    pub next: String,
    /// Where he arrived / where the next trip starts.
    pub place_from: String,
    pub place_to: String,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    /// The day its minutes count on: the day the trip before it counts on when it is a staționare of the same shift (see
    /// [`Effective::work_day`]), else the day it starts.
    pub day: Option<NaiveDate>,
    /// The trip before the pause went to Bucharest/Otopeni and the one after it comes back from there (and no excursion lies
    /// between them): the way back always counts on the day of the way there, whatever the wait between them was. Not when
    /// the office moved one of them (a drive made by the app) to another day: it stays where it was put.
    pub round_trip: bool,
    pub gap_min: i32,
    /// Minutes of the pause spent on other trips (private, charter): they count as trip hours, not as waiting.
    pub busy_min: i32,
    pub kind: PauseKind,
    pub state: PauseState,
    pub hours_override: Option<f64>,
    pub note: String,
    /// Minutes that count as waiting (in the hours and in the amount).
    pub counted_min: i32,
    /// Added to the estimate for the waiting beyond the included time.
    pub extra_pay: f64,
    pub alerts: Vec<Alert>,
}

/// One day of one driver.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DayRow {
    pub date: Option<NaiveDate>,
    pub trips: i32,
    /// Regular legs made that day (what the 240 lei per pair is made of).
    pub legs: i32,
    pub trip_min: i32,
    pub station_min: i32,
    /// Time added to bring the day up to the hours of a full day: before the first departure and after the last arrival
    /// (see [`FillEdit`]). Part of `total_min`.
    pub fill_before_min: i32,
    pub fill_after_min: i32,
    pub fill_min: i32,
    /// The office typed one of the two values.
    pub fill_manual: bool,
    /// Trips + waiting + the time added.
    pub total_min: i32,
    pub pay_legs: f64,
    pub pay_exterior: f64,
    pub pay_station: f64,
    pub pay_total: f64,
    /// Taken out of the pontaj by the office (it stays on screen, but counts nowhere).
    pub excluded: bool,
    /// The day is a leave the office chose (a day without any trip): the code of the kind of leave.
    pub leave: Option<String>,
    pub alerts: Vec<Alert>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Totals {
    pub days: i32,
    pub legs: i32,
    pub trip_min: i32,
    pub station_min: i32,
    /// Time added to the days to bring them up to a full day (part of `total_min`).
    pub fill_min: i32,
    pub total_min: i32,
    pub pay_legs: f64,
    pub pay_exterior: f64,
    pub pay_station: f64,
    pub pay_total: f64,
    /// Minutes above the monthly ceiling (0 when within it).
    pub over_min: i32,
    /// Days of leave in the month, per code of leave.
    pub leave_days: BTreeMap<String, i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProposalKind {
    /// Shorten the counted waiting of one pause.
    Pause,
    /// Take a whole day out of the pontaj.
    Day,
}

/// A way to bring a driver under the monthly ceiling. Only a suggestion: the office applies it or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub kind: ProposalKind,
    pub date: NaiveDate,
    /// For a pause: the trips around it, and the waiting hours it would keep.
    pub prev: Option<String>,
    pub next: Option<String>,
    pub set_hours: Option<f64>,
    /// Minutes taken out and the amount that goes with them.
    pub minutes: i32,
    pub pay: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Driver {
    pub id: i64,
    pub name: String,
    /// Sorted by (effective) departure.
    pub trips: Vec<Trip>,
    pub pause_edits: Vec<PauseEdit>,
    pub excluded_days: Vec<NaiveDate>,
    /// Days the office marked as leave: date → code of the kind of leave ([`LEAVE_TYPES`]). Only a day without any trip
    /// ([`Driver::free_days`]) counts as leave; if a trip shows up on it later, the leave stays here but counts nowhere.
    pub leaves: BTreeMap<NaiveDate, String>,
    /// The time the office typed for the days brought up to a full day: date → [`FillEdit`].
    pub fills: BTreeMap<NaiveDate, FillEdit>,
    /// The driver's trips just before and just after the month (from the days around it that the download asks for):
    /// not part of this sheet, they only tell where he was when the month began and where he went when it ended, so the
    /// trips between months can be explained (see [`super::transfers`]). Replaced by every download.
    pub before: Vec<Trip>,
    pub after: Vec<Trip>,
    // --- derived ---
    /// The days of the month on which the driver has no trip at all (weekends and holidays included): the days that can
    /// be marked as leave.
    pub free_days: Vec<NaiveDate>,
    pub pauses: Vec<Pause>,
    pub days: Vec<DayRow>,
    pub totals: Totals,
    pub alerts: Vec<Alert>,
    pub proposals: Vec<Proposal>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sheet {
    pub year: i32,
    pub month: u32,
    /// When the trips were last downloaded.
    pub fetched_at: Option<String>,
    pub days: Vec<DayInfo>,
    /// Mon–Fri days of the month, holidays included.
    pub weekdays: u32,
    /// Monthly ceiling: 8 hours × `weekdays`, in minutes.
    pub cap_min: i32,
    pub drivers: Vec<Driver>,
    /// Drivers the office took out of the pontaj (see [`super::hidden`]): what the sheet knew about them, kept apart so that
    /// adding one back loses nothing. They are in no total and in no file.
    pub removed_drivers: Vec<Driver>,
    /// Remarks from the last download (trips that changed or disappeared in the API).
    pub notices: Vec<String>,
}
