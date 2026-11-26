//! Dev helper for previewing the UI in a plain browser: reads `{ "cmd", "args" }`
//! from stdin and prints what the matching Tauri command would return.
use std::io::Read;
use std::path::PathBuf;

use pontaj_core::roster::{self, Plan, StaffMember};
use pontaj_core::timesheet::Group;
use pontaj_core::{add_pdfs, catb, combined, history, recompute, Settings, Timesheet};
use serde_json::{json, Value};

/// Where the kept PDF months live for the preview (`PONTAJ_PDF_DIR`, else a folder in the temp directory).
fn history_dir() -> std::path::PathBuf {
    std::env::var("PONTAJ_PDF_DIR").map_or_else(|_| std::env::temp_dir().join("pontaj-preview-pdf"), Into::into)
}

/// The folder of the saved months of the staff (the app's `roster` folder), `ROSTER_DIR` or a temp one.
fn roster_dir() -> PathBuf {
    std::env::var("ROSTER_DIR").map(Into::into).unwrap_or_else(|_| std::env::temp_dir().join("pontaj-preview-roster"))
}

/// A tachograph month made up for the preview (there is no PDF reader in a browser): two drivers, two truck drivers of the list.
fn preview_tachograph(year: i32, month: u32, settings: &Settings) -> Timesheet {
    let names = ["Andrei Nicolae", "Arif Redin", "Badaluta Marius Adrian", "Chenaru Florin"];
    let drivers: Vec<StaffMember> = names
        .iter()
        .map(|name| StaffMember { name: (*name).into(), function: "Șofer de autoturisme".into(), group: Group::Office, plan: Plan::standard_weekdays(), note: String::new(), moves: Vec::new(), starts_on: None, stops_on: None })
        .collect();
    let mut sheet = roster::build_sheet(&drivers, Group::Office, year, month, settings);
    sheet.company = "AUTO LUX TRADING SRL".into();
    sheet.sources = vec!["raport-tahograf.pdf".into()];
    for employee in &mut sheet.employees {
        employee.group = Group::Tachograph;
    }
    recompute(&mut sheet, settings);
    sheet
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).expect("stdin");
    let request: Value = serde_json::from_str(&input).expect("json request");
    let args = &request["args"];
    let get = |key: &str| args.get(key).cloned().unwrap_or(Value::Null);
    let settings: Settings = serde_json::from_value(get("settings")).unwrap_or_default();

    let out = match request["cmd"].as_str().unwrap_or_default() {
        "load_settings" => json!(Settings::default()),
        "parse_pdfs" => {
            let paths: Vec<String> = serde_json::from_value(get("paths")).expect("paths");
            let previous: Option<Timesheet> = serde_json::from_value(get("previous")).ok().flatten();
            // With `PONTAJ_PDF_DIR` the months are kept there, as the app keeps them in its data folder.
            let done = match std::env::var("PONTAJ_PDF_DIR") {
                Ok(dir) => history::add_pdfs(std::path::Path::new(&dir), previous, &paths, &settings),
                Err(_) => add_pdfs(previous, &paths, &settings),
            };
            match done {
                Ok(sheet) => json!(sheet),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "pdf_months" => json!(history::list(&history_dir())),
        "pdf_open" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(9) as u32);
            match history::open(&history_dir(), year, month, &settings) {
                Ok(sheet) => json!(sheet),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "pdf_save" => {
            let sheet: Timesheet = serde_json::from_value(get("timesheet")).expect("timesheet");
            match history::save(&history_dir(), &sheet) {
                Ok(()) => json!(null),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "pdf_delete" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(9) as u32);
            match history::remove(&history_dir(), year, month) {
                Ok(()) => json!(null),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "recalculate" => {
            let mut sheet: Timesheet = serde_json::from_value(get("timesheet")).expect("timesheet");
            recompute(&mut sheet, &settings);
            json!(sheet)
        }
        "remove_pdf" => {
            let mut sheet: Timesheet = serde_json::from_value(get("timesheet")).expect("timesheet");
            pontaj_core::timesheet::remove_source(&mut sheet, get("source").as_str().unwrap_or_default(), &settings);
            json!(sheet)
        }
        "preview_tachograph" => json!(preview_tachograph(get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(11) as u32, &settings)),
        // Office, garage and city: the saved months are in `ROSTER_DIR`.
        "roster_open" => {
            let group: Group = serde_json::from_value(get("group")).expect("group");
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(11) as u32);
            match roster::open(&roster_dir(), group, year, month, &settings) {
                Ok(sheet) => json!(sheet),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "roster_save" => {
            let group: Group = serde_json::from_value(get("group")).expect("group");
            let sheet: Timesheet = serde_json::from_value(get("sheet")).expect("sheet");
            match roster::save(&roster_dir(), group, &sheet) {
                Ok(()) => json!(true),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "roster_regenerate" => {
            let mut sheet: Timesheet = serde_json::from_value(get("timesheet")).expect("timesheet");
            roster::regenerate(&mut sheet, get("name").as_str().unwrap_or_default(), &settings.roster, &settings);
            json!(sheet)
        }
        // The one Excel file; Categoria B is not part of it here (its months are saved by the app, not by this helper).
        "export_all" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(11) as u32);
            let tachograph: Option<Timesheet> = serde_json::from_value(get("timesheet")).ok().flatten();
            let path = std::env::var("EXPORT_PATH").unwrap_or_else(|_| "/tmp/pontaj-export.xlsx".into());
            let result = roster::open_employees(&roster_dir(), year, month, &settings).and_then(|staff| {
                let parts = combined::Parts { year, month, tachograph: tachograph.as_ref(), catb: None, staff };
                combined::write_xlsx(&parts, &settings, std::path::Path::new(&path))
            });
            match result {
                Ok(()) => json!(path),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // Categoria B: no network here. `catb_download` reads an answer of the API saved in the file named by `CATB_API_JSON`.
        "catb_download" => {
            let body = std::env::var("CATB_API_JSON").ok().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
            match catb::api::parse_rows(&body) {
                Ok(parsed) => json!(parsed),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "catb_merge" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(10) as u32);
            let previous: Option<catb::Sheet> = serde_json::from_value(get("sheet")).ok().flatten();
            match serde_json::from_value::<catb::api::Parsed>(get("parsed")) {
                Ok(parsed) => json!(catb::merge_download(previous, year, month, &parsed, &settings)),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "catb_export" => {
            let sheet: catb::Sheet = serde_json::from_value(get("sheet")).expect("sheet");
            let path = get("path").as_str().unwrap_or("/tmp/catb.xlsx").to_string();
            match catb::excel::write_xlsx(&sheet, &settings, std::path::Path::new(&path)) {
                Ok(()) => json!({ "path": path }),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // A month saved by the app, read from the folder named by `CATB_DIR` (only read; the preview never saves into it).
        "catb_months" => json!(std::env::var("CATB_DIR").map(|dir| catb::store::list(std::path::Path::new(&dir))).unwrap_or_default()),
        "catb_open" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(11) as u32);
            let dir = PathBuf::from(std::env::var("CATB_DIR").unwrap_or_default());
            match catb::store::load(&dir, year, month) {
                Ok(Some(mut sheet)) => {
                    let (previous, next) = catb::store::load_around(&dir, year, month);
                    catb::open_saved(&mut sheet, previous, next, &settings);
                    json!(sheet)
                }
                Ok(None) => Value::Null,
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // The preview saves into the folder named by `CATB_DIR` too (a temp one in the tests), like the app does.
        "catb_save" => {
            let sheet: catb::Sheet = serde_json::from_value(get("sheet")).expect("sheet");
            match catb::store::save(&PathBuf::from(std::env::var("CATB_DIR").unwrap_or_default()), &sheet) {
                Ok(()) => json!(null),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // A shift cycle moved: the saved months of `ROSTER_DIR` follow it on the days nobody typed on.
        "roster_move_cycle" => {
            let before: StaffMember = serde_json::from_value(get("before")).expect("before");
            let moved: roster::ShiftMove = serde_json::from_value(get("moved")).expect("moved");
            match roster::move_cycle(&roster_dir(), &before, moved.from, &settings) {
                Ok(()) => json!(null),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // A planned leave taken back: its days leave the saved months of `ROSTER_DIR` / `CATB_DIR`.
        "leaves_strip" => {
            let plan: pontaj_core::leave::LeavePlan = serde_json::from_value(get("plan")).expect("plan");
            let catb_dir = PathBuf::from(std::env::var("CATB_DIR").unwrap_or_default());
            match pontaj_core::leave::strip_saved(&plan, &roster_dir(), &catb_dir, &settings) {
                Ok(()) => json!(null),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        // "Reglează după Excel": the file the native dialog would pick is the one named by `RECONCILE_FILE`.
        "catb_reconcile" => {
            let sheet: catb::Sheet = serde_json::from_value(get("sheet")).expect("sheet");
            let Ok(path) = std::env::var("RECONCILE_FILE") else { return println!("null") };
            let done = pontaj_core::pontaj_file::read(std::path::Path::new(&path)).and_then(|file| catb::reconcile::reconcile(&sheet, &file, &settings));
            match done {
                Ok(done) => json!(done),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "catb_recalculate" => {
            let mut sheet: catb::Sheet = serde_json::from_value(get("sheet")).expect("sheet");
            catb::recompute(&mut sheet, &settings);
            json!(sheet)
        }
        "catb_recalculate_driver" => {
            let mut driver: catb::Driver = serde_json::from_value(get("driver")).expect("driver");
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(10) as u32);
            catb::recompute_driver(&mut driver, year, month, &settings);
            json!(driver)
        }
        "catb_resolve_overlap" => {
            let driver: catb::Driver = serde_json::from_value(get("driver")).expect("driver");
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(10) as u32);
            let (prev, next) = (get("prev").as_str().unwrap_or_default().to_string(), get("next").as_str().unwrap_or_default().to_string());
            match catb::resolve_overlap(driver, &prev, &next, year, month, &settings) {
                Ok(done) => json!(done),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "catb_move_trip" => {
            let driver: catb::Driver = serde_json::from_value(get("driver")).expect("driver");
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(10) as u32);
            let (trip, days) = (get("trip").as_str().unwrap_or_default().to_string(), get("days").as_i64().unwrap_or(0) as i32);
            match catb::move_transfer(driver, &trip, days, year, month, &settings) {
                Ok(done) => json!(done),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        _ => Value::Null,
    };
    println!("{out}");
}
