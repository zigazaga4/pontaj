//! End-to-end check on a real report: `PONTAJ_PDF=raport.pdf cargo test -p pontaj-core --test export`.
//! Skipped when `PONTAJ_PDF` is not set. Writes the workbook to `PONTAJ_XLSX` (or the temp dir).

use pontaj_core::{process_pdfs, recompute, timesheet::CODES, write_xlsx, Settings};

#[test]
fn export_with_absences_and_manual_hours() {
    let Ok(pdf) = std::env::var("PONTAJ_PDF") else {
        eprintln!("PONTAJ_PDF not set, skipping");
        return;
    };
    let settings = Settings::default();
    let mut sheet = process_pdfs(&[pdf], &settings).expect("PDF parsed");

    // Absences for the 2nd and 3rd driver, plus one manual day for the 1st.
    assert!(sheet.employees.len() >= 3, "the test PDF needs at least 3 drivers");
    let edits: [(usize, &[u32], Option<&str>, Option<i32>); 3] = [
        (1, &[28, 29, 30], Some("CO"), None),
        (2, &[21, 22, 23, 24, 25, 28, 29, 30], Some("CO"), None),
        (0, &[1], None, Some(7)),
    ];
    for (idx, days, code, hours) in edits {
        let emp = &mut sheet.employees[idx];
        for d in days {
            let entry = &mut emp.days[*d as usize - 1];
            entry.code = code.map(str::to_string);
            entry.override_hours = hours;
        }
    }
    recompute(&mut sheet, &settings);

    let co = CODES.iter().position(|(c, _)| *c == "CO").unwrap();
    let olp = CODES.iter().position(|(c, _)| *c == "OLP").unwrap();
    for emp in &sheet.employees {
        let t = &emp.totals;
        let worked: i32 = emp.days.iter().filter_map(|d| d.hours).sum();
        assert_eq!(t.worked, worked, "{}", emp.name);
        assert_eq!(t.not_worked, (sheet.norm_hours - worked).max(0), "{}", emp.name);
        let absences: i32 = t.code_hours.iter().enumerate().filter(|(i, _)| *i != olp).map(|(_, h)| h).sum();
        assert_eq!(t.code_hours[olp], (t.not_worked - absences).max(0), "{}", emp.name);
        assert_eq!(t.overtime, (worked + absences - sheet.norm_hours).max(0), "{}", emp.name);
        for d in &emp.days {
            let span: i32 = d.intervals.iter().map(|i| i.end - i.start).sum();
            assert_eq!(span, d.hours.unwrap_or(0) * 60, "{} day {}", emp.name, d.day);
        }
    }
    assert_eq!(sheet.employees[1].totals.code_hours[co], 24);
    assert_eq!(sheet.employees[2].totals.code_hours[co], 64);

    let out = std::env::var("PONTAJ_XLSX").map(Into::into).unwrap_or_else(|_| std::env::temp_dir().join("pontaj_verify.xlsx"));
    write_xlsx(&sheet, &settings, &out).expect("workbook written");
    assert!(std::fs::metadata(&out).unwrap().len() > 5_000);
}
