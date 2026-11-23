//! The Categoria B Excel export checked against the app, on the app's own backend (no browser, no process per step):
//! months made up as the API sends them, every change the office can make on the screen (through JSON, like the screen),
//! the export written and read back, every formula recomputed, every driver and day compared with the app.
//!
//! `cargo test -p pontaj-core --test catb_excel` (seconds). More random months: `PONTAJ_TEST_RANDOM=300`. The months the
//! app saved, read only: `PONTAJ_CATB_DATA="$HOME/Library/Application Support/ro.autolux.pontaj"` (the folder with
//! `setari.json` and `catb/`).

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};
use pontaj_core::catb::api::{ApiRow, Parsed};
use pontaj_core::catb::{self, PauseKind, PauseState, Sheet, LEAVE_TYPES};
use pontaj_core::settings::Settings;

use support::check::{check, hm, norm, Report};
use support::edits::{self as ui, busy, regular_trips};
use support::month::{self, CONSTANTA, OTOPENI};
use support::rng::Rng;
use support::workbook::{Value, Workbook};

/// Drivers of a made-up month.
const DRIVERS: usize = 8;
/// The months the named cases run on: a short one, one with holidays, one ending the year.
const MONTHS: [(i32, u32); 3] = [(2026, 2), (2026, 11), (2026, 12)];

static FILES: AtomicUsize = AtomicUsize::new(0);
/// Milliseconds spent writing, reading back and checking the files, making the months and changing them (summed over
/// the threads).
static SPENT: [AtomicUsize; 5] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
/// Milliseconds per action, by its place in [`ACTIONS`].
static ACTION_MS: [AtomicUsize; 15] = [const { AtomicUsize::new(0) }; 15];

fn timed<T>(slot: usize, work: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = work();
    SPENT[slot].fetch_add(started.elapsed().as_millis() as usize, Ordering::Relaxed);
    out
}

/// One day of one driver as the first sheet has it.
struct WrittenDay {
    day: u32,
    hours: Value,
    night: Value,
    /// The written intervals (start, end) in minutes of the day, in the order of the rows.
    times: Vec<(Option<i32>, Option<i32>)>,
}

/// The first sheet's days that have something written, for each driver whose name contains `wanted`.
fn written(workbook: &Workbook, wanted: &str) -> Vec<(String, Vec<WrittenDay>)> {
    let wanted = norm(wanted);
    let main = &workbook.sheets[0];
    let last = main.values.keys().map(|(r, _)| *r).max().unwrap_or(0);
    let days: Vec<(u32, u32)> = (4..36).filter_map(|c| main.value(9, c).num().map(|d| (c, d as u32))).collect();
    let is_total = |r: u32| main.text(r, 0).is_some_and(|t| t.trim() == "TOTAL");
    let mut out = Vec::new();
    for r in 10..=last {
        let Some(name) = main.text(r, 1).filter(|n| norm(n).contains(&wanted)) else { continue };
        let mut end = r + 1;
        while end <= last && main.text(end, 1).is_none() && !is_total(end) {
            end += 1;
        }
        let mut list = Vec::new();
        for &(c, day) in &days {
            let times: Vec<(Option<i32>, Option<i32>)> = (r + 2..end)
                .step_by(2)
                .map(|x| (main.value(x, c).minutes(), main.value(x + 1, c).minutes()))
                .filter(|(a, b)| a.is_some() || b.is_some())
                .collect();
            if !main.value(r, c).is_blank() || !times.is_empty() {
                list.push(WrittenDay { day, hours: main.value(r, c), night: main.value(r + 1, c), times });
            }
        }
        out.push((name, list));
    }
    out
}

/// `PONTAJ_SHOW=popescu`: what the first sheet writes, day by day, for the drivers whose name contains it.
fn show(label: &str, workbook: &Workbook) {
    let Ok(wanted) = std::env::var("PONTAJ_SHOW") else { return };
    for (name, days) in written(workbook, &wanted) {
        let mut out = format!("[{label}] {name}:\n");
        for d in days {
            let times: Vec<String> = d.times.iter().map(|(a, b)| format!("{}–{}", a.map_or("_".into(), hm), b.map_or("_".into(), hm))).collect();
            out += &format!("  {:2}: ore {}, noapte {}, {}\n", d.day, d.hours.show(), d.night.show(), times.join("  "));
        }
        eprint!("{out}");
    }
}

