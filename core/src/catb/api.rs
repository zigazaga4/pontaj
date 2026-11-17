//! Reads the drivers' trips from the ALT API (`GET {base}/external/driver-trips`).
//!
//! One element of the answer is one driver on one car in one trip. The key goes in the `X-Api-Key` header; at most
//! 30 requests a minute and 93 days per request are allowed, so a month is a single request.

use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use serde::Deserialize;

use super::config::CatbSettings;
use super::model::TripKind;
use crate::calendar::days_in_month;
use crate::{Error, Result};

/// A row of the API, flattened.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRow {
    pub driver_id: i64,
    pub driver_name: String,
    pub trip_id: String,
    pub kind: TripKind,
    pub label: String,
    pub from: String,
    pub to: String,
    pub departure: NaiveDateTime,
    pub arrival: Option<NaiveDateTime>,
    pub return_at: Option<NaiveDateTime>,
    pub car: String,
    pub passengers: i32,
}

/// The rows of an answer, and how many could not be read.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub rows: Vec<ApiRow>,
    pub skipped: usize,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    data: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    driver_id: i64,
    #[serde(default)]
    driver_name: String,
    #[serde(default)]
    trip: RawTrip,
    #[serde(default)]
    route: RawRoute,
    #[serde(default)]
    departure: String,
    duration_minutes: Option<i32>,
    arrival: Option<String>,
    return_at: Option<String>,
    #[serde(default)]
    car: RawCar,
    #[serde(default)]
    passengers: i32,
}

#[derive(Deserialize, Default)]
struct RawTrip {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
}

#[derive(Deserialize, Default)]
struct RawRoute {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
}

#[derive(Deserialize, Default)]
struct RawCar {
    #[serde(default)]
    tag: String,
}

/// `2026-10-04 00:30` (also with seconds, or with a `T`).
pub fn parse_time(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"].iter().find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
}

impl Raw {
    fn into_row(self) -> Option<ApiRow> {
        let departure = parse_time(&self.departure)?;
        if self.trip.id.is_empty() {
            return None;
        }
        let arrival = self.arrival.as_deref().and_then(parse_time).or_else(|| self.duration_minutes.map(|m| departure + chrono::Duration::minutes(m as i64)));
        Some(ApiRow {
            driver_id: self.driver_id,
            driver_name: self.driver_name.trim().to_string(),
            trip_id: self.trip.id,
            kind: TripKind::from_api(&self.trip.kind),
            label: self.trip.label,
            from: self.route.from.trim().to_string(),
            to: self.route.to.trim().to_string(),
            departure,
            arrival,
            return_at: self.return_at.as_deref().and_then(parse_time),
            car: self.car.tag,
            passengers: self.passengers,
        })
    }
}

/// Reads the JSON answer of the API.
pub fn parse_rows(body: &str) -> Result<Parsed> {
    let response: Response = serde_json::from_str(body).map_err(|e| Error::new(format!("Răspunsul API-ului nu poate fi citit: {e}")))?;
    let total = response.data.len();
    let rows: Vec<ApiRow> = response.data.into_iter().filter_map(Raw::into_row).collect();
    Ok(Parsed { skipped: total - rows.len(), rows })
}

/// The reason the API gave for refusing a request (`{"message": "..."}`).
fn refusal(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_default()
}

fn get(url: &str, key: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into();
    let mut response = agent
        .get(url)
        .header("X-Api-Key", key.trim())
        .header("Accept", "application/json")
        .call()
        .map_err(|e| Error::new(format!("Nu pot ajunge la API (verifică internetul): {e}")))?;
    let status = response.status().as_u16();
    let retry = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::new(format!("Răspunsul API-ului s-a întrerupt: {e}")))?;
    match status {
        200 => Ok(body),
        401 => Err(Error::new("Cheia API este greșită sau lipsește. Verific-o în Reguli → Categoria B.")),
        404 => Err(Error::new("Accesul la API nu este activat pe server. Anunță administratorul aplicației ALT.")),
        422 => Err(Error::new(format!("API-ul a refuzat cererea: {}", refusal(&body)))),
        429 => Err(Error::new(format!("Prea multe cereri către API (maxim 30 pe minut). Încearcă din nou peste {} secunde.", retry.as_deref().unwrap_or("60")))),
        503 => Err(Error::new("API-ul are o problemă de configurare pe server. Anunță administratorul aplicației ALT.")),
        other => Err(Error::new(format!("API-ul a răspuns cu eroarea {other}."))),
    }
}

