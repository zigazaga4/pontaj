//! Months made up for the tests, as the API would send them: every kind of day the real months have (a pair of trips with
//! waiting at the airport, a late trip past midnight, three or four trips, a far trip with no arrival, trips the API makes
//! overlap, a category D van, days off), with the days around the month, for any number of drivers.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use pontaj_core::calendar::days_in_month;
use pontaj_core::catb::api::{ApiRow, Parsed};
use pontaj_core::catb::{self, Sheet, TripKind};
use pontaj_core::settings::Settings;

use super::rng::Rng;

pub const OTOPENI: &str = "Aeroport Otopeni Bucuresti";
pub const CONSTANTA: &str = "Constanta";
const HOTEL: &str = "Hotel Hilton Garden Inn Bucuresti";
const LOCAL: [&str; 4] = ["Mamaia", "Navodari", "Eforie Nord", "Medgidia"];
const FAR: [&str; 3] = ["Tulcea", "Brasov", "Iasi"];

/// Made-up names (with the letters that sort differently: Ș, Ț, Î, Ă).
const NAMES: [&str; 14] = [
    "Ștefănescu Proba", "Țestescu Unu", "Alfa Doi", "Beta Trei", "Îndrumare Patru", "Gama Cinci", "Delta Șase",
    "Epsilon Șapte", "Zeta Opt", "Teta Nouă", "Ăstălalt Zece", "Sigma Unsprezece", "Omega Doisprezece", "Kapa Treisprezece",
];

struct Maker<'a> {
    rng: &'a mut Rng,
    rows: Vec<ApiRow>,
    next_id: u32,
}

