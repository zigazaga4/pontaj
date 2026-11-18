//! Trips the API does not have: how a driver got from where one regular trip ended to where the next one starts.
//!
//! The regular trips go Constanța ↔ Otopeni, so a driver's trips follow each other: he arrives somewhere and the next
//! trip leaves from there. When it does not, and the Otopeni/Bucharest end is involved, he made a drive nobody logged: an
//! empty car to Otopeni, or the way back from it with the car or as a passenger of another one. That drive counts as a
//! regular trip, so the app adds it (a [`TripKind::Transfer`]); the office can change its times and places or take it out.
//!
//! - He arrived in Constanța (or anywhere else) and the next trip leaves from Otopeni: the drive *ends* when that trip
//!   leaves and lasts [`CatbSettings::transfer_min`] before it, so the time that is left stays where he was, in Constanța.
//! - He arrived at Otopeni and the next trip leaves from Constanța or its county: the drive *starts* when he arrived (he
//!   does not wait at Otopeni) and lasts as long, but never past the next departure. A next trip that starts far away
//!   (Tulcea → Otopeni) is not a return: nothing is added, and the pause keeps its "loc diferit" remark.
//!
//! Nothing is added when the next regular trip is not known, when the office decided something about the time between
//! the two trips, or when there is no room (the trips overlap). Across the turn of the month the trips just outside it
//! ([`Driver::before`], [`Driver::after`]) take part, and a drive belongs to the month in which it starts: two sheets
//! never count the same one.

use chrono::{Datelike, Duration, NaiveDateTime};

use super::compute::effective;
use super::config::CatbSettings;
use super::model::*;
use super::places;
use crate::settings::Settings;

/// Where one regular trip of the driver starts and ends.
struct Leg {
    id: String,
    from: String,
    to: String,
    departure: NaiveDateTime,
    arrival: Option<NaiveDateTime>,
}

fn before_id(next: &str) -> String {
    format!("gen-pre-{next}")
}

fn after_id(prev: &str) -> String {
    format!("gen-ret-{prev}")
}

/// The regular trips around the month in order: those just before it, those of the sheet (as they count now, the ones the
/// office took out left out), those just after it.
fn sequence(driver: &Driver, cfg: &CatbSettings) -> Vec<Leg> {
    let outside = driver.before.iter().chain(&driver.after).filter_map(|trip| {
        let eff = effective(trip, cfg);
        if !eff.regular || trip.edit.removed {
            return None;
        }
        Some(Leg { id: trip.id.clone(), from: eff.from, to: eff.to, departure: eff.departure?, arrival: eff.arrival })
    });
    let inside = driver
        .trips
        .iter()
        .filter(|t| !t.generated && !t.edit.removed && t.eff.regular)
        .filter_map(|t| Some(Leg { id: t.id.clone(), from: t.eff.from.clone(), to: t.eff.to.clone(), departure: t.eff.departure?, arrival: t.eff.arrival }));
    let mut legs: Vec<Leg> = outside.chain(inside).collect();
    legs.sort_by(|a, b| (a.departure, &a.id).cmp(&(b.departure, &b.id)));
    legs
}

/// Has the office decided what the time between these two trips was (staționare or pauză, or the hours)?
fn decided(driver: &Driver, prev: &str, next: &str) -> bool {
    driver.pause_edits.iter().any(|e| e.prev == prev && e.next == next && (e.state != PauseState::Auto || e.hours.is_some()))
}

