//! What the screen does when the office changes something, step by step as `ui/js/catb.js` and `ui/js/catb_driver.js` do
//! it: the driver travels to Rust as JSON, is changed, recomputed alone (the command `catb_recalculate_driver`) and put back
//! in the sheet. The buttons that Rust answers itself (move a drive, resolve an overlap) call the same functions as their
//! commands. Every function returns whether the change was made (a refused move is not an error: the screen says why).

use chrono::{Duration, NaiveDate, NaiveDateTime};
use pontaj_core::catb::api::Parsed;
use pontaj_core::catb::{self, Driver, FillEdit, HiddenDriver, PauseEdit, PauseKind, PauseState, Sheet, Trip};
use pontaj_core::settings::Settings;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// The way a value goes from Rust to the screen and back.
pub fn through_json<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_str(&serde_json::to_string(value).expect("to JSON")).expect("from JSON")
}

fn position(sheet: &Sheet, id: i64) -> Option<usize> {
    sheet.drivers.iter().position(|d| d.id == id)
}

/// The screen's `edit()`: change the driver, recompute him alone, put him back.
pub fn edit(sheet: &mut Sheet, id: i64, settings: &Settings, change: impl FnOnce(&mut Driver) -> bool) -> bool {
    let Some(at) = position(sheet, id) else { return false };
    let mut driver = through_json(&sheet.drivers[at]);
    if !change(&mut driver) {
        return false;
    }
    catb::recompute_driver(&mut driver, sheet.year, sheet.month, settings);
    sheet.drivers[at] = through_json(&driver);
    true
}

fn trip_mut<'a>(driver: &'a mut Driver, trip: &str) -> Option<&'a mut Trip> {
    driver.trips.iter_mut().find(|t| t.id == trip)
}

fn midnight(at: NaiveDateTime) -> NaiveDateTime {
    at.date().and_hms_opt(0, 0, 0).unwrap()
}

/// A departure or an arrival typed as a time of day (`timeInput`): the date stays, an arrival not after the departure is
/// the next day, and typing the API's value again undoes the edit.
pub fn type_time(sheet: &mut Sheet, id: i64, trip: &str, arrival: bool, minute_of_day: i32, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(t) = trip_mut(d, trip) else { return false };
        let departure = t.edit.departure.unwrap_or(t.departure);
        let mut value = midnight(departure) + Duration::minutes(i64::from(minute_of_day.rem_euclid(1440)));
        if arrival && value <= departure {
            value += Duration::days(1);
        }
        if arrival {
            t.edit.arrival = (Some(value) != t.arrival).then_some(value);
        } else {
            t.edit.departure = (value != t.departure).then_some(value);
        }
        true
    })
}

/// The box emptied: back to what the API says.
pub fn clear_time(sheet: &mut Sheet, id: i64, trip: &str, arrival: bool, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(t) = trip_mut(d, trip) else { return false };
        if arrival {
            t.edit.arrival = None;
        } else {
            t.edit.departure = None;
        }
        true
    })
}

/// ✕: a trip added by hand is deleted, any other is taken out of the pontaj.
pub fn remove_trip(sheet: &mut Sheet, id: i64, trip: &str, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(at) = d.trips.iter().position(|t| t.id == trip) else { return false };
        if d.trips[at].manual {
            d.trips.remove(at);
        } else {
            d.trips[at].edit.removed = true;
        }
        true
    })
}

/// ↺: the trip comes back.
pub fn restore_trip(sheet: &mut Sheet, id: i64, trip: &str, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(t) = trip_mut(d, trip) else { return false };
        t.edit.removed = false;
        true
    })
}

/// "Adaugă cursă": the trip the dialog builds (`askNewTrip`), as JSON like the screen sends it.
#[allow(clippy::too_many_arguments)]
pub fn add_trip(sheet: &mut Sheet, id: i64, trip_id: &str, from: &str, to: &str, departure: NaiveDateTime, minutes: i64, settings: &Settings) -> bool {
    let fmt = |t: NaiveDateTime| t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let trip: Trip = serde_json::from_value(serde_json::json!({
        "id": trip_id, "kind": "eco", "label": "adăugată manual", "from": from, "to": to,
        "departure": fmt(departure), "arrival": fmt(departure + Duration::minutes(minutes)), "returnAt": null,
        "car": "", "passengers": 0, "manual": true,
        "edit": { "departure": null, "arrival": null, "from": null, "to": null, "price": null, "removed": false },
    }))
    .expect("the trip of the dialog");
    edit(sheet, id, settings, |d| {
        if d.trips.iter().any(|t| t.id == trip_id) {
            return false;
        }
        d.trips.push(trip);
        true
    })
}

