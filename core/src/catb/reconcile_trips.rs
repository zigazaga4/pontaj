//! "Reglare după Excel", the trips: the departure, the arrival, the places and the "taken out" mark of each trip of the file's
//! sheet "Detaliu" are brought into the app as the office's changes of the trips ([`TripEdit`]), the same ones the office makes
//! by hand, so the hours of the days follow from the rules and the trips look in the app as they look in the file.
//!
//! The lines of the file and the trips of the app are matched **in the order of the clock**, not by day: a trip of the night
//! (21:00 → 01:00) is written on the day it started in the file but can sit on the next day in the app, and the other way
//! round. Two trips are the same when they are of the same kind (Eco, Privată... or Deplasare) and leave within
//! [`MAX_SHIFT_MIN`] of each other, and the matching keeps the order and takes as many pairs as it can, the nearest first.

use chrono::{NaiveDate, NaiveDateTime};

use super::compute::recompute_driver;
use super::excel::{clock, REMOVED_SUFFIX};
use super::model::*;
use super::reconcile::{Skipped, TripChange};
use crate::pontaj_file::{Detail, DetailLine};
use crate::settings::Settings;

/// The farthest apart (in minutes) the departures of a line of the file and of a trip of the app can be, to be the same trip: the
/// office moves a time by hours, not by a day.
const MAX_SHIFT_MIN: i64 = 12 * 60;

/// The kinds a line of "Detaliu" can have to be a trip (the other lines are waiting, time added, adjustments, leave).
const TRIP_KINDS: [TripKind; 6] = [TripKind::Eco, TripKind::Private, TripKind::Vip, TripKind::Bus, TripKind::Passenger, TripKind::Transfer];

/// A trip of the file with its kind, whether it is taken out, and the moment it leaves.
struct FileTrip<'a> {
    line: &'a DetailLine,
    kind: TripKind,
    removed: bool,
    start: NaiveDateTime,
}

/// The trips of the file, in the order of the clock (a line with no departure cannot be matched and is left out).
fn file_trips(detail: &Detail) -> Vec<FileTrip<'_>> {
    let mut trips: Vec<FileTrip> = detail
        .lines
        .iter()
        .filter_map(|line| {
            let (kind, removed) = line.kind.strip_suffix(REMOVED_SUFFIX).map_or((line.kind.trim(), false), |kind| (kind.trim(), true));
            let kind = TRIP_KINDS.into_iter().find(|known| known.label() == kind)?;
            Some(FileTrip { line, kind, removed, start: line.start? })
        })
        .collect();
    trips.sort_by_key(|trip| trip.start);
    trips
}