/// The sheet as the "Export Excel" button sends it (it travels as JSON), written and read back.
fn export(label: &str, sheet: &Sheet, settings: &Settings) -> (Sheet, Workbook) {
    let sheet: Sheet = ui::through_json(sheet);
    let path = std::env::temp_dir().join(format!("pontaj-catb-test-{}-{}.xlsx", std::process::id(), FILES.fetch_add(1, Ordering::Relaxed)));
    timed(0, || catb::excel::write_xlsx(&sheet, settings, &path)).unwrap_or_else(|e| panic!("[{label}] exportul: {e}"));
    let workbook = timed(1, || Workbook::read(&path));
    std::fs::remove_file(&path).ok();
    (sheet, workbook)
}

/// What the "Export Excel" button does with the sheet on the screen (it travels as JSON), read back and compared.
fn export_and_check(label: &str, sheet: &Sheet, settings: &Settings) -> Report {
    let (sheet, workbook) = export(label, sheet, settings);
    show(label, &workbook);
    let mut report = timed(2, || check(&sheet, settings, &workbook));
    for problem in &mut report.problems {
        *problem = format!("[{label}] {problem}");
    }
    report
}

/// A month on the screen: the sheet, the settings, the API's last answer and the dice of the random cases.
struct Case {
    sheet: Sheet,
    settings: Settings,
    answer: Parsed,
    rng: Rng,
}

impl Case {
    fn new(seed: u64, year: i32, month: u32, drivers: usize, settings: Settings) -> Self {
        let (sheet, answer) = timed(3, || month::downloaded(seed, year, month, drivers, &settings));
        Case { sheet, settings, answer, rng: Rng::new(seed ^ 0x5eed) }
    }

    /// One of [`ACTIONS`], timed.
    fn apply(&mut self, index: usize) {
        let started = std::time::Instant::now();
        timed(4, || (ACTIONS[index].1)(self));
        ACTION_MS[index].fetch_add(started.elapsed().as_millis() as usize, Ordering::Relaxed);
    }

    fn in_month(&self, date: NaiveDate) -> bool {
        (date.year(), date.month()) == (self.sheet.year, self.sheet.month)
    }

    /// Worked days of the month of a driver.
    fn worked(&self, id: i64) -> Vec<NaiveDate> {
        ui::driver(&self.sheet, id).map(ui::worked_days).unwrap_or_default().into_iter().filter(|d| self.in_month(*d)).collect()
    }
}

fn minute_of(at: NaiveDateTime) -> i32 {
    (at.hour() * 60 + at.minute()) as i32
}

// ---------------------------------------------------------------------------------------------------------------------
// What the office does on the screen
// ---------------------------------------------------------------------------------------------------------------------

/// Departures and arrivals typed over the API's: later, longer, past midnight, then one emptied (back to the API).
fn type_times(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let trips = regular_trips(ui::driver(&c.sheet, id).unwrap());
        for (k, (trip, at)) in trips.iter().enumerate().take(8) {
            let minute = minute_of(*at);
            match k % 4 {
                0 => ui::type_time(&mut c.sheet, id, trip, false, minute + 35, &c.settings),
                1 => ui::type_time(&mut c.sheet, id, trip, true, minute + 200, &c.settings),
                2 => ui::type_time(&mut c.sheet, id, trip, false, 22 * 60 + 40, &c.settings) && ui::type_time(&mut c.sheet, id, trip, true, 75, &c.settings),
                _ => ui::type_time(&mut c.sheet, id, trip, true, minute + 95, &c.settings) && ui::clear_time(&mut c.sheet, id, trip, true, &c.settings),
            };
        }
    }
}

/// Trips taken out of the pontaj, one put back.
fn remove_trips(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let trips = regular_trips(ui::driver(&c.sheet, id).unwrap());
        for (trip, _) in trips.iter().step_by(4).take(3) {
            ui::remove_trip(&mut c.sheet, id, trip, &c.settings);
        }
        if let Some((trip, _)) = trips.first() {
            ui::restore_trip(&mut c.sheet, id, trip, &c.settings);
        }
    }
}