/// What the office decided about a pause (`setPause`): an edit that says nothing is dropped.
pub fn decide_pause(sheet: &mut Sheet, id: i64, prev: &str, next: &str, state: PauseState, hours: Option<f64>, note: &str, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let at = match d.pause_edits.iter().position(|e| e.prev == prev && e.next == next) {
            Some(at) => at,
            None => {
                d.pause_edits.push(PauseEdit { prev: prev.into(), next: next.into(), ..PauseEdit::default() });
                d.pause_edits.len() - 1
            }
        };
        let e = &mut d.pause_edits[at];
        e.state = state;
        e.hours = hours;
        e.note = note.into();
        if e.state == PauseState::Auto && e.hours.is_none() && e.note.is_empty() {
            d.pause_edits.remove(at);
        }
        true
    })
}

/// Time added before the first departure / after the last arrival, typed in minutes (None = the rules decide).
pub fn type_fill(sheet: &mut Sheet, id: i64, date: NaiveDate, before: bool, minutes: Option<i32>, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let e = d.fills.entry(date).or_insert_with(FillEdit::default);
        if before {
            e.before = minutes;
        } else {
            e.after = minutes;
        }
        if e.before.is_none() && e.after.is_none() {
            d.fills.remove(&date);
        }
        true
    })
}

/// The hours of a day fixed by hand, as "Reglează după Excel" leaves them (None = the day goes back to the rules).
pub fn set_hours(sheet: &mut Sheet, id: i64, date: NaiveDate, minutes: Option<i32>, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        match minutes {
            Some(m) => {
                d.set_hours.insert(date, m);
            }
            None => {
                d.set_hours.remove(&date);
            }
        }
        true
    })
}

/// "Scoate ziua din pontaj" / "Pune ziua la loc".
pub fn toggle_day(sheet: &mut Sheet, id: i64, date: NaiveDate, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        match d.excluded_days.iter().position(|x| *x == date) {
            Some(at) => {
                d.excluded_days.remove(at);
            }
            None => d.excluded_days.push(date),
        }
        true
    })
}

/// The leave select of a free day ("" = an ordinary day off).
pub fn set_leave(sheet: &mut Sheet, id: i64, date: NaiveDate, code: Option<&str>, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        match code {
            Some(c) => {
                d.leaves.insert(date, c.into());
            }
            None => {
                d.leaves.remove(&date);
            }
        }
        true
    })
}

pub fn set_price(sheet: &mut Sheet, id: i64, trip: &str, price: Option<f64>, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(t) = trip_mut(d, trip) else { return false };
        t.edit.price = price;
        true
    })
}

/// A place typed over the API's ("" = back to the API's).
pub fn set_place(sheet: &mut Sheet, id: i64, trip: &str, destination: bool, text: &str, settings: &Settings) -> bool {
    edit(sheet, id, settings, |d| {
        let Some(t) = trip_mut(d, trip) else { return false };
        let (field, original) = if destination { (&mut t.edit.to, t.to.clone()) } else { (&mut t.edit.from, t.from.clone()) };
        *field = (!text.trim().is_empty() && text.trim() != original).then(|| text.trim().to_string());
        true
    })
}

/// The ◀ / ▶ buttons of a drive made by the app (command `catb_move_trip`).
pub fn move_drive(sheet: &mut Sheet, id: i64, trip: &str, days: i32, settings: &Settings) -> bool {
    let Some(at) = position(sheet, id) else { return false };
    match catb::move_transfer(through_json(&sheet.drivers[at]), trip, days, sheet.year, sheet.month, settings) {
        Ok(driver) => {
            sheet.drivers[at] = through_json(&driver);
            true
        }
        Err(_) => false,
    }
}

