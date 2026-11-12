//! Dev helper for previewing the UI in a plain browser: reads `{ "cmd", "args" }`
//! from stdin and prints what the matching Tauri command would return.
use std::io::Read;

use pontaj_core::{add_pdfs, recompute, Settings, Timesheet};
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
        _ => Value::Null,
    };
    println!("{out}");
}