/// Trips added by hand: a day of five trips (one past midnight), one of them deleted again; an empty day given a trip.
fn add_trips(c: &mut Case) {
    for (n, id) in busy(&c.sheet, 3).into_iter().enumerate() {
        let worked = c.worked(id);
        let Some(day) = worked.get(worked.len() / 2).copied() else { continue };
        let at = |h: u32, m: u32| day.and_hms_opt(h, m, 0).unwrap();
        let s = &c.settings;
        ui::add_trip(&mut c.sheet, id, &format!("m-{n}-1"), CONSTANTA, OTOPENI, at(13, 0), 180, s);
        ui::add_trip(&mut c.sheet, id, &format!("m-{n}-2"), OTOPENI, CONSTANTA, at(16, 30), 180, s);
        ui::add_trip(&mut c.sheet, id, &format!("m-{n}-3"), CONSTANTA, "Mamaia", at(20, 0), 40, s);
        ui::add_trip(&mut c.sheet, id, &format!("m-{n}-4"), "Mamaia", CONSTANTA, at(21, 15), 30, s);
        ui::add_trip(&mut c.sheet, id, &format!("m-{n}-5"), CONSTANTA, OTOPENI, at(22, 50), 180, s);
        ui::remove_trip(&mut c.sheet, id, &format!("m-{n}-3"), s);
        // A day without trips gets one.
        let free = ui::driver(&c.sheet, id).unwrap().free_days.iter().copied().find(|d| c.in_month(*d) && d.weekday().number_from_monday() < 6);
        if let Some(free) = free {
            ui::add_trip(&mut c.sheet, id, &format!("m-{n}-6"), OTOPENI, CONSTANTA, free.and_hms_opt(9, 10, 0).unwrap(), 185, &c.settings);
        }
    }
}

/// Pauses decided by the office: waiting, at home, more hours than the gap, hours typed on an undecided one.
fn decide_pauses(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let pauses: Vec<(String, String, PauseKind, i32)> =
            ui::driver(&c.sheet, id).unwrap().pauses.iter().map(|p| (p.prev.clone(), p.next.clone(), p.kind, p.gap_min)).collect();
        for (k, (prev, next, kind, gap)) in pauses.into_iter().filter(|p| p.2 != PauseKind::Overlap).take(8).enumerate() {
            let s = &c.settings;
            match k % 4 {
                0 => ui::decide_pause(&mut c.sheet, id, &prev, &next, PauseState::Stayed, None, "", s),
                1 => ui::decide_pause(&mut c.sheet, id, &prev, &next, PauseState::Off, None, "acasă", s),
                2 => ui::decide_pause(&mut c.sheet, id, &prev, &next, PauseState::Stayed, Some(f64::from(gap) / 60.0 + 2.0), "mai mult decât pauza", s),
                _ => ui::decide_pause(&mut c.sheet, id, &prev, &next, if kind == PauseKind::Rest { PauseState::Auto } else { PauseState::Stayed }, Some(1.5), "", s),
            };
        }
    }
}

/// Time added typed by hand: none, a little, a lot, on one side, then back to the rules.
fn type_fills(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let days = c.worked(id);
        let s = &c.settings;
        let typed: [(usize, bool, Option<i32>); 7] =
            [(0, true, Some(0)), (0, false, Some(0)), (1, true, Some(15)), (2, false, Some(240)), (3, true, Some(90)), (3, true, None), (4, true, Some(600))];
        for (i, before, minutes) in typed {
            if let Some(day) = days.get(i) {
                ui::type_fill(&mut c.sheet, id, *day, before, minutes, s);
            }
        }
    }
}

/// Hours fixed by hand (what "Reglează după Excel" leaves): shorter and longer than the day, a half hour, on a day with no
/// trip at all, and one day given back to the rules.
fn fix_hours(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let days = c.worked(id);
        let s = &c.settings;
        for (i, minutes) in [(0, 480), (1, 270), (2, 720), (3, 30), (3, 450)] {
            if let Some(day) = days.get(i) {
                ui::set_hours(&mut c.sheet, id, *day, Some(minutes), s);
            }
        }
        if let Some(day) = days.get(4) {
            ui::set_hours(&mut c.sheet, id, *day, Some(600), s);
            ui::set_hours(&mut c.sheet, id, *day, None, s);
        }
        let free: Vec<NaiveDate> = ui::driver(&c.sheet, id).unwrap().free_days.iter().copied().filter(|d| c.in_month(*d)).collect();
        for day in free.iter().take(2) {
            ui::set_hours(&mut c.sheet, id, *day, Some(360), s);
        }
        if !days.is_empty() {
            let fixed = ui::driver(&c.sheet, id).unwrap().days.iter().filter(|d| d.adjusted).count();
            assert!(fixed > 0, "nicio zi fixată, deși s-au fixat ore");
        }
    }
}