/// "Rezolvă suprapunerea" (command `catb_resolve_overlap`).
pub fn resolve_overlap(sheet: &mut Sheet, id: i64, prev: &str, next: &str, settings: &Settings) -> bool {
    let Some(at) = position(sheet, id) else { return false };
    match catb::resolve_overlap(through_json(&sheet.drivers[at]), prev, next, sheet.year, sheet.month, settings) {
        Ok(done) => {
            sheet.drivers[at] = through_json(&done.driver);
            true
        }
        Err(_) => false,
    }
}

/// Every overlap of the sheet resolved, the way the office does it (one press per overlap). Returns the presses.
pub fn resolve_all_overlaps(sheet: &mut Sheet, settings: &Settings) -> usize {
    let mut pressed = 0;
    let ids: Vec<i64> = sheet.drivers.iter().map(|d| d.id).collect();
    for id in ids {
        for _ in 0..80 {
            let Some(at) = position(sheet, id) else { break };
            let Some(p) = sheet.drivers[at].pauses.iter().find(|p| p.kind == PauseKind::Overlap) else { break };
            let (prev, next) = (p.prev.clone(), p.next.clone());
            if !resolve_overlap(sheet, id, &prev, &next, settings) {
                break;
            }
            pressed += 1;
        }
    }
    pressed
}

/// × next to a driver: out of the pontaj in every month (the list lives in the settings; the sheet is recomputed).
pub fn hide_driver(sheet: &mut Sheet, id: i64, settings: &mut Settings) -> bool {
    let Some(at) = position(sheet, id) else { return false };
    let name = sheet.drivers[at].name.clone();
    settings.catb.hidden_drivers.push(HiddenDriver { id, name });
    *sheet = through_json(sheet);
    catb::recompute(sheet, settings);
    true
}

/// "Adaugă înapoi".
pub fn show_driver(sheet: &mut Sheet, id: i64, settings: &mut Settings) -> bool {
    let before = settings.catb.hidden_drivers.len();
    settings.catb.hidden_drivers.retain(|d| d.id != id);
    catb::recompute(sheet, settings);
    before != settings.catb.hidden_drivers.len()
}

/// The settings saved in "Reguli": the whole month is recomputed (`catb_recalculate`).
pub fn change_settings(sheet: &mut Sheet, settings: &mut Settings, new: Settings) {
    *settings = new;
    *sheet = through_json(sheet);
    catb::recompute(sheet, settings);
}

/// The refresh from the API (silent or by the button): the answer merged into the sheet as it is (`catb_merge`).
pub fn refresh(sheet: &mut Sheet, answer: &Parsed, settings: &Settings) {
    *sheet = catb::merge_download(Some(through_json(sheet)), sheet.year, sheet.month, answer, settings);
}

/// The app closed and opened again: the month saved as JSON, read back and made ready (`catb_open`).
pub fn reopen(sheet: &mut Sheet, settings: &Settings) {
    let mut saved: Sheet = through_json(sheet);
    catb::open_saved(&mut saved, None, None, settings);
    *sheet = saved;
}

// ---------------------------------------------------------------------------------------------------------------------
// What there is to change
// ---------------------------------------------------------------------------------------------------------------------

/// The regular trips of a driver the office would touch (from the API, not removed, with a departure).
pub fn regular_trips(driver: &Driver) -> Vec<(String, NaiveDateTime)> {
    driver
        .trips
        .iter()
        .filter(|t| !t.generated && !t.manual && !t.edit.removed && t.eff.regular)
        .filter_map(|t| t.eff.departure.map(|at| (t.id.clone(), at)))
        .collect()
}

/// The drivers with the most trips (ids), the ones a test edits.
pub fn busy(sheet: &Sheet, count: usize) -> Vec<i64> {
    let mut list: Vec<(usize, i64)> = sheet.drivers.iter().map(|d| (regular_trips(d).len(), d.id)).filter(|(n, _)| *n >= 4).collect();
    list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    list.into_iter().take(count).map(|(_, id)| id).collect()
}

pub fn driver(sheet: &Sheet, id: i64) -> Option<&Driver> {
    sheet.drivers.iter().find(|d| d.id == id)
}

/// Days of the month with work (dates).
pub fn worked_days(driver: &Driver) -> Vec<NaiveDate> {
    driver.days.iter().filter(|r| r.total_min > 0 && !r.excluded).filter_map(|r| r.date).collect()
}