/// Downloads all the trips of a month with one request.
pub fn fetch_month(cfg: &CatbSettings, year: i32, month: u32) -> Result<Parsed> {
    if cfg.api_key.trim().is_empty() {
        return Err(Error::new("Lipsește cheia API. Trec-o în Reguli → Categoria B."));
    }
    let first = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| Error::new("Luna aleasă nu este validă."))?;
    let last = first.with_day(days_in_month(year, month)).unwrap_or(first);
    let url = format!("{}/external/driver-trips?date_from={first}&date_to={last}", cfg.api_base.trim().trim_end_matches('/'));
    parse_rows(&get(&url, &cfg.api_key)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of the API documentation, plus a private trip without duration and a broken row.
    const SAMPLE: &str = r#"{
      "date_from": "2026-10-04", "date_to": "2026-10-04", "driver_id": null, "count": 3,
      "data": [
        { "driver_id": 462, "driver_name": "Popescu Ion",
          "trip": { "type": "eco", "transport_id": 117599, "id": "eco-117599", "label": "ECO 04.10.2026 00:30 Constanta - Aeroport Otopeni Bucuresti" },
          "route": { "from": "Constanta", "to": "Aeroport Otopeni Bucuresti" },
          "departure": "2026-10-04 00:30", "duration_minutes": 210, "arrival": "2026-10-04 04:00", "return_at": null,
          "car": { "id": 90, "tag": "CT-22-XDD", "model": "TOURISMO", "seats": 55, "category_id": 4, "category": "Autocar" }, "passengers": 40 },
        { "driver_id": 462, "driver_name": "Popescu Ion",
          "trip": { "type": "prv", "transport_id": 80211, "id": "private-80211", "label": "PRV" },
          "route": { "from": "Constanta", "to": "Chisinau" },
          "departure": "2026-10-04 09:00", "duration_minutes": null, "arrival": null, "return_at": "2026-10-06 18:00",
          "car": { "id": 7, "tag": "CT-10-ABC" }, "passengers": 3 },
        { "driver_id": 1, "driver_name": "Fara Data", "trip": { "type": "eco", "id": "eco-1" }, "departure": "ieri" }
      ] }"#;

    #[test]
    fn reads_the_documented_answer() {
        let parsed = parse_rows(SAMPLE).unwrap();
        assert_eq!((parsed.rows.len(), parsed.skipped), (2, 1), "the row with an unreadable date is counted, not guessed");
        let eco = &parsed.rows[0];
        assert_eq!((eco.driver_id, eco.trip_id.as_str(), eco.kind), (462, "eco-117599", TripKind::Eco));
        assert_eq!(eco.arrival, parse_time("2026-10-04 04:00"));
        assert_eq!(eco.car, "CT-22-XDD");
        let private = &parsed.rows[1];
        assert_eq!((private.kind, private.arrival), (TripKind::Private, None));
        assert_eq!(private.return_at, parse_time("2026-10-06 18:00"));
    }

    #[test]
    fn arrival_comes_from_the_duration_when_it_is_missing() {
        let body = r#"{"data":[{"driver_id":5,"driver_name":"X","trip":{"type":"eco","id":"eco-9"},"route":{"from":"A","to":"B"},"departure":"2026-10-04 23:30","duration_minutes":180}]}"#;
        let row = &parse_rows(body).unwrap().rows[0];
        assert_eq!(row.arrival, parse_time("2026-10-05 02:30"));
    }

    #[test]
    fn a_missing_key_is_reported_before_any_request() {
        let error = fetch_month(&CatbSettings::default(), 2026, 10).unwrap_err().to_string();
        assert!(error.contains("cheia API"), "{error}");
    }

    #[test]
    fn refusals_show_the_message_of_the_api() {
        assert_eq!(refusal(r#"{"message":"Intervalul depășește 93 de zile","errors":{}}"#), "Intervalul depășește 93 de zile");
        assert_eq!(refusal("nu e json"), "");
    }
}