/// Days taken out of the pontaj (one put back) and leave on free days, every code, one taken back.
fn days_and_leave(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let days = c.worked(id);
        for i in [1, 2, 2] {
            if let Some(day) = days.get(i) {
                ui::toggle_day(&mut c.sheet, id, *day, &c.settings);
            }
        }
        let free: Vec<NaiveDate> = ui::driver(&c.sheet, id).unwrap().free_days.iter().copied().filter(|d| c.in_month(*d)).collect();
        for (k, day) in free.iter().enumerate().take(8) {
            ui::set_leave(&mut c.sheet, id, *day, Some(LEAVE_TYPES[k % LEAVE_TYPES.len()].0), &c.settings);
        }
        if let Some(day) = free.first() {
            ui::set_leave(&mut c.sheet, id, *day, None, &c.settings);
        }
    }
}

/// The drives made by the app moved a day earlier or later (a move out of the month is refused), one moved back.
fn move_drives(c: &mut Case) {
    let ids: Vec<i64> = c.sheet.drivers.iter().map(|d| d.id).collect();
    for id in ids {
        let drives: Vec<String> = ui::driver(&c.sheet, id).unwrap().trips.iter().filter(|t| t.generated && !t.edit.removed).map(|t| t.id.clone()).take(3).collect();
        for (k, drive) in drives.iter().enumerate() {
            let days = if k % 2 == 0 { -1 } else { 1 };
            ui::move_drive(&mut c.sheet, id, drive, days, &c.settings);
            if k == 2 {
                ui::move_drive(&mut c.sheet, id, drive, -days, &c.settings);
            }
        }
    }
}

fn resolve_overlaps(c: &mut Case) {
    ui::resolve_all_overlaps(&mut c.sheet, &c.settings);
}

/// Prices and places typed: a far place, a local one, the price of a far trip, one place taken back.
fn prices_and_places(c: &mut Case) {
    for id in busy(&c.sheet, 3) {
        let trips = regular_trips(ui::driver(&c.sheet, id).unwrap());
        let s = &c.settings;
        if let Some((trip, _)) = trips.get(1) {
            ui::set_place(&mut c.sheet, id, trip, true, "Tulcea", s);
            ui::set_price(&mut c.sheet, id, trip, Some(350.0), s);
        }
        if let Some((trip, _)) = trips.get(3) {
            ui::set_place(&mut c.sheet, id, trip, true, "Mamaia", s);
        }
        if let Some((trip, _)) = trips.get(5) {
            ui::set_place(&mut c.sheet, id, trip, false, "Medgidia", s);
            ui::set_place(&mut c.sheet, id, trip, false, "", s);
        }
    }
}

/// Two drivers taken out of the pontaj, one added back.
fn hide_drivers(c: &mut Case) {
    let ids: Vec<i64> = c.sheet.drivers.iter().map(|d| d.id).collect();
    for id in ids.iter().skip(1).take(2) {
        ui::hide_driver(&mut c.sheet, *id, &mut c.settings);
    }
    if let Some(id) = ids.get(1) {
        ui::show_driver(&mut c.sheet, *id, &mut c.settings);
    }
}

/// The silent refresh with the same answer.
fn refresh_same(c: &mut Case) {
    let answer = Parsed { rows: c.answer.rows.clone(), ..c.answer };
    ui::refresh(&mut c.sheet, &answer, &c.settings);
}

