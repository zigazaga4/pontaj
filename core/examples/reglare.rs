//! Dev helper: shows what "Reglează după Excel" would do with a file, without touching any saved data.
//!
//! `cargo run -p pontaj-core --example reglare -- <fișier.xlsx> [dosarul catb al aplicației] [setari.json]`
//!
//! The saved month is only read; the month with the file's hours is written to `/tmp/pontaj-reglat.xlsx` (and read back, to
//! check that the new export says what the file said).
use std::path::{Path, PathBuf};

use pontaj_core::catb::{self, reconcile};
use pontaj_core::{pontaj_file, Settings};

fn app_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/ro.autolux.pontaj")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(file_path) = args.first() else {
        eprintln!("folosire: reglare <fișier.xlsx> [dosar catb] [setari.json]");
        std::process::exit(2);
    };
    let catb_dir = args.get(1).map_or_else(|| app_dir().join("catb"), PathBuf::from);
    let settings_path = args.get(2).map_or_else(|| app_dir().join("setari.json"), PathBuf::from);
    let settings = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|json| serde_json::from_str::<Settings>(&json).ok())
        .map_or_else(Settings::default, Settings::migrated);

    let file = pontaj_file::read(Path::new(file_path)).unwrap_or_else(|e| {
        eprintln!("EROARE la citire: {e}");
        std::process::exit(1);
    });
    println!("Fișier: luna {:02}.{}, {} persoane", file.month, file.year, file.people.len());

    let mut sheet = catb::store::load(&catb_dir, file.year, file.month).expect("citirea lunii salvate").unwrap_or_else(|| {
        eprintln!("Luna {:02}.{} nu este salvată în {}", file.month, file.year, catb_dir.display());
        std::process::exit(1);
    });
    let (previous, next) = catb::store::load_around(&catb_dir, file.year, file.month);
    catb::open_saved(&mut sheet, previous, next, &settings);
    println!("Aplicația: {} șoferi, descărcat din API la {}", sheet.drivers.len(), sheet.fetched_at.as_deref().unwrap_or("?"));

    let done = match reconcile::reconcile(&sheet, &file, &settings) {
        Ok(done) => done,
        Err(e) => {
            eprintln!("EROARE: {e}");
            std::process::exit(1);
        }
    };
    let report = &done.report;
    println!("\nGăsiți în fișier: {} · în aplicație dar nu în fișier: {:?} · alte persoane din fișier: {}", report.matched, report.not_in_file, report.others);
    println!("\n{} zile se schimbă:", report.changes.len());
    for change in &report.changes {
        println!("  {:<28} {}  aplicația: {:<4} → fișier: {}", change.driver, change.date.map(|d| d.format("%d.%m").to_string()).unwrap_or_default(), if change.app.is_empty() { "—" } else { &change.app }, if change.excel.is_empty() { "—" } else { &change.excel });
    }
    println!("\n{} zile nu s-au putut aduce la valoarea din fișier:", report.skipped.len());
    for skipped in &report.skipped {
        println!("  {:<28} {}  fișier: {:<6} {}", skipped.driver, skipped.date.map(|d| d.format("%d.%m").to_string()).unwrap_or_default(), skipped.excel, skipped.reason);
    }

    let out = Path::new("/tmp/pontaj-reglat.xlsx");
    catb::excel::write_xlsx(&done.sheet, &settings, out).expect("scrierea exportului");
    let back = pontaj_file::read(out).expect("citirea exportului nou");
    let (mut same, mut differ) = (0, 0);
    for person in &file.people {
        let Some(new) = back.people.iter().find(|p| p.name == person.name) else { continue };
        for (old, new) in person.days.iter().zip(&new.days) {
            if old == new { same += 1 } else { differ += 1 }
        }
    }
    println!("\nExportul nou față de fișier, zi cu zi: {same} la fel, {differ} diferite (zilele neaduse la fișier se văd mai sus)");
}
