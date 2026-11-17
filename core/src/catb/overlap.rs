//! The "Rezolvă suprapunerea" button: moves the meeting point of two consecutive regular trips that overlap.
//!
//! The departure times are what the API really knows; the arrival is only the departure plus the fixed duration of the
//! route (an estimate). So the earlier trip is cut where the later one really starts. When that would leave it with
//! too little of its duration (the two trips run side by side), the later trip starts a little later instead, so that
//! each keeps at least `overlap_keep_pct` percent of its duration; if even then the later one would be squeezed below
//! that, the time between the earlier departure and the later arrival is shared in proportion to their durations.
//!
//! Only these two trips change, and only through their edits: the times of the API stay as they are, the outer ends of
//! the pair (earlier departure, later arrival) never move, and typing the API's time back undoes the fix.

use chrono::{Duration, NaiveDateTime};
use serde::Serialize;

use super::compute::recompute_driver;
use super::model::*;
use crate::settings::Settings;
use crate::{Error, Result};

/// The driver after the fix and a sentence telling what was moved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub driver: Driver,
    pub summary: String,
}

/// The moment the earlier trip (`a_start..a_end`) ends and the later one (`b_start..b_end`) starts, never before the
/// later trip's own departure and never after the earlier trip's estimated arrival.
fn meeting_point(a_start: NaiveDateTime, a_end: NaiveDateTime, b_start: NaiveDateTime, b_end: Option<NaiveDateTime>, keep_pct: i32) -> NaiveDateTime {
    let pct = i64::from(keep_pct.clamp(0, 100));
    let minutes = |from: NaiveDateTime, to: NaiveDateTime| (to - from).num_minutes().max(0);
    let keep_a = minutes(a_start, a_end) * pct / 100;
    let earliest = a_start + Duration::minutes(keep_a);
    // The earlier trip is simply cut where the later one starts.
    if b_start >= earliest {
        return b_start;
    }
    // The later trip has to start later than it does, as late as the earlier one needs to keep its share.
    let point = match b_end {
        Some(end) => {
            let keep_b = minutes(b_start, end) * pct / 100;
            if minutes(earliest, end) >= keep_b {
                earliest
            } else {
                // Neither can keep its share: split the whole span in proportion to the two durations.
                let (dur_a, dur_b) = (minutes(a_start, a_end), minutes(b_start, end));
                if dur_a + dur_b == 0 {
                    b_start
                } else {
                    a_start + Duration::minutes(minutes(a_start, end) * dur_a / (dur_a + dur_b))
                }
            }
        }
        None => earliest,
    };
    point.clamp(b_start, a_end)
}

fn clock(time: NaiveDateTime) -> String {
    time.format("%H:%M").to_string()
}

fn trip_name(trip: &Trip) -> String {
    format!("{} {} → {}", trip.kind.label(), trip.eff.from, trip.eff.to)
}

/// Fixes the overlap between the trips `prev` and `next` (the two ends of an `Overlap` pause) and recomputes the driver.
pub fn resolve_overlap(mut driver: Driver, prev: &str, next: &str, year: i32, month: u32, settings: &Settings) -> Result<Resolved> {
    recompute_driver(&mut driver, year, month, settings);
    let overlapping = driver.pauses.iter().any(|p| p.prev == prev && p.next == next && p.kind == PauseKind::Overlap);
    let position = |id: &str| driver.trips.iter().position(|t| t.id == id);
    let (Some(a), Some(b)) = (position(prev), position(next)) else { return Err(Error::new("Nu găsesc cursele suprapuse.")) };
    if !overlapping {
        return Err(Error::new("Cursele nu se mai suprapun (poate le-ai corectat deja)."));
    }
    let (first, second) = (&driver.trips[a].eff, &driver.trips[b].eff);
    let (Some(a_start), Some(a_end), Some(b_start)) = (first.departure, first.arrival, second.departure) else { return Err(Error::new("Cursele nu au orele necesare.")) };
    let point = meeting_point(a_start, a_end, b_start, second.arrival, settings.catb.overlap_keep_pct);

    let mut moved = Vec::new();
    {
        let trip = &mut driver.trips[a];
        trip.edit.arrival = (Some(point) != trip.arrival).then_some(point);
        moved.push(format!("{}: sosirea {} → {}", trip_name(trip), clock(a_end), clock(point)));
    }
    if point > b_start {
        let trip = &mut driver.trips[b];
        trip.edit.departure = (point != trip.departure).then_some(point);
        moved.push(format!("{}: plecarea {} → {}", trip_name(trip), clock(b_start), clock(point)));
    }
    recompute_driver(&mut driver, year, month, settings);
    Ok(Resolved { driver, summary: format!("{}.", moved.join("; ")) })
}

