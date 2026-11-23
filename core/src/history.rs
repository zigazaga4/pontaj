//! The tachograph months kept on this computer: every timesheet read from PDFs is saved by month, so the sheets of past
//! months can be opened again, edited and exported without reading the PDFs a second time (see [`crate::store`] for how
//! the files are written).

use std::path::Path;

use serde::Serialize;

use crate::{rules, store, Result, Settings, Timesheet};

/// One kept month, as the list on the screen shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedMonth {
    pub year: i32,
    pub month: u32,
    pub drivers: usize,
    /// How many PDFs the month was read from.
    pub files: usize,
    /// Hours written for all the drivers.
    pub worked: f64,
    /// The file is there but cannot be read (its `.bak` is beside it).
    pub broken: bool,
}

/// Keeps the timesheet (its edits included) under its own month.
pub fn save(dir: &Path, sheet: &Timesheet) -> Result<()> {
    store::save(dir, sheet.year, sheet.month, sheet)
}

/// A kept month, calculated again with the settings of now: a rule changed since the month was saved applies to it too.
pub fn open(dir: &Path, year: i32, month: u32, settings: &Settings) -> Result<Option<Timesheet>> {
    let mut sheet: Option<Timesheet> = store::load(dir, year, month)?;
    if let Some(sheet) = &mut sheet {
        rules::recompute(sheet, settings);
    }
    Ok(sheet)
}

/// The kept months, newest first.
pub fn list(dir: &Path) -> Vec<SavedMonth> {
    store::list(dir)
        .into_iter()
        .map(|(year, month)| match store::load::<Timesheet>(dir, year, month) {
            Ok(Some(sheet)) => SavedMonth { year, month, drivers: sheet.employees.len(), files: sheet.sources.len(), worked: sheet.employees.iter().map(|e| e.totals.worked).sum(), broken: false },
            _ => SavedMonth { year, month, drivers: 0, files: 0, worked: 0.0, broken: true },
        })
        .collect()
}

/// Forgets a month (its `.bak` stays beside it).
pub fn remove(dir: &Path, year: i32, month: u32) -> Result<()> {
    store::remove(dir, year, month)
}

/// Reads PDFs like [`crate::add_pdfs`], adding them to the month they belong to when that month is kept, and keeps the result.
pub fn add_pdfs<P: AsRef<Path> + Sync>(dir: &Path, previous: Option<Timesheet>, paths: &[P], settings: &Settings) -> Result<Timesheet> {
    let sheet = crate::add_pdfs_with(previous, paths, settings, |year, month| open(dir, year, month, settings))?;
    save(dir, &sheet)?;
    Ok(sheet)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::pdf::{DriverReport, Segment};
    use crate::timesheet::{self, combine};
    use chrono::NaiveDate;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pontaj-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn month(source: &str, name: &str, month: u32, day: u32, settings: &Settings) -> Timesheet {
        let mut seg = Segment::manual(NaiveDate::from_ymd_opt(2026, month, day).unwrap(), 480, 960);
        seg.manual = false;
        seg.source = source.to_string();
        let report = DriverReport { name: name.into(), company: "EXEMPLU SRL".into(), location: String::new(), period: None, segments: vec![seg], source: source.into() };
        timesheet::build(vec![report], settings).unwrap()
    }

    #[test]
    fn a_kept_month_comes_back_with_its_edits_and_the_list_describes_it() {
        let dir = temp_dir("roundtrip");
        let settings = Settings::default();
        let mut sep = month("a.pdf", "Popescu, Ion", 9, 1, &settings);
        sep.employees[0].days[0].override_hours = Some(10.0);
        rules::recompute(&mut sep, &settings);
        save(&dir, &sep).unwrap();
        save(&dir, &month("b.pdf", "Ionescu, Vasile", 10, 2, &settings)).unwrap();

        let list = list(&dir);
        assert_eq!(list.iter().map(|m| (m.year, m.month)).collect::<Vec<_>>(), [(2026, 10), (2026, 9)], "newest first");
        assert_eq!((list[1].drivers, list[1].files, list[1].worked, list[1].broken), (1, 1, 10.0, false));

        let back = open(&dir, 2026, 9, &settings).unwrap().unwrap();
        assert_eq!(back.employees[0].days[0].override_hours, Some(10.0));
        assert_eq!(back.sources, ["a.pdf"]);
        assert!(open(&dir, 2026, 8, &settings).unwrap().is_none(), "a month never read is simply absent");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opening_applies_the_rules_of_now_and_a_broken_file_is_listed_but_not_opened() {
        let dir = temp_dir("rules");
        let settings = Settings::default();
        let mut sep = month("a.pdf", "Popescu, Ion", 9, 1, &settings);
        sep.employees[0].totals.worked = 0.0;
        sep.employees[0].days[0].hours = None;
        save(&dir, &sep).unwrap();
        let back = open(&dir, 2026, 9, &settings).unwrap().unwrap();
        assert_eq!(back.employees[0].days[0].hours, Some(8.0), "the day is calculated again from the activities");

        std::fs::write(dir.join("2026-08.json"), "{ nu e json").unwrap();
        let listed = list(&dir);
        assert!(listed.iter().any(|m| (m.month, m.broken) == (8, true)), "the unreadable month is still shown");
        assert!(open(&dir, 2026, 8, &settings).is_err());
        assert!(dir.join("2026-08.json").exists(), "and it is left where it is");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removing_a_month_takes_it_off_the_list() {
        let dir = temp_dir("remove");
        let settings = Settings::default();
        save(&dir, &month("a.pdf", "Popescu, Ion", 9, 1, &settings)).unwrap();
        save(&dir, &month("b.pdf", "Popescu, Ion", 10, 1, &settings)).unwrap();
        remove(&dir, 2026, 9).unwrap();
        assert_eq!(list(&dir).iter().map(|m| m.month).collect::<Vec<_>>(), [10]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pdfs_read_while_another_month_is_open_join_the_kept_month_from_disk() {
        let dir = temp_dir("join");
        let settings = Settings::default();
        let mut sep = month("a.pdf", "Popescu, Ion", 9, 1, &settings);
        sep.employees[0].days[0].override_hours = Some(10.0);
        save(&dir, &sep).unwrap();
        let oct = month("c.pdf", "Popescu, Ion", 10, 1, &settings);
        let more = month("b.pdf", "Popescu, Ion", 9, 2, &settings);
        let joined = combine(Some(oct), more, &settings, |year, m| open(&dir, year, m, &settings)).unwrap();
        assert_eq!((joined.month, joined.sources.len()), (9, 2));
        assert_eq!(joined.employees[0].days[0].override_hours, Some(10.0), "the edits of the kept month are in it");
        std::fs::remove_dir_all(&dir).ok();
    }
}
