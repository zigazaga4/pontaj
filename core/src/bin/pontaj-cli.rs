//! Command-line variant: `pontaj-cli raport1.pdf [raport2.pdf ...] [-o pontaj.xlsx]`.

use std::path::PathBuf;

use pontaj_core::{default_output_path, process_pdfs, time::fmt_duration, write_xlsx, Settings};

fn main() {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => output = args.next().map(PathBuf::from),
            _ => inputs.push(PathBuf::from(arg)),
        }
    }
    if inputs.is_empty() {
        eprintln!("Utilizare: pontaj-cli raport.pdf [alt-raport.pdf ...] [-o pontaj.xlsx]");
        std::process::exit(2);
    }

    let settings = Settings::default();
    let sheet = match process_pdfs(&inputs, &settings) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Eroare: {e}");
            std::process::exit(1);
        }
    };

    println!("{} — {:02}.{} — normă {} ore", sheet.company, sheet.month, sheet.year, sheet.norm_hours);
    for emp in &sheet.employees {
        let days: Vec<String> = emp
            .days
            .iter()
            .filter(|d| d.hours.is_some() || d.worked_min > 0)
            .map(|d| {
                let night = d.night_hours.map(|n| format!("/n{n}")).unwrap_or_default();
                format!("{}:{}({}){night}", d.day, d.hours.unwrap_or(0), fmt_duration(d.worked_min))
            })
            .collect();
        println!("\n{} — {} ore, {} noapte\n  {}", emp.name, emp.totals.worked, emp.totals.night, days.join("  "));
    }
    for w in &sheet.warnings {
        println!("! {w}");
    }

    let out = output.unwrap_or_else(|| default_output_path(&std::env::current_dir().unwrap_or_default(), &sheet, &settings));
    match write_xlsx(&sheet, &settings, &out) {
        Ok(()) => println!("\nExcel salvat: {}", out.display()),
        Err(e) => {
            eprintln!("Eroare: {e}");
            std::process::exit(1);
        }
    }
}