#[cfg(test)]
mod tests {
    use super::super::api::parse_time;
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        parse_time(text).unwrap()
    }

    fn trip(id: &str, kind: TripKind, from: &str, to: &str, departure: &str, arrival: Option<&str>) -> Trip {
        Trip { id: id.into(), kind, from: from.into(), to: to.into(), departure: at(departure), arrival: arrival.map(at), ..Default::default() }
    }

    fn driver(trips: Vec<Trip>) -> Driver {
        Driver { id: 1, name: "TEST".into(), trips, ..Default::default() }
    }

    fn fix(driver: Driver, prev: &str, next: &str) -> Result<Resolved> {
        resolve_overlap(driver, prev, next, 2026, 11, &Settings::default())
    }

    fn times(driver: &Driver, id: &str) -> (String, String) {
        let trip = driver.trips.iter().find(|t| t.id == id).unwrap();
        (clock(trip.eff.departure.unwrap()), trip.eff.arrival.map_or("-".into(), clock))
    }

    #[test]
    fn a_small_overlap_cuts_the_earlier_trip_where_the_later_one_starts() {
        // Eco 06:30 → 09:30 (an estimate), the private trip really leaves Otopeni at 08:40.
        let d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 06:30", Some("2026-11-04 09:30")),
            trip("private-2", TripKind::Private, "Aeroport Otopeni Bucuresti", "Cernavoda", "2026-11-04 08:40", Some("2026-11-04 11:00")),
        ]);
        let done = fix(d, "eco-1", "private-2").unwrap();
        assert_eq!(times(&done.driver, "eco-1"), ("06:30".into(), "08:40".into()));
        assert_eq!(times(&done.driver, "private-2"), ("08:40".into(), "11:00".into()), "the later trip does not move");
        assert_eq!(done.summary, "Eco Constanta → Aeroport Otopeni Bucuresti: sosirea 09:30 → 08:40.");
        // Gone: no overlap left, and only the earlier trip carries an edit.
        assert!(done.driver.pauses.iter().all(|p| p.kind != PauseKind::Overlap), "{:?}", done.driver.pauses);
        assert!(done.driver.trips.iter().find(|t| t.id == "private-2").unwrap().edit == TripEdit::default());
        assert_eq!(done.driver.trips[0].edit.arrival, Some(at("2026-11-04 08:40")));
    }

    #[test]
    fn trips_side_by_side_meet_in_the_middle_of_what_the_earlier_one_can_give() {
        // Two ecos 16:30 → 19:30 and 17:30 → 20:30: cutting the first at 17:30 would leave it 1 h of 3 h.
        let d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 16:30", Some("2026-11-04 19:30")),
            trip("eco-2", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 17:30", Some("2026-11-04 20:30")),
        ]);
        let done = fix(d, "eco-1", "eco-2").unwrap();
        // The first keeps half of its 3 h (until 18:00), the second starts then and keeps its arrival (2 h 30 min of 3 h).
        assert_eq!(times(&done.driver, "eco-1"), ("16:30".into(), "18:00".into()));
        assert_eq!(times(&done.driver, "eco-2"), ("18:00".into(), "20:30".into()));
        assert!(done.summary.contains("sosirea 19:30 → 18:00") && done.summary.contains("plecarea 17:30 → 18:00"), "{}", done.summary);
        assert!(done.driver.pauses.iter().all(|p| p.kind != PauseKind::Overlap));
    }

    #[test]
    fn when_neither_trip_can_keep_its_share_the_span_is_split_in_proportion() {
        // B lies inside A: A 08:00 → 12:00 (4 h), B 08:30 → 10:00 (1 h 30 min). The pair spans 08:00 → 10:00.
        let (a0, a1, b0, b1) = (at("2026-11-04 08:00"), at("2026-11-04 12:00"), at("2026-11-04 08:30"), at("2026-11-04 10:00"));
        let point = meeting_point(a0, a1, b0, Some(b1), 50);
        // 120 min × 240 / 330 = 87 min after 08:00.
        assert_eq!(point, at("2026-11-04 09:27"));
        assert!(point >= b0 && point <= a1);
    }

    #[test]
    fn a_later_trip_without_an_arrival_just_starts_later() {
        let (a0, a1, b0) = (at("2026-11-04 06:00"), at("2026-11-04 10:00"), at("2026-11-04 07:00"));
        assert_eq!(meeting_point(a0, a1, b0, None, 50), at("2026-11-04 08:00"), "the earlier one keeps half of 4 h");
        // An overlap that leaves enough to the earlier trip does not move the later one.
        assert_eq!(meeting_point(a0, a1, at("2026-11-04 09:00"), None, 50), at("2026-11-04 09:00"));
    }

    #[test]
    fn only_the_two_trips_change_and_typing_the_api_time_back_undoes_it() {
        let d = driver(vec![
            trip("eco-0", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-04 01:00", Some("2026-11-04 04:00")),
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 06:30", Some("2026-11-04 09:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-04 08:40", Some("2026-11-04 11:40")),
            trip("eco-3", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 12:00", Some("2026-11-04 15:00")),
        ]);
        let done = fix(d, "eco-1", "eco-2").unwrap();
        for id in ["eco-0", "eco-3"] {
            assert!(done.driver.trips.iter().find(|t| t.id == id).unwrap().edit == TripEdit::default(), "{id} was touched");
        }
        // Putting the API's arrival back (what the box does when the same time is typed) gives the overlap again.
        let mut again = done.driver.clone();
        again.trips.iter_mut().find(|t| t.id == "eco-1").unwrap().edit.arrival = None;
        recompute_driver(&mut again, 2026, 11, &Settings::default());
        assert!(again.pauses.iter().any(|p| p.kind == PauseKind::Overlap));
    }

    #[test]
    fn it_refuses_when_there_is_no_overlap_any_more() {
        let d = driver(vec![
            trip("eco-1", TripKind::Eco, "Constanta", "Aeroport Otopeni Bucuresti", "2026-11-04 06:30", Some("2026-11-04 09:30")),
            trip("eco-2", TripKind::Eco, "Aeroport Otopeni Bucuresti", "Constanta", "2026-11-04 10:00", Some("2026-11-04 13:00")),
        ]);
        let err = fix(d, "eco-1", "eco-2").unwrap_err();
        assert!(err.to_string().contains("nu se mai suprapun"), "{err}");
        let err = fix(driver(vec![]), "x", "y").unwrap_err();
        assert!(err.to_string().contains("Nu găsesc"), "{err}");
    }
}