/// Pairs the trips of the file with the trips of the app that are the same trip: in order, as many pairs as possible, and among
/// the ways to get that many the one with the least time between the pairs. Returns `(index in file, index in app)`.
fn align(file: &[&FileTrip], app: &[&Trip]) -> Vec<(usize, usize)> {
    // The minutes between the departures when the two can be the same trip.
    let gap = |i: usize, j: usize| -> Option<i64> {
        let (f, a) = (file[i], app[j]);
        let minutes = (f.start - a.eff.departure?).num_minutes().abs();
        (f.kind == a.kind && minutes <= MAX_SHIFT_MIN).then_some(minutes)
    };
    // (pairs, minutes between them): more pairs first, then less time.
    let better = |a: (usize, i64), b: (usize, i64)| a.0 > b.0 || (a.0 == b.0 && a.1 < b.1);
    let (n, m) = (file.len(), app.len());
    let mut best = vec![vec![(0usize, 0i64); m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            let mut now = best[i - 1][j];
            if better(best[i][j - 1], now) {
                now = best[i][j - 1];
            }
            if let Some(gap) = gap(i - 1, j - 1) {
                let (pairs, minutes) = best[i - 1][j - 1];
                if better((pairs + 1, minutes + gap), now) {
                    now = (pairs + 1, minutes + gap);
                }
            }
            best[i][j] = now;
        }
    }
    let (mut i, mut j, mut pairs) = (n, m, Vec::new());
    while i > 0 && j > 0 {
        let paired = gap(i - 1, j - 1).is_some_and(|gap| best[i][j] == (best[i - 1][j - 1].0 + 1, best[i - 1][j - 1].1 + gap));
        if paired {
            pairs.push((i - 1, j - 1));
            (i, j) = (i - 1, j - 1);
        } else if best[i][j] == best[i - 1][j] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// "21:30–01:30 (+1) · Constanta → Aeroport Otopeni Bucuresti": a trip as the office reads it.
fn describe(departure: NaiveDateTime, arrival: Option<NaiveDateTime>, from: &str, to: &str, removed: bool) -> String {
    let day = departure.date();
    let mut text = format!("{}–{}", clock(Some(departure), day), arrival.map_or_else(|| "?".to_string(), |at| clock(Some(at), day)));
    if !from.is_empty() || !to.is_empty() {
        text = format!("{text} · {from} → {to}");
    }
    if removed {
        text.push_str(" (scoasă)");
    }
    text
}

fn describe_trip(trip: &Trip) -> String {
    let eff = &trip.eff;
    describe(eff.departure.unwrap_or(trip.departure), eff.arrival, &eff.from, &eff.to, trip.edit.removed)
}

fn describe_line(trip: &FileTrip) -> String {
    describe(trip.start, trip.line.end, &trip.line.from, &trip.line.to, trip.removed)
}

/// What a trip is, as far as the file can change it.
type Shape = (Option<NaiveDateTime>, Option<NaiveDateTime>, String, String, bool);

fn shape_of(trip: &Trip) -> Shape {
    (trip.eff.departure, trip.eff.arrival, trip.eff.from.clone(), trip.eff.to.clone(), trip.edit.removed)
}

/// What the app held about a trip before the file was followed.
struct Before {
    id: String,
    text: String,
    shape: Shape,
}

/// Gives a trip the times, the places and the mark of the file's line as the office's changes. Only what differs from what the trip
/// is without them is kept as a change, so a file that says what the app already says leaves no change behind. A time the file does
/// not have (an arrival not known) leaves the trip's own.
fn follow_line(trip: &mut Trip, line: &FileTrip) {
    trip.edit.departure = (line.start != trip.departure).then_some(line.start);
    if let Some(end) = line.line.end {
        trip.edit.arrival = (Some(end) != trip.arrival).then_some(end);
    }
    if !line.line.from.is_empty() {
        trip.edit.from = (line.line.from != trip.from.trim()).then(|| line.line.from.clone());
    }
    if !line.line.to.is_empty() {
        trip.edit.to = (line.line.to != trip.to.trim()).then(|| line.line.to.clone());
    }
    trip.edit.removed = line.removed;
}

/// How one pass matched the file with the app: the lines that found a trip (index of the line, id of the trip), the lines that did
/// not, and the trips that did not.
#[derive(Default)]
struct Pass {
    found: Vec<(usize, String)>,
    alone_file: Vec<usize>,
    alone_app: Vec<String>,
}

/// Matches the lines of the file that are drives of the app (or the others, when `generated` is false) with the trips of the same
/// sort, and follows them.
fn follow_pass(driver: &mut Driver, trips: &[FileTrip], generated: bool) -> Pass {
    let indexes: Vec<usize> = (0..trips.len()).filter(|i| (trips[*i].kind == TripKind::Transfer) == generated).collect();
    let file: Vec<&FileTrip> = indexes.iter().map(|i| &trips[*i]).collect();
    let app: Vec<&Trip> = driver.trips.iter().filter(|trip| trip.generated == generated && trip.eff.departure.is_some()).collect();
    let pairs = align(&file, &app);
    let pass = Pass {
        found: pairs.iter().map(|(i, j)| (indexes[*i], app[*j].id.clone())).collect(),
        alone_file: (0..file.len()).filter(|i| !pairs.iter().any(|(taken, _)| taken == i)).map(|i| indexes[i]).collect(),
        alone_app: (0..app.len()).filter(|j| !pairs.iter().any(|(_, taken)| taken == j)).map(|j| app[j].id.clone()).collect(),
    };
    for (i, id) in &pass.found {
        if let Some(trip) = driver.trips.iter_mut().find(|trip| trip.id == *id) {
            follow_line(trip, &trips[*i]);
        }
    }
    pass
}

/// What following the trips of the file did, for the report.
#[derive(Debug, Default)]
pub(super) struct Followed {
    pub changes: Vec<TripChange>,
    pub skipped: Vec<Skipped>,
}

/// Brings the trips of `driver` to what the sheet "Detaliu" says about them.
///
/// First the trips of the API (and the ones added by hand), then the drives the app makes by itself: those depend on the trips
/// around them, so they are rebuilt once the first group has its new times, and only then do the file's drives find theirs.
pub(super) fn follow_trips(driver: &mut Driver, detail: &Detail, year: i32, month: u32, settings: &Settings) -> Followed {
    let name = driver.name.clone();
    let skipped = |date: NaiveDate, excel: String, reason: String| Skipped { driver: name.clone(), date: Some(date), excel, reason };
    recompute_driver(driver, year, month, settings);
    let trips = file_trips(detail);
    // What every trip was before anything changed (the drives the app makes are rebuilt on the way).
    let was: Vec<Before> = driver.trips.iter().map(|trip| Before { id: trip.id.clone(), text: describe_trip(trip), shape: shape_of(trip) }).collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut alone_file: Vec<usize> = Vec::new();
    let mut alone_app: Vec<String> = Vec::new();
    for generated in [false, true] {
        let pass = follow_pass(driver, &trips, generated);
        (found.extend(pass.found), alone_file.extend(pass.alone_file), alone_app.extend(pass.alone_app));
        recompute_driver(driver, year, month, settings);
    }

    let mut followed = Followed::default();
    for i in alone_file {
        let trip = &trips[i];
        let reason = if trip.kind == TripKind::Transfer {
            "aplicația nu face deplasarea asta (deplasările se fac singure, din cursele dinaintea și de după ele)"
        } else {
            "în aplicație nu este o cursă de același fel care să plece la o oră apropiată"
        };
        followed.skipped.push(skipped(trip.line.date, describe_line(trip), reason.into()));
    }
    for id in alone_app {
        let Some(trip) = driver.trips.iter().find(|trip| trip.id == id) else { continue };
        if let Some(day) = trip.eff.work_day {
            followed.skipped.push(skipped(day, "—".into(), format!("cursa din aplicație ({}) nu este în Excel; rămâne cum e", describe_trip(trip))));
        }
    }
    for (i, id) in found {
        let (line, Some(trip)) = (&trips[i], driver.trips.iter().find(|trip| trip.id == id)) else { continue };
        // A drive that did not exist before (the new times of the trips around it made the app generate it) is a change too.
        let before = was.iter().find(|before| before.id == id);
        if before.is_none_or(|before| shape_of(trip) != before.shape) {
            followed.changes.push(TripChange { driver: name.clone(), date: Some(line.line.date), kind: line.kind.label().to_string(), app: before.map_or_else(String::new, |before| before.text.clone()), excel: describe_trip(trip) });
        }
        // The day a trip counts on comes from the rules (the places and the clock), not from a field: when it still lands on another
        // day than the file's, the hours of the days are brought to the file's afterwards, and the office is told.
        if let Some(day) = trip.eff.work_day.filter(|day| *day != line.line.date) {
            let reason = format!(
                "în Excel cursa e trecută pe {}, în aplicație rămâne pe {} (ziua o stabilesc regulile, după locuri și ore); orele zilelor se aduc la Excel",
                line.line.date.format("%d.%m"),
                day.format("%d.%m")
            );
            followed.skipped.push(skipped(line.line.date, describe_line(line), reason));
        }
    }
    followed.changes.sort_by_key(|change| change.date);
    followed
}