/// The drives the rules ask for in this month, with the times and places the rules give them.
fn wanted(driver: &Driver, year: i32, month: u32, cfg: &CatbSettings) -> Vec<Trip> {
    let length = Duration::minutes(i64::from(cfg.transfer_min.max(1)));
    let legs = sequence(driver, cfg);
    let mut out = Vec::new();
    for pair in legs.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let Some(arrived) = a.arrival else { continue };
        if a.to.trim().is_empty() || b.from.trim().is_empty() || places::same_area(&a.to, &b.from, &cfg.extra_local_places) || decided(driver, &a.id, &b.id) {
            continue;
        }
        let (id, start, end) = if places::is_bucharest(&b.from) {
            (before_id(&b.id), (b.departure - length).max(arrived), b.departure)
        } else if places::is_bucharest(&a.to) && places::area(&b.from, &cfg.extra_local_places) == places::Area::Constanta {
            // "Came back": only toward Constanța and its county. A trip that starts far away (Tulcea → Otopeni) is not a return.
            (after_id(&a.id), arrived, (arrived + length).min(b.departure))
        } else {
            continue;
        };
        if end <= start || (start.year(), start.month()) != (year, month) {
            continue;
        }
        let label = format!("deplasare adăugată de aplicație: {} → {}", a.to, b.from);
        out.push(Trip { id, kind: TripKind::Transfer, label, from: a.to.clone(), to: b.from.clone(), departure: start, arrival: Some(end), generated: true, ..Default::default() });
    }
    out
}

