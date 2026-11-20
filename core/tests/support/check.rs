//! The exported workbook compared with the app, cell by cell, with every formula recomputed by the test engine: what the
//! screen shows for a driver and a day is what the file shows once Excel opens it.
//!
//! First sheet (per driver: hours, night, then a start and an end row per trip of the driver's busiest day, at least
//! two pairs): the hours are the day's total rounded the firm's way (blank on a day taken out, the code on a leave day);
//! under them every trip of the day from its departure to its arrival, in the order they leave, the rest blank; the
//! night hours are those of the written times. "Pontaj zilnic" and "Sinteză": the minutes of the app. Every formula:
//! the value cached in the file is what it computes.

use chrono::{Datelike, NaiveDate, NaiveTime};
use pontaj_core::calendar::days_in_month;
use pontaj_core::catb::{Driver, Sheet};
use pontaj_core::settings::Settings;
use pontaj_core::time::round_hours;

use super::formula::Engine;
use super::workbook::{Value, Workbook};

#[derive(Default)]
pub struct Report {
    pub problems: Vec<String>,
    pub drivers: usize,
    pub days: usize,
    pub formulas: usize,
    /// Drivers written with more than the two pairs of times (a day with three trips or more).
    pub extra_rows: usize,
}

impl Report {
    pub fn add(&mut self, other: Report) {
        self.problems.extend(other.problems);
        self.drivers += other.drivers;
        self.days += other.days;
        self.formulas += other.formulas;
        self.extra_rows += other.extra_rows;
    }
}

