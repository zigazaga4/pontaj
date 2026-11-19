//! Reads the drivers' trips from the ALT API (`GET {base}/external/driver-trips`).
//!
//! One element of the answer is one driver on one car in one trip. The key goes in the `X-Api-Key` header; at most
//! 30 requests a minute and 93 days per request are allowed, so a month is a single request.

use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use super::config::CatbSettings;
use super::model::TripKind;
use crate::calendar::days_in_month;
use crate::{Error, Result};

/// A row of the API, flattened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// Registration number of the car (`CT-22-XDD`).
    pub car: String,
    /// Make/model as the ALT app writes it (`TOURISMO`, `VITO`), and its passenger seats: they tell a category D vehicle
    /// from a small one. 0 seats = the API did not say.
    pub car_model: String,
    pub car_seats: i32,
    pub car_category: String,
    pub passengers: i32,
}

/// The rows of an answer, and how many could not be read. It goes to the screen and comes back (the download and the
/// merge into the open sheet are two steps, so an edit made while the API answers is never lost).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Parsed {
    /// Besides the month's trips, those of the days around it (see [`DAYS_BEFORE`], [`DAYS_AFTER`]): the caller keeps
    /// only the month's as trips and uses the others to tell where each driver was when the month began and ended.
    pub rows: Vec<ApiRow>,
    pub skipped: usize,
    /// The API refused the wider range, so only the month was asked for: no trips around it are known.
    pub without_neighbors: bool,
}

/// Days before the month and after it that a download asks for as well. A driver's last trip before the month can be
/// weeks old (he was on leave); 31 days covers a whole month away. The longest range the API gives is 93 days.
pub const DAYS_BEFORE: i64 = 31;
pub const DAYS_AFTER: i64 = 14;

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
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    seats: Option<i32>,
    #[serde(default)]
    category: Option<String>,
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
            car_model: self.car.model.unwrap_or_default().trim().to_string(),
            car_seats: self.car.seats.unwrap_or(0).max(0),
            car_category: self.car.category.unwrap_or_default().trim().to_string(),
            passengers: self.passengers,
        })
    }
}

/// Reads the JSON answer of the API.
pub fn parse_rows(body: &str) -> Result<Parsed> {
    let response: Response = serde_json::from_str(body).map_err(|e| Error::new(format!("Răspunsul API-ului nu poate fi citit: {e}")))?;
    let total = response.data.len();
    let rows: Vec<ApiRow> = response.data.into_iter().filter_map(Raw::into_row).collect();
    Ok(Parsed { skipped: total - rows.len(), rows, without_neighbors: false })
}

/// The reason the API gave for refusing a request (`{"message": "..."}`).
fn refusal(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// What the server answered to one request.
struct Reply {
    status: u16,
    retry_after: Option<String>,
    body: String,
}

fn call(url: &str, key: &str) -> Result<Reply> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into();
    let mut response = agent
        .get(url)
        .header("X-Api-Key", key.trim())
        .header("Accept", "application/json")
        .call()
        .map_err(|e| Error::new(format!("Nu pot ajunge la API (verifică internetul): {e}")))?;
    let status = response.status().as_u16();
    let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::new(format!("Răspunsul API-ului s-a întrerupt: {e}")))?;
    Ok(Reply { status, retry_after, body })
}

/// The body of a successful answer, or the explanation of the refusal in plain Romanian.
fn accept(reply: Reply) -> Result<String> {
    match reply.status {
        200 => Ok(reply.body),
        401 => Err(Error::new("Cheia API este greșită sau a fost schimbată. Anunță administratorul aplicației ALT; o cheie nouă se poate trece în Reguli → Categoria B.")),
        404 => Err(Error::new("Accesul la API nu este activat pe server. Anunță administratorul aplicației ALT.")),
        422 => Err(Error::new(format!("API-ul a refuzat cererea: {}", refusal(&reply.body)))),
        429 => Err(Error::new(format!("Prea multe cereri către API (maxim 30 pe minut). Încearcă din nou peste {} secunde.", reply.retry_after.as_deref().unwrap_or("60")))),
        503 => Err(Error::new("API-ul are o problemă de configurare pe server. Anunță administratorul aplicației ALT.")),
        other => Err(Error::new(format!("API-ul a răspuns cu eroarea {other}."))),
    }
}

/// Downloads the trips of a month, and those of the days around it, with one request. If the API refuses the wider
/// range, only the month is asked for (and the answer says so).
pub fn fetch_month(cfg: &CatbSettings, year: i32, month: u32) -> Result<Parsed> {
    fetch_month_with_key(cfg, cfg.effective_key(), year, month)
}

fn fetch_month_with_key(cfg: &CatbSettings, key: &str, year: i32, month: u32) -> Result<Parsed> {
    if key.is_empty() {
        return Err(Error::new("Această versiune a aplicației nu are cheia API încorporată. O poți trece în Reguli → Categoria B."));
    }
    let first = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| Error::new("Luna aleasă nu este validă."))?;
    let last = first.with_day(days_in_month(year, month)).unwrap_or(first);
    let base = cfg.api_base.trim().trim_end_matches('/');
    let url = |from: NaiveDate, to: NaiveDate| format!("{base}/external/driver-trips?date_from={from}&date_to={to}");

    let wide = call(&url(first - chrono::Duration::days(DAYS_BEFORE), last + chrono::Duration::days(DAYS_AFTER)), key)?;
    if wide.status != 422 {
        return parse_rows(&accept(wide)?);
    }
    let mut parsed = parse_rows(&accept(call(&url(first, last), key)?)?)?;
    parsed.without_neighbors = true;
    Ok(parsed)
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
        assert_eq!((eco.car_model.as_str(), eco.car_seats, eco.car_category.as_str()), ("TOURISMO", 55, "Autocar"));
        let private = &parsed.rows[1];
        assert_eq!((private.car_model.as_str(), private.car_seats), ("", 0), "a car the API says little about has no model and 0 seats");
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
        let error = fetch_month_with_key(&CatbSettings::default(), "", 2026, 10).unwrap_err().to_string();
        assert!(error.contains("cheia API"), "{error}");
    }

    #[test]
    fn refusals_show_the_message_of_the_api() {
        assert_eq!(refusal(r#"{"message":"Intervalul depășește 93 de zile","errors":{}}"#), "Intervalul depășește 93 de zile");
        assert_eq!(refusal("nu e json"), "");
    }

    #[test]
    fn answers_are_accepted_or_explained() {
        let reply = |status: u16, body: &str| Reply { status, retry_after: Some("12".into()), body: body.into() };
        assert_eq!(accept(reply(200, "{}")).unwrap(), "{}");
        assert!(accept(reply(422, r#"{"message":"Interval prea lung"}"#)).unwrap_err().to_string().contains("Interval prea lung"));
        assert!(accept(reply(429, "")).unwrap_err().to_string().contains("12 secunde"));
        assert!(accept(reply(500, "")).unwrap_err().to_string().contains("500"));
    }

    #[test]
    fn the_range_asked_for_fits_the_limit_of_the_api() {
        // The longest month plus the days around it must stay under the 93 days the API allows.
        assert!(31 + DAYS_BEFORE + DAYS_AFTER <= 93);
    }
}