/// The silent refresh after the API changed: a trip moved by two hours, one gone, a new one.
fn refresh_changed(c: &mut Case) {
    let mut rows: Vec<ApiRow> = c.answer.rows.clone();
    let in_month = |r: &ApiRow| (r.departure.year(), r.departure.month()) == (c.sheet.year, c.sheet.month);
    let picks: Vec<usize> = (0..rows.len()).filter(|&i| in_month(&rows[i])).collect();
    if picks.len() > 12 {
        let moved = picks[c.rng.below(picks.len())];
        rows[moved].departure += Duration::hours(2);
        rows[moved].arrival = rows[moved].arrival.map(|a| a + Duration::hours(2));
        let mut new = rows[picks[3]].clone();
        new.trip_id = format!("eco-nou-{}", c.rng.below(100_000));
        new.departure = new.departure + Duration::days(1) - Duration::minutes(i64::from(minute_of(new.departure))) + Duration::hours(14);
        new.arrival = Some(new.departure + Duration::minutes(180));
        rows.remove(picks[7]);
        rows.push(new);
    }
    c.answer = Parsed { rows, ..c.answer };
    let answer = Parsed { rows: c.answer.rows.clone(), ..c.answer };
    ui::refresh(&mut c.sheet, &answer, &c.settings);
}

/// The app closed and opened again.
fn reopen(c: &mut Case) {
    ui::reopen(&mut c.sheet, &c.settings);
}

type Action = fn(&mut Case);

const ACTIONS: [(&str, Action); 15] = [
    ("ore tastate", type_times),
    ("curse scoase", remove_trips),
    ("curse adăugate", add_trips),
    ("pauze decise", decide_pauses),
    ("completare tastată", type_fills),
    ("ore fixate", fix_hours),
    ("zile scoase și concedii", days_and_leave),
    ("deplasări mutate", move_drives),
    ("suprapuneri rezolvate", resolve_overlaps),
    ("prețuri și locuri", prices_and_places),
    ("șoferi scoși", hide_drivers),
    ("actualizare la fel", refresh_same),
    ("actualizare schimbată", refresh_changed),
    ("redeschisă", reopen),
    ("nimic", |_| {}),
];

fn all_actions(c: &mut Case) {
    for index in 0..ACTIONS.len() {
        c.apply(index);
    }
}

/// The settings the office can change in "Reguli", one at a time.
fn variants() -> Vec<(&'static str, Settings)> {
    let base = Settings::default();
    let with = |change: &dyn Fn(&mut Settings)| {
        let mut s = base.clone();
        change(&mut s);
        s
    };
    vec![
        ("rotunjire de la 1 minut", with(&|s| s.round_up_from_min = 1)),
        ("rotunjire de la 30", with(&|s| s.round_up_from_min = 30)),
        ("rotunjire de la 60", with(&|s| s.round_up_from_min = 60)),
        ("fără completare", with(&|s| s.catb.fill_day = false)),
        ("fără deplasări generate", with(&|s| s.catb.generate_transfers = false)),
        ("7 ore pe zi", with(&|s| s.hours_per_day = 7)),
        ("completare 0 pe cursă", with(&|s| s.catb.fill_per_trip_min = 0)),
        ("completare 90 pe cursă", with(&|s| s.catb.fill_per_trip_min = 90)),
        ("noapte 22-06", with(&|s| (s.night_start, s.night_end) = ("22:00".into(), "06:00".into()))),
        ("noapte 22-07 fără prag", with(&|s| (s.night_start, s.night_end, s.night_threshold_min) = ("22:00".into(), "07:00".into(), 0))),
        ("prag noapte 60", with(&|s| s.night_threshold_min = 60)),
        ("cu bani", with(&|s| s.catb.show_money = true)),
    ]
}

// ---------------------------------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------------------------------

/// Runs the jobs on all the cores (each one is independent: its own month, its own file).
fn run_all(jobs: Vec<Box<dyn FnOnce() -> Report + Send>>) -> Report {
    let jobs = Mutex::new(jobs.into_iter().map(Some).collect::<Vec<_>>());
    let next = AtomicUsize::new(0);
    let total = Mutex::new(Report::default());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.lock().unwrap().get_mut(i).and_then(Option::take) else { break };
                let report = job();
                total.lock().unwrap().add(report);
            });
        }
    });
    total.into_inner().unwrap()
}