/// Names compared the way a person reads them: no diacritics, no case, single spaces.
pub fn norm(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'ă' | 'â' | 'Ă' | 'Â' => 'A',
            'î' | 'Î' => 'I',
            'ș' | 'ş' | 'Ș' | 'Ş' => 'S',
            'ț' | 'ţ' | 'Ț' | 'Ţ' => 'T',
            other => other.to_ascii_uppercase(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn hm(m: i32) -> String {
    let sign = if m < 0 { "-" } else { "" };
    format!("{sign}{}:{:02}", m.abs() / 60, m.abs() % 60)
}

fn clock(text: &str) -> i32 {
    let (h, m) = text.split_once(':').unwrap_or((text, "0"));
    h.trim().parse::<i32>().unwrap_or(0) * 60 + m.trim().parse::<i32>().unwrap_or(0)
}

/// The trips of a day as the screen lists them, each from its departure to its arrival (minutes from the midnight that opens
/// the day; what happens after midnight in a shift that began that day is over 1440): those that count on that day (not
/// taken out, not an excursion). A trip counts on the day it leaves, except the way back of a trip to Bucharest/Otopeni and a
/// trip after a staționare going over midnight (the same outing): those count on the day of the trip before them.
pub fn day_trips(driver: &Driver, date: NaiveDate) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for t in &driver.trips {
        if t.edit.removed || t.eff.excursion || !t.eff.counts_hours || t.eff.minutes <= 0 {
            continue;
        }
        let Some(at) = t.eff.departure else { continue };
        if t.eff.work_day.unwrap_or(at.date()) == date {
            let start = (at - date.and_time(NaiveTime::MIN)).num_minutes() as i32;
            out.push((start, start + t.eff.minutes));
        }
    }
    out.sort();
    out
}

/// The times the first sheet writes for a day: the trips as they are, with the time added to the day already in them (the
/// first departure earlier by what was added before it, the trip that ends last later by what was added after it).
pub fn written_trips(driver: &Driver, date: NaiveDate, row: &pontaj_core::catb::DayRow) -> Vec<(i32, i32)> {
    let mut trips = day_trips(driver, date);
    if let Some(first) = trips.first_mut() {
        first.0 -= row.fill_before_min;
    }
    if let Some(last) = trips.iter_mut().max_by_key(|t| t.1) {
        last.1 += row.fill_after_min;
    }
    trips
}

/// Is the place at the Bucharest end of the route (written like the test months write it)?
fn bucharest_side(place: &str) -> bool {
    let place = place.to_lowercase();
    ["otopeni", "bucuresti", "bucharest", "baneasa", "henri coanda", "hilton garden"].iter().any(|k| place.contains(k))
}

/// The rule behind the day a trip counts on, checked apart from the app's own code. A trip counts on the day of the trip
/// before it (the pause between them, as the sheet lists it) when it is the way back of a trip to Bucharest/Otopeni and leaves
/// on that day or the next one (the wait, long or short, whatever the office chose for it, does not matter; an excursion
/// between the two breaks the pair), or when the wait is a staționare that goes over a single midnight. Every other trip
/// counts on the day it leaves.
pub fn work_day_problems(driver: &Driver) -> Vec<String> {
    let mut problems = Vec::new();
    let station = |p: &pontaj_core::catb::Pause| match p.state {
        pontaj_core::catb::PauseState::Stayed => true,
        pontaj_core::catb::PauseState::Off => false,
        pontaj_core::catb::PauseState::Auto => p.kind == pontaj_core::catb::PauseKind::Normal,
    };
    for (index, t) in driver.trips.iter().enumerate() {
        let Some(at) = t.eff.departure else { continue };
        let after = driver.pauses.iter().find(|p| p.next == t.id);
        let before = after.and_then(|p| driver.trips.iter().position(|x| x.id == p.prev).map(|i| (p, i)));
        let joined = before.and_then(|(p, i)| {
            let prev = &driver.trips[i];
            let day = prev.eff.work_day?;
            let excursion_between = driver.trips.get(i + 1..index).is_some_and(|between| between.iter().any(|x| !x.edit.removed && x.eff.excursion));
            // (A drive made by the app that the office moved a day is a way there or back like any other.)
            let way_back = bucharest_side(&prev.eff.to) && !bucharest_side(&prev.eff.from) && bucharest_side(&t.eff.from) && !bucharest_side(&t.eff.to) && !excursion_between && (at.date() - day).num_days() <= 1;
            let over_midnight = station(p) && p.start.zip(p.end).is_some_and(|(s, e)| (e.date() - s.date()).num_days() <= 1);
            (way_back || over_midnight).then_some(day)
        });
        let expected = joined.unwrap_or(at.date());
        if t.eff.work_day != Some(expected) {
            problems.push(format!("{} {}: trece pe {:?}, dar ar trebui pe {} (pleacă pe {})", driver.name, t.id, t.eff.work_day, expected, at.date()));
        }
    }
    problems
}

/// A day of the month whose trips the first sheet writes: one that has hours (not taken out, not a leave, not under
/// the rounding).
fn written_day(row: &pontaj_core::catb::DayRow, from: i32) -> bool {
    !row.excluded && row.leave.is_none() && row.total_min >= from
}

/// Minutes of the intervals inside the night window (the window repeats every day; one that ends past midnight wraps).
fn night_minutes(intervals: &[(i32, i32)], settings: &Settings) -> i32 {
    let (start, mut end) = (clock(&settings.night_start), clock(&settings.night_end));
    if end <= start {
        end += 1440;
    }
    let mut total = 0;
    for &(a, b) in intervals {
        for k in -2..=2 {
            let (ws, we) = (start + k * 1440, end + k * 1440);
            total += (b.min(we) - a.max(ws)).max(0);
        }
    }
    total
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (x, y) if x.is_blank() && y.is_blank() => true,
        (Value::Num(x), Value::Num(y)) => (x - y).abs() < 1e-7,
        (Value::Text(x), Value::Text(y)) => x.trim() == y.trim(),
        (Value::Bool(x), Value::Bool(y)) => x == y,
        _ => false,
    }
}