impl Maker<'_> {
    #[allow(clippy::too_many_arguments)]
    fn trip(&mut self, driver: (i64, &str), kind: TripKind, from: &str, to: &str, departure: NaiveDateTime, minutes: Option<i64>, seats: i32) {
        self.next_id += 1;
        let prefix = match kind {
            TripKind::Eco => "eco",
            TripKind::Private => "private",
            TripKind::Vip => "vip",
            _ => "bus",
        };
        let van = seats > 8;
        self.rows.push(ApiRow {
            driver_id: driver.0,
            driver_name: driver.1.to_string(),
            trip_id: format!("{prefix}-{}", 50_000 + self.next_id),
            kind,
            label: prefix.into(),
            from: from.into(),
            to: to.into(),
            departure,
            arrival: minutes.map(|m| departure + Duration::minutes(m)),
            return_at: None,
            car: if van { "CT-51-DXD".into() } else { format!("CT-{:02}-XDD", 10 + self.rng.below(80)) },
            car_model: if van { "SPRINTER".into() } else { "VITO".into() },
            car_seats: seats,
            car_category: String::new(),
            passengers: self.rng.range(1, 7),
        });
    }

    /// A time of the day on a 5-minute step between two hours.
    fn at(&mut self, day: NaiveDate, from_h: i32, to_h: i32) -> NaiveDateTime {
        day.and_hms_opt(0, 0, 0).unwrap() + Duration::minutes(i64::from(self.rng.range(from_h * 12, to_h * 12) * 5))
    }

    fn day(&mut self, driver: (i64, &str), day: NaiveDate) {
        let small = 8;
        match self.rng.below(100) {
            0..=24 => {} // a day off
            25..=57 => {
                // The usual pair: to the airport early, waiting there, back to Constanța.
                let go = self.at(day, 3, 8);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, go, Some(180), small);
                let back = go + Duration::minutes(180 + i64::from(self.rng.range(6, 110) * 5));
                self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, back, Some(180), small);
            }
            58..=64 => {
                // The other way round, coming back late (past midnight sometimes).
                let go = self.at(day, 8, 14);
                self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, go, Some(180), small);
                let back = self.at(day, 19, 23);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, back, Some(180), small);
            }
            65..=72 => {
                // Three or four trips: the pair, a hotel run, a local trip.
                let go = self.at(day, 3, 6);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, go, Some(180), small);
                let hotel = go + Duration::minutes(180 + i64::from(self.rng.range(2, 12) * 5));
                self.trip(driver, TripKind::Private, OTOPENI, HOTEL, hotel, Some(60), small);
                let back = hotel + Duration::minutes(60 + i64::from(self.rng.range(6, 60) * 5));
                self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, back, Some(180), small);
                if self.rng.chance(50) {
                    let local = back + Duration::minutes(180 + i64::from(self.rng.range(6, 40) * 5));
                    let place = *self.rng.pick(&LOCAL);
                    self.trip(driver, TripKind::Private, CONSTANTA, place, local, Some(40), small);
                }
            }
            92..=95 => {
                // To the airport in the afternoon, a night of waiting there, back before dawn: one shift over midnight.
                let go = self.at(day, 13, 18);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, go, Some(180), small);
                let back = go + Duration::minutes(180 + i64::from(self.rng.range(96, 138) * 5));
                self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, back, Some(180), small);
            }
            73..=78 => {
                // One late trip that ends the next day.
                let go = self.at(day, 21, 23);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, go, Some(180), small);
            }
            79..=83 => {
                // A far trip (no arrival sometimes), between regular ones or alone.
                let go = self.at(day, 5, 9);
                let place = *self.rng.pick(&FAR);
                let minutes = if self.rng.chance(50) { None } else { Some(i64::from(self.rng.range(36, 72) * 5)) };
                self.trip(driver, TripKind::Private, CONSTANTA, place, go, minutes, small);
                if self.rng.chance(60) {
                    let later = self.at(day, 15, 20);
                    self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, later, Some(180), small);
                }
            }
            84..=88 => {
                // The API's estimated arrivals make two trips overlap.
                let go = self.at(day, 4, 10);
                self.trip(driver, TripKind::Eco, CONSTANTA, OTOPENI, go, Some(180), small);
                let next = go + Duration::minutes(i64::from(self.rng.range(20, 34) * 5));
                self.trip(driver, TripKind::Vip, OTOPENI, CONSTANTA, next, Some(180), small);
            }
            89..=91 => {
                // A category D van in the morning, a small car later.
                let go = self.at(day, 4, 8);
                self.trip(driver, TripKind::Bus, CONSTANTA, OTOPENI, go, Some(180), 20);
                let back = go + Duration::minutes(180 + i64::from(self.rng.range(12, 60) * 5));
                self.trip(driver, TripKind::Eco, OTOPENI, CONSTANTA, back, Some(180), small);
            }
            _ => {
                // A single trip at any hour, any kind.
                let go = self.at(day, 0, 22);
                let kind = *self.rng.pick(&[TripKind::Eco, TripKind::Private, TripKind::Vip]);
                let (from, to) = if self.rng.chance(50) { (CONSTANTA, OTOPENI) } else { (OTOPENI, CONSTANTA) };
                self.trip(driver, kind, from, to, go, Some(180), small);
            }
        }
    }
}

/// The API's answer for a month (with 4 days around it) of `drivers` drivers.
pub fn api_rows(seed: u64, year: i32, month: u32, drivers: usize) -> Vec<ApiRow> {
    let mut rng = Rng::new(seed);
    let mut maker = Maker { rng: &mut rng, rows: Vec::new(), next_id: seed as u32 * 1000 };
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap() - Duration::days(4);
    let last = NaiveDate::from_ymd_opt(year, month, days_in_month(year, month)).unwrap() + Duration::days(4);
    for (i, name) in NAMES.iter().cycle().take(drivers).enumerate() {
        let driver = (900 + i as i64, *name);
        let mut day = first;
        while day <= last {
            maker.day(driver, day);
            day += Duration::days(1);
        }
    }
    maker.rows
}

pub fn parsed(rows: Vec<ApiRow>) -> Parsed {
    Parsed { rows, skipped: 0, without_neighbors: false }
}

/// The month as the app has it after the first download.
pub fn downloaded(seed: u64, year: i32, month: u32, drivers: usize, settings: &Settings) -> (Sheet, Parsed) {
    let answer = parsed(api_rows(seed, year, month, drivers));
    let sheet = catb::apply_download(None, year, month, &answer, "01.01.2026 08:00", settings);
    (sheet, answer)
}