fn finish(name: &str, report: Report, exports: usize, started: std::time::Instant) {
    eprintln!(
        "{name}: {exports} exporturi, {} șoferi, {} zile, {} formule recalculate, {} șoferi cu rânduri în plus (3+ curse pe zi), {:.1} s",
        report.drivers,
        report.days,
        report.formulas,
        report.extra_rows,
        started.elapsed().as_secs_f64()
    );
    let ms = |i: usize| SPENT[i].load(Ordering::Relaxed);
    eprintln!(
        "  timp pe fire (toate testele de până acum): luni făcute {} ms, schimbări {} ms, scriere {} ms, citire {} ms, verificare {} ms",
        ms(3),
        ms(4),
        ms(0),
        ms(1),
        ms(2)
    );
    let per_action: Vec<String> = ACTIONS.iter().zip(&ACTION_MS).map(|((name, _), t)| format!("{name} {}", t.load(Ordering::Relaxed))).collect();
    eprintln!("  pe schimbare (ms): {}", per_action.join(", "));
    if !report.problems.is_empty() {
        for problem in report.problems.iter().take(60) {
            eprintln!("  {problem}");
        }
        panic!("{name}: {} nepotriviri între aplicație și Excel", report.problems.len());
    }
}

/// Wednesday 4 November: Constanța → Otopeni 14:30–17:30. The way back (made by the app) is put on Thursday with "+1 zi",
/// 03:30–07:30 typed, and the 10 hours between chosen as "Pauză". The Excel has the whole outing on Wednesday, nothing on
/// Thursday, like for any other way back.
#[test]
fn a_way_back_the_office_moved_a_day_is_written_on_the_day_of_the_way_there() {
    let body = r#"{"data":[
        {"driver_id":11,"driver_name":"Epure Test","trip":{"type":"eco","id":"eco-1"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-11-04 14:30","duration_minutes":180},
        {"driver_id":11,"driver_name":"Epure Test","trip":{"type":"eco","id":"eco-2"},"route":{"from":"Constanta","to":"Aeroport Otopeni Bucuresti"},"departure":"2026-11-07 03:30","duration_minutes":180}]}"#;
    let settings = Settings::default();
    let mut sheet = catb::apply_download(None, 2026, 11, &catb::api::parse_rows(body).unwrap(), "x", &settings);
    let (id, drive) = (11, "gen-ret-eco-1");
    assert!(ui::move_drive(&mut sheet, id, drive, 1, &settings));
    assert!(ui::type_time(&mut sheet, id, drive, false, 3 * 60 + 30, &settings));
    assert!(ui::type_time(&mut sheet, id, drive, true, 7 * 60 + 30, &settings));
    assert!(ui::decide_pause(&mut sheet, id, "eco-1", drive, PauseState::Off, None, "", &settings));

    let (sheet, workbook) = export("epure", &sheet, &settings);
    let problems = check(&sheet, &settings, &workbook).problems;
    assert!(problems.is_empty(), "{problems:#?}");
    let days = written(&workbook, "epure").remove(0).1;
    let shown = |d: &WrittenDay| format!("{}: ore {}, {:?}", d.day, d.hours.show(), d.times);
    let fourth = days.iter().find(|d| d.day == 4).unwrap_or_else(|| panic!("nothing on the 4th: {:?}", days.iter().map(shown).collect::<Vec<_>>()));
    // Both trips on the 4th: 3 h + 4 h and the hour added to the day (half before the first departure, half after the last arrival).
    assert_eq!(fourth.hours.num(), Some(8.0), "{}", shown(fourth));
    assert_eq!(fourth.times, [(Some(14 * 60), Some(17 * 60 + 30)), (Some(3 * 60 + 30), Some(8 * 60))], "{}", shown(fourth));
    assert!(days.iter().all(|d| d.day != 5), "nothing on the 5th: {:?}", days.iter().map(shown).collect::<Vec<_>>());
}