pub fn check(sheet: &Sheet, settings: &Settings, wb: &Workbook) -> Report {
    let mut report = Report::default();
    let engine = Engine::new(wb);
    let from = settings.round_up_from_min.clamp(1, 60);
    let hours = |m: i32| if m < from { None } else { Some(round_hours(m, from)) };
    let month_days = days_in_month(sheet.year, sheet.month);
    let problems = &mut report.problems;

    // ----- every formula: the value cached in the file is what the formula computes
    for (si, s) in wb.sheets.iter().enumerate() {
        for &(r, c) in s.formulas.keys() {
            report.formulas += 1;
            let computed = engine.value(si, r, c);
            let cached = s.value(r, c);
            if matches!(computed, Value::Error(_)) || !same(&computed, &cached) {
                problems.push(format!("„{}” rând {} col {}: în fișier {}, formula dă {}", s.name, r + 1, c + 1, cached.show(), computed.show()));
            }
        }
    }

    // ----- the first sheet: a block of rows per driver (its name on the first one), then the TOTAL line
    let main = &wb.sheets[0];
    let last_row = main.values.keys().map(|(r, _)| *r).max().unwrap_or(0);
    let mut order: Vec<(&Driver, u32, u32)> = Vec::new();
    let mut r = 10;
    while r <= last_row && main.text(r, 0).map(|t| t.trim().to_string()).as_deref() != Some("TOTAL") {
        let Some(name) = main.text(r, 1) else {
            problems.push(format!("prima foaie, rândul {}: nici șofer, nici TOTAL", r + 1));
            break;
        };
        let Some(driver) = sheet.drivers.iter().find(|d| norm(&d.name) == norm(&name)) else {
            problems.push(format!("prima foaie: „{name}” nu e șofer în aplicație"));
            break;
        };
        // The driver's rows: hours, night and a pair per trip of its busiest day (at least two pairs).
        let busiest = driver
            .days
            .iter()
            .filter(|row| row.date.is_some_and(|d| (d.year(), d.month()) == (sheet.year, sheet.month)) && written_day(row, from))
            .filter_map(|row| row.date.map(|d| day_trips(driver, d).len()))
            .max()
            .unwrap_or(0);
        let rows = 2 + 2 * busiest.max(2) as u32;
        for k in 0..rows - 2 {
            let want = if k % 2 == 0 { "Ora începerii" } else { "Ora terminării" };
            if main.text(r + 2 + k, 2).as_deref().map(str::trim) != Some(want) {
                problems.push(format!("{}: rândul {} trebuia „{want}”", driver.name, r + 3 + k));
            }
        }
        let next = r + rows;
        if !(main.text(next, 1).is_some() || main.text(next, 0).is_some_and(|t| t.trim() == "TOTAL")) || (r + 1..next).any(|x| main.text(x, 1).is_some()) {
            problems.push(format!("{}: blocul de rânduri nu are {rows} rânduri (o pereche de ore pentru fiecare cursă a zilei cu cele mai multe curse: {busiest})", driver.name));
        }
        order.push((driver, r, rows));
        r = next;
    }
    if main.text(r, 0).map(|t| t.trim().to_string()).as_deref() != Some("TOTAL") {
        problems.push("prima foaie: lipsește rândul TOTAL după ultimul șofer".into());
    }
    if order.len() != sheet.drivers.len() {
        problems.push(format!("prima foaie are {} șoferi, aplicația {}", order.len(), sheet.drivers.len()));
    }
    let names: Vec<String> = order.iter().map(|(d, _, _)| norm(&d.name)).collect();
    let mut sorted = names.clone();
    sorted.sort();
    if names != sorted {
        problems.push("prima foaie: șoferii nu sunt în ordine alfabetică".into());
    }

    for &(driver, r, rows) in &order {
        report.drivers += 1;
        problems.extend(work_day_problems(driver));
        let pairs = (rows - 2) / 2;
        if pairs > 2 {
            report.extra_rows += 1;
        }
        let (mut sum_hours, mut sum_night) = (0, 0);
        for row in &driver.days {
            let Some(date) = row.date else { continue };
            if (date.year(), date.month()) != (sheet.year, sheet.month) {
                continue;
            }
            report.days += 1;
            let col = 4 + date.day() - 1;
            let label = format!("{} {}", driver.name, date);
            let app = format!(
                "aplicația {} (curse {}, staționare {}, completare {}{}{})",
                hm(row.total_min),
                hm(row.trip_min),
                hm(row.station_min),
                hm(row.fill_min),
                if row.excluded { ", ZI SCOASĂ" } else { "" },
                row.leave.as_deref().map(|l| format!(", concediu {l}")).unwrap_or_default()
            );
            let worked = !row.excluded && row.leave.is_none() && row.total_min > 0;
            let want = if row.excluded {
                Value::Empty
            } else if let Some(code) = &row.leave {
                Value::Text(code.clone())
            } else {
                hours(row.total_min).map_or(Value::Empty, |h| Value::Num(f64::from(h)))
            };
            let shown = engine.value(0, r, col);
            if !same(&shown, &want) {
                problems.push(format!("{label}: {app} → celula de ore trebuia {}, Excel arată {}", want.show(), shown.show()));
            }
            if let Value::Num(h) = want {
                sum_hours += h as i32;
            }

            // The times written under the hours: every trip of the day, from its departure to its arrival (the time added to
            // the day is in the first departure and the last arrival), the rest blank.
            let written: Vec<(Option<i32>, Option<i32>)> = (0..pairs).map(|p| (main.value(r + 2 + 2 * p, col).minutes(), main.value(r + 3 + 2 * p, col).minutes())).collect();
            let night_cell = engine.value(0, r + 1, col);
            let trips = if worked && hours(row.total_min).is_some() { written_trips(driver, date, row) } else { Vec::new() };
            debug_assert_eq!(worked && hours(row.total_min).is_some(), written_day(row, from));
            let shown = |pairs: &[(Option<i32>, Option<i32>)]| {
                pairs
                    .iter()
                    .filter(|(a, b)| a.is_some() || b.is_some())
                    .map(|(a, b)| format!("{}–{}", a.map_or("_".into(), hm), b.map_or("_".into(), hm)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let want: Vec<(Option<i32>, Option<i32>)> =
                (0..pairs as usize).map(|p| trips.get(p).map_or((None, None), |(a, b)| (Some(a.rem_euclid(1440)), Some(b.rem_euclid(1440))))).collect();
            if written != want {
                let trips_text = trips.iter().map(|(a, b)| format!("{}–{}", hm(a % 1440), hm(b % 1440))).collect::<Vec<_>>().join(", ");
                problems.push(format!("{label}: {app} → orele scrise „{}”, cursele zilei „{trips_text}”", shown(&written)));
            }
            // The night hours of the written times (the trips; an arrival past midnight goes on after 24:00).
            let night = night_minutes(&trips, settings);
            let want_night = (night > 0 && night >= settings.night_threshold_min).then(|| round_hours(night, from)).filter(|n| *n > 0);
            let ok = match want_night {
                Some(n) => night_cell.num() == Some(f64::from(n)),
                None => night_cell.is_blank() || night_cell.num() == Some(0.0),
            };
            if !ok {
                problems.push(format!(
                    "{label}: noaptea scrisă {}, cursele {} au {} de noapte → {}",
                    night_cell.show(),
                    shown(&want),
                    hm(night),
                    want_night.map_or("nimic".to_string(), |n| n.to_string())
                ));
            }
            sum_night += want_night.unwrap_or(0);
        }

        // The totals of the row.
        let total_col = 4 + month_days;
        let total = engine.value(0, r, total_col);
        if total.num() != Some(f64::from(sum_hours)) {
            problems.push(format!("{}: „Total ore lucrate” {}, zilele dau {sum_hours}", driver.name, total.show()));
        }
        let night_total = engine.value(0, r + 1, total_col + 1);
        let night_ok = if sum_night > 0 { night_total.num() == Some(f64::from(sum_night)) } else { night_total.is_blank() || night_total.num() == Some(0.0) };
        if !night_ok {
            problems.push(format!("{}: „ore de noapte” {}, zilele dau {sum_night}", driver.name, night_total.show()));
        }
        // The leave days of the app are the codes written in the row.
        for (code, n) in &driver.totals.leave_days {
            let written = (0..month_days).filter(|d| matches!(engine.value(0, r, 4 + d), Value::Text(t) if t.trim() == code)).count();
            if written != *n as usize {
                problems.push(format!("{}: {n} zile de {code} în aplicație, {written} scrise", driver.name));
            }
        }
    }

    // ----- "Pontaj zilnic": trips, waiting, time added, total of every day (same order of drivers)
    if let Some(grid) = wb.index("Pontaj zilnic") {
        for (k, (driver, _, _)) in order.iter().enumerate() {
            let top = 4 + 4 * k as u32;
            for row in &driver.days {
                let Some(date) = row.date else { continue };
                if (date.year(), date.month()) != (sheet.year, sheet.month) || row.excluded || row.leave.is_some() {
                    continue;
                }
                let col = 3 + date.day() - 1;
                let m = |n: u32| engine.value(grid, top + n, col).minutes().unwrap_or(0);
                let got = [m(0), m(1), m(2), m(3)];
                let want = [row.trip_min, row.station_min, row.fill_min, row.total_min];
                if got != want {
                    problems.push(format!(
                        "{} {date}: „Pontaj zilnic” {} / {} / {} / {}, aplicația {} / {} / {} / {}",
                        driver.name,
                        hm(got[0]),
                        hm(got[1]),
                        hm(got[2]),
                        hm(got[3]),
                        hm(want[0]),
                        hm(want[1]),
                        hm(want[2]),
                        hm(want[3])
                    ));
                }
            }
        }
    } else {
        problems.push("lipsește foaia „Pontaj zilnic”".into());
    }

    // ----- "Sinteză": one line per driver
    if let Some(si) = wb.index("Sinteză") {
        let s = &wb.sheets[si];
        let last = s.values.keys().map(|(r, _)| *r).max().unwrap_or(0);
        let mut seen: Vec<String> = Vec::new();
        for r in 0..=last {
            let Some(name) = s.text(r, 1) else { continue };
            let Some(driver) = sheet.drivers.iter().find(|d| norm(&d.name) == norm(&name)) else { continue };
            seen.push(norm(&driver.name));
            let v = |c: u32| engine.value(si, r, c);
            let t = &driver.totals;
            let pairs = [("curse", 4, t.trip_min), ("staționare", 5, t.station_min), ("completare", 6, t.fill_min), ("total", 7, t.total_min)];
            for (what, c, want) in pairs {
                let got = v(c).minutes().unwrap_or(0);
                if got != want {
                    problems.push(format!("Sinteză, {}: {what} {}, aplicația {}", driver.name, hm(got), hm(want)));
                }
            }
            if v(2).num() != Some(f64::from(t.days)) {
                problems.push(format!("Sinteză, {}: zile lucrate {}, aplicația {}", driver.name, v(2).show(), t.days));
            }
            if v(3).num() != Some(f64::from(t.legs)) {
                problems.push(format!("Sinteză, {}: curse regulate {}, aplicația {}", driver.name, v(3).show(), t.legs));
            }
        }
        if seen.len() != sheet.drivers.len() {
            let missing: Vec<String> = sheet.drivers.iter().map(|d| d.name.clone()).filter(|n| !seen.contains(&norm(n))).collect();
            problems.push(format!("Sinteză are {} șoferi, aplicația {}; lipsesc: {}", seen.len(), sheet.drivers.len(), missing.join(", ")));
        }
    } else {
        problems.push("lipsește foaia „Sinteză”".into());
    }
    report
}