/// Brings the generated trips of the driver in line with his real ones: adds the missing, drops the ones no longer needed,
/// refreshes the times and places the rules give. What the office changed on a generated trip stays (`edit`), and a
/// generated trip the office took out stays taken out. Call after the real trips have their `eff`.
pub(super) fn sync(driver: &mut Driver, year: i32, month: u32, settings: &Settings) {
    let cfg = &settings.catb;
    let wanted = if cfg.generate_transfers { wanted(driver, year, month, cfg) } else { Vec::new() };
    driver.trips.retain(|t| !t.generated || wanted.iter().any(|w| w.id == t.id));
    for fresh in wanted {
        match driver.trips.iter_mut().find(|t| t.generated && t.id == fresh.id) {
            Some(trip) => (trip.from, trip.to, trip.departure, trip.arrival, trip.label) = (fresh.from, fresh.to, fresh.departure, fresh.arrival, fresh.label),
            None => driver.trips.push(fresh),
        }
    }
    for trip in driver.trips.iter_mut().filter(|t| t.generated) {
        trip.eff = effective(trip, cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::super::compute::recompute_driver;
    use super::*;

    const OTOPENI: &str = "Aeroport Otopeni Bucuresti";

    fn trip(id: &str, from: &str, to: &str, departure: &str, arrival: &str) -> Trip {
        Trip { id: id.into(), kind: TripKind::Eco, from: from.into(), to: to.into(), departure: parse_time(departure).unwrap(), arrival: parse_time(arrival), ..Default::default() }
    }

    fn driver(trips: Vec<Trip>) -> Driver {
        Driver { id: 1, name: "BADEA LUCIAN".into(), trips, ..Default::default() }
    }

    fn run(driver: &mut Driver, month: u32) {
        recompute_driver(driver, 2026, month, &Settings::default());
    }

    fn generated(driver: &Driver) -> Vec<&Trip> {
        driver.trips.iter().filter(|t| t.generated).collect()
    }

    fn times(trip: &Trip) -> (String, String) {
        let at = |t: Option<NaiveDateTime>| t.map_or("-".into(), |t| t.format("%d.%m %H:%M").to_string());
        (at(trip.eff.departure), at(trip.eff.arrival))
    }

    /// The case that started it all: the last trip of October ends in Constanța, the first of November leaves from Otopeni.
    fn november() -> Driver {
        let mut d = driver(vec![trip("eco-2", OTOPENI, "Constanta", "2026-11-01 05:30", "2026-11-01 08:30")]);
        d.before = vec![trip("eco-1", OTOPENI, "Constanta", "2026-10-31 11:00", "2026-10-31 14:00")];
        d
    }

    #[test]
    fn the_empty_drive_to_otopeni_is_added_before_a_trip_that_leaves_from_otopeni() {
        let mut d = november();
        run(&mut d, 11);
        let made = generated(&d);
        assert_eq!(made.len(), 1);
        let drive = made[0];
        assert_eq!((drive.id.as_str(), drive.kind, drive.eff.from.as_str(), drive.eff.to.as_str()), ("gen-pre-eco-2", TripKind::Transfer, "Constanta", OTOPENI));
        assert_eq!(times(drive), ("01.11 02:30".into(), "01.11 05:30".into()), "3 hours, ending when the trip leaves Otopeni");
        assert!(drive.eff.regular && drive.eff.counts_hours);
        assert_eq!((drive.eff.minutes, drive.eff.price), (180, 120.0));
        // It is a trip of the day: two regular legs, six hours, 240 lei; the time in Constanța before it is a pauză.
        let day = &d.days[0];
        assert_eq!((day.legs, day.trip_min, day.station_min, day.pay_total), (2, 360, 0, 240.0));
        assert!(day.alerts.iter().any(|a| a.code == "transfer" && a.level == Level::Info), "{:?}", day.alerts);
        assert!(d.trips.iter().position(|t| t.id == drive.id) < d.trips.iter().position(|t| t.id == "eco-2"), "sorted by departure");
    }

    #[test]
    fn the_way_back_is_added_after_a_trip_that_ended_at_otopeni_when_the_next_one_leaves_from_elsewhere() {
        // October 31: Constanța → Otopeni, arrives 23:00; November 1 the next trip leaves from Constanța.
        let mut october = driver(vec![trip("eco-1", "Constanta", OTOPENI, "2026-10-31 20:00", "2026-10-31 23:00")]);
        october.after = vec![trip("eco-2", "Constanta", OTOPENI, "2026-11-01 08:00", "2026-11-01 11:00")];
        run(&mut october, 10);
        let made = generated(&october);
        assert_eq!(made.len(), 1);
        assert_eq!((made[0].id.as_str(), made[0].eff.from.as_str(), made[0].eff.to.as_str()), ("gen-ret-eco-1", OTOPENI, "Constanta"));
        assert_eq!(times(made[0]), ("31.10 23:00".into(), "01.11 02:00".into()), "leaves as soon as he arrived, 3 hours");
        assert_eq!(october.days[0].legs, 2);
        assert_eq!(october.pauses.iter().map(|p| (p.kind, p.counted_min)).collect::<Vec<_>>(), [(PauseKind::Normal, 0)], "no waiting at Otopeni: he leaves as soon as he arrives");

        // November's sheet sees the same pair from the other side and adds nothing: the drive started in October.
        let mut november = driver(vec![trip("eco-2", "Constanta", OTOPENI, "2026-11-01 08:00", "2026-11-01 11:00")]);
        november.before = vec![trip("eco-1", "Constanta", OTOPENI, "2026-10-31 20:00", "2026-10-31 23:00")];
        run(&mut november, 11);
        assert!(generated(&november).is_empty(), "{:?}", generated(&november));
    }

    #[test]
    fn nothing_is_made_up_when_the_next_trip_is_not_known_or_there_is_no_history() {
        let mut d = driver(vec![trip("eco-1", "Constanta", OTOPENI, "2026-10-31 20:00", "2026-10-31 23:00")]);
        run(&mut d, 10);
        assert!(generated(&d).is_empty(), "he may still be at Otopeni: no way back is invented");
        let mut d = driver(vec![trip("eco-2", OTOPENI, "Constanta", "2026-11-01 05:30", "2026-11-01 08:30")]);
        run(&mut d, 11);
        assert!(generated(&d).is_empty(), "nothing is known about where he was before the first trip");
    }

    #[test]
    fn it_works_inside_the_month_and_never_past_the_next_departure_or_before_the_arrival() {
        // Arrives at Otopeni 08:00, the next trip leaves from Constanța at 10:00: only 2 hours of room.
        let mut d = driver(vec![trip("eco-1", "Constanta", OTOPENI, "2026-10-05 05:00", "2026-10-05 08:00"), trip("eco-2", "Constanta", OTOPENI, "2026-10-05 10:00", "2026-10-05 13:00")]);
        run(&mut d, 10);
        assert_eq!(times(generated(&d)[0]), ("05.10 08:00".into(), "05.10 10:00".into()));
        // Arrives in Constanța 14:00, leaves from Otopeni 16:00: the empty drive cannot start before he arrived.
        let mut d = driver(vec![trip("eco-1", OTOPENI, "Constanta", "2026-10-05 11:00", "2026-10-05 14:00"), trip("eco-2", OTOPENI, "Constanta", "2026-10-05 16:00", "2026-10-05 19:00")]);
        run(&mut d, 10);
        assert_eq!(times(generated(&d)[0]), ("05.10 14:00".into(), "05.10 16:00".into()));
        // Trips that overlap leave no room.
        let mut d = driver(vec![trip("eco-1", OTOPENI, "Constanta", "2026-10-05 11:00", "2026-10-05 14:00"), trip("eco-2", OTOPENI, "Constanta", "2026-10-05 13:00", "2026-10-05 16:00")]);
        run(&mut d, 10);
        assert!(generated(&d).is_empty());
    }

    #[test]
    fn a_trip_that_starts_far_away_is_not_a_return_but_the_drive_from_far_away_to_otopeni_is_added() {
        // Arrives at Otopeni, the next trip leaves from Tulcea: not a return to Constanța, nothing is added.
        let mut d = driver(vec![trip("eco-1", "Constanta", OTOPENI, "2026-10-05 05:00", "2026-10-05 08:00"), trip("eco-2", "Tulcea", OTOPENI, "2026-10-05 15:00", "2026-10-05 18:00")]);
        run(&mut d, 10);
        assert!(generated(&d).is_empty());
        assert!(d.pauses[0].alerts.iter().any(|a| a.code == "place_differs"), "the pause keeps its remark: {:?}", d.pauses[0].alerts);
        // Arrives in Tulcea, the next trip leaves from Otopeni: he drove the empty car from there.
        let mut d = driver(vec![trip("eco-1", OTOPENI, "Tulcea", "2026-10-05 05:00", "2026-10-05 08:00"), trip("eco-2", OTOPENI, "Constanta", "2026-10-05 15:00", "2026-10-05 18:00")]);
        run(&mut d, 10);
        let made = generated(&d);
        assert_eq!((made.len(), made[0].eff.from.as_str(), made[0].id.as_str()), (1, "Tulcea", "gen-pre-eco-2"));
        assert_eq!(times(made[0]), ("05.10 12:00".into(), "05.10 15:00".into()));
    }

    #[test]
    fn trips_that_follow_each_other_need_nothing() {
        let mut d = driver(vec![
            trip("eco-1", "Constanta", OTOPENI, "2026-10-05 05:00", "2026-10-05 08:00"),
            trip("eco-2", OTOPENI, "Constanta", "2026-10-05 10:00", "2026-10-05 13:00"),
            trip("eco-3", "Navodari", OTOPENI, "2026-10-06 05:00", "2026-10-06 08:00"),
        ]);
        run(&mut d, 10);
        assert!(generated(&d).is_empty(), "arriving in Constanța and leaving from Năvodari is the same side");
    }

    #[test]
    fn excursions_and_other_trips_in_between_do_not_break_the_chain() {
        let mut private = trip("private-2", "Constanta", "Kalambaka", "2026-10-06 06:00", "2026-10-09 18:00");
        private.kind = TripKind::Private;
        let mut d = driver(vec![
            trip("eco-1", OTOPENI, "Constanta", "2026-10-05 11:00", "2026-10-05 14:00"),
            private,
            trip("eco-3", "Constanta", OTOPENI, "2026-10-10 05:00", "2026-10-10 08:00"),
        ]);
        run(&mut d, 10);
        assert!(d.trips.iter().any(|t| t.id == "private-2" && t.eff.excursion));
        assert!(generated(&d).is_empty(), "{:?}", generated(&d));
    }

    #[test]
    fn what_the_office_changes_on_a_generated_trip_stays_and_a_removed_one_stays_removed() {
        let mut d = november();
        run(&mut d, 11);
        let id = "gen-pre-eco-2";
        let edit = |d: &mut Driver| d.trips.iter_mut().find(|t| t.id == id).unwrap().edit.clone();
        d.trips.iter_mut().find(|t| t.id == id).unwrap().edit.departure = parse_time("2026-11-01 03:30");
        d.trips.iter_mut().find(|t| t.id == id).unwrap().edit.from = Some("Mamaia".into());
        run(&mut d, 11);
        assert_eq!(times(generated(&d)[0]), ("01.11 03:30".into(), "01.11 05:30".into()));
        assert_eq!(d.days[0].trip_min, 120 + 180, "the edited length counts");

        // The API moves the real trip: the drive follows it, the office's own times stay.
        d.trips.iter_mut().find(|t| t.id == "eco-2").unwrap().departure = parse_time("2026-11-01 07:00").unwrap();
        d.trips.iter_mut().find(|t| t.id == "eco-2").unwrap().arrival = parse_time("2026-11-01 10:00");
        run(&mut d, 11);
        assert_eq!(edit(&mut d).departure, parse_time("2026-11-01 03:30"));
        assert_eq!(generated(&d)[0].eff.arrival, parse_time("2026-11-01 07:00"));

        // Taken out: it counts nowhere, and it does not come back on the next recompute.
        d.trips.iter_mut().find(|t| t.id == id).unwrap().edit.removed = true;
        run(&mut d, 11);
        run(&mut d, 11);
        assert_eq!(generated(&d).len(), 1);
        assert_eq!((d.days[0].legs, d.days[0].trip_min), (1, 180));
    }

    #[test]
    fn a_drive_that_is_no_longer_needed_disappears() {
        let mut d = november();
        run(&mut d, 11);
        assert_eq!(generated(&d).len(), 1);
        // The real trip now leaves from Constanța: nothing to explain.
        d.trips.iter_mut().find(|t| t.id == "eco-2").unwrap().edit.from = Some("Constanta".into());
        run(&mut d, 11);
        assert!(generated(&d).is_empty());
        // Idempotent: a recompute of a driver without changes changes nothing.
        let mut e = november();
        run(&mut e, 11);
        let once = e.clone();
        run(&mut e, 11);
        assert_eq!(e, once);
    }

    #[test]
    fn a_decision_of_the_office_about_the_time_between_the_trips_is_respected() {
        let mut d = november();
        d.pause_edits.push(PauseEdit { prev: "eco-1".into(), next: "eco-2".into(), state: PauseState::Stayed, ..Default::default() });
        run(&mut d, 11);
        assert!(generated(&d).is_empty());
        // A note alone decides nothing.
        d.pause_edits[0].state = PauseState::Auto;
        d.pause_edits[0].note = "foaia".into();
        run(&mut d, 11);
        assert_eq!(generated(&d).len(), 1);
    }

    #[test]
    fn the_setting_turns_it_off_and_sets_the_length() {
        let mut off = Settings::default();
        off.catb.generate_transfers = false;
        let mut d = november();
        recompute_driver(&mut d, 2026, 11, &off);
        assert!(generated(&d).is_empty());

        let mut two_hours = Settings::default();
        two_hours.catb.transfer_min = 120;
        let mut d = november();
        recompute_driver(&mut d, 2026, 11, &two_hours);
        assert_eq!(times(generated(&d)[0]), ("01.11 03:30".into(), "01.11 05:30".into()));
    }
}