#[test]
fn every_change_of_the_screen_reaches_the_excel() {
    let started = std::time::Instant::now();
    let mut jobs: Vec<Box<dyn FnOnce() -> Report + Send>> = Vec::new();
    for (m, (year, month)) in MONTHS.into_iter().enumerate() {
        for (a, (name, _)) in ACTIONS.into_iter().enumerate() {
            jobs.push(Box::new(move || {
                let mut case = Case::new(100 + m as u64 * 31 + a as u64, year, month, DRIVERS, Settings::default());
                case.apply(a);
                export_and_check(&format!("{month:02}.{year} {name}"), &case.sheet, &case.settings)
            }));
        }
        jobs.push(Box::new(move || {
            let mut case = Case::new(7 + m as u64, year, month, DRIVERS, Settings::default());
            all_actions(&mut case);
            export_and_check(&format!("{month:02}.{year} toate schimbările"), &case.sheet, &case.settings)
        }));
    }
    // Every change, then the rules changed (the month is recomputed with the new ones, the edits stay).
    for (name, settings) in variants() {
        jobs.push(Box::new(move || {
            let mut case = Case::new(77, 2026, 10, DRIVERS, Settings::default());
            all_actions(&mut case);
            let mut current = case.settings.clone();
            let hidden = current.catb.hidden_drivers.clone();
            ui::change_settings(&mut case.sheet, &mut current, Settings { catb: catb::CatbSettings { hidden_drivers: hidden, ..settings.catb.clone() }, ..settings });
            export_and_check(&format!("10.2026 reguli: {name}"), &case.sheet, &current)
        }));
    }
    let exports = jobs.len();
    finish("cazuri numite", run_all(jobs), exports, started);
}

#[test]
fn random_months_and_random_changes_reach_the_excel() {
    let started = std::time::Instant::now();
    let count: u64 = std::env::var("PONTAJ_TEST_RANDOM").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let jobs: Vec<Box<dyn FnOnce() -> Report + Send>> = (0..count)
        .map(|seed| {
            Box::new(move || {
                let mut dice = Rng::new(9000 + seed);
                let (year, month) = (2025 + dice.range(0, 2), dice.range(1, 12) as u32);
                let mut settings = Settings::default();
                settings.round_up_from_min = *dice.pick(&[1, 15, 20, 30, 45, 60]);
                settings.catb.fill_day = dice.chance(80);
                settings.catb.generate_transfers = dice.chance(85);
                settings.night_threshold_min = *dice.pick(&[0, 60, 180]);
                let mut case = Case::new(9000 + seed, year, month, 5 + dice.below(6), settings);
                let steps = 4 + dice.below(10);
                for _ in 0..steps {
                    case.apply(dice.below(ACTIONS.len()));
                }
                export_and_check(&format!("aleator {seed} ({month:02}.{year}, {steps} pași)"), &case.sheet, &case.settings)
            }) as Box<dyn FnOnce() -> Report + Send>
        })
        .collect();
    finish("luni aleatoare", run_all(jobs), count as usize, started);
}

/// The months saved by the app on this computer, read only: as they are, and with every change of the screen on them.
#[test]
fn the_saved_months_reach_the_excel() {
    let Ok(dir) = std::env::var("PONTAJ_CATB_DATA") else {
        eprintln!("PONTAJ_CATB_DATA nu e setat: lunile salvate nu sunt verificate");
        return;
    };
    let started = std::time::Instant::now();
    let dir = PathBuf::from(dir);
    let settings = saved_settings(&dir);
    let store = dir.join("catb");
    let mut jobs: Vec<Box<dyn FnOnce() -> Report + Send>> = Vec::new();
    for (year, month) in catb::store::list(&store) {
        for with_changes in [false, true] {
            let (store, settings) = (store.clone(), settings.clone());
            jobs.push(Box::new(move || {
                let mut sheet = catb::store::load(&store, year, month).ok().flatten().expect("luna salvată");
                let (previous, next) = catb::store::load_around(&store, year, month);
                catb::open_saved(&mut sheet, previous, next, &settings);
                let mut case = Case { sheet, settings, answer: month::parsed(Vec::new()), rng: Rng::new(u64::from(month)) };
                let label = format!("salvată {month:02}.{year}{}", if with_changes { " + schimbări" } else { "" });
                if with_changes {
                    // Not the refreshes: the saved month has no API answer here.
                    for (index, (name, _)) in ACTIONS.iter().enumerate() {
                        if !name.starts_with("actualizare") {
                            case.apply(index);
                        }
                    }
                }
                export_and_check(&label, &case.sheet, &case.settings)
            }));
        }
    }
    let exports = jobs.len();
    finish("luni salvate", run_all(jobs), exports, started);
}

fn saved_settings(dir: &Path) -> Settings {
    std::fs::read_to_string(dir.join("setari.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .map(Settings::migrated)
        .unwrap_or_default()
}
