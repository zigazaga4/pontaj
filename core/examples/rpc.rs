//! Dev helper for previewing the UI in a plain browser: reads `{ "cmd", "args" }`
//! from stdin and prints what the matching Tauri command would return.
use std::io::Read;

use pontaj_core::{add_pdfs, catb, recompute, Settings, Timesheet};
use serde_json::{json, Value};

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
            match add_pdfs(previous, &paths, &settings) {
                Ok(sheet) => json!(sheet),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "recalculate" => {
            let mut sheet: Timesheet = serde_json::from_value(get("timesheet")).expect("timesheet");
            recompute(&mut sheet, &settings);
            json!(sheet)
        }
        // Categoria B: no network here. `catb_fetch` reads an answer of the API saved in the file named by `CATB_API_JSON`.
        "catb_fetch" => {
            let (year, month) = (get("year").as_i64().unwrap_or(2026) as i32, get("month").as_u64().unwrap_or(10) as u32);
            let previous: Option<catb::Sheet> = serde_json::from_value(get("previous")).ok().flatten();
            let body = std::env::var("CATB_API_JSON").ok().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
            match catb::api::parse_rows(&body) {
                Ok(parsed) => json!(catb::apply_download(previous, year, month, &parsed, "preview", &settings)),
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
        _ => Value::Null,
    };
    println!("{out}");
}
