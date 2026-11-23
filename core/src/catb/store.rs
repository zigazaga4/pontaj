//! Saving the months: one JSON file per month (`2026-10.json`) in a folder of the app.
//! The office's edits live only here and in the Excel files, so a write never destroys the previous version:
//! the file is written beside the old one and moved into place, and the old one is kept as `2026-10.json.bak`.

use std::path::Path;

use super::model::Sheet;
use crate::{month_file, Result};

/// Saves a month.
pub fn save(dir: &Path, sheet: &Sheet) -> Result<()> {
    month_file::save(dir, sheet.year, sheet.month, sheet)
}

/// A saved month, if there is one. A file that cannot be read is an error (and stays where it is).
pub fn load(dir: &Path, year: i32, month: u32) -> Result<Option<Sheet>> {
    month_file::load(dir, year, month)
}

/// The months saved on each side of one (before, after), if there are any: the trips just outside a month are theirs (see
/// [`super::borrow_neighbors`]). A file that cannot be read counts as absent here; it is reported when that month is opened.
pub fn load_around(dir: &Path, year: i32, month: u32) -> (Option<Sheet>, Option<Sheet>) {
    let (before_year, before_month) = if month == 1 { (year - 1, 12) } else { (year, month - 1) };
    let (after_year, after_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    (load(dir, before_year, before_month).ok().flatten(), load(dir, after_year, after_month).ok().flatten())
}

/// The saved months, newest first.
pub fn list(dir: &Path) -> Vec<(i32, u32)> {
    month_file::list(dir)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::merge::new_sheet;
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pontaj-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_month_comes_back_as_it_was_saved() {
        let dir = temp_dir("roundtrip");
        let mut sheet = new_sheet(2026, 10);
        sheet.fetched_at = Some("17.11.2026 10:00".into());
        sheet.notices.push("o notă".into());
        save(&dir, &sheet).unwrap();
        assert_eq!(load(&dir, 2026, 10).unwrap(), Some(sheet));
        assert_eq!(load(&dir, 2026, 11).unwrap(), None, "another month is simply absent");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_previous_version_is_kept_and_months_are_listed_newest_first() {
        let dir = temp_dir("backup");
        let mut sheet = new_sheet(2026, 9);
        save(&dir, &sheet).unwrap();
        save(&dir, &new_sheet(2026, 10)).unwrap();
        sheet.notices.push("a doua versiune".into());
        save(&dir, &sheet).unwrap();
        let backup = std::fs::read_to_string(dir.join("2026-09.json.bak")).unwrap();
        assert!(!backup.contains("a doua versiune"), "the .bak is the version before the last save");
        assert_eq!(list(&dir), vec![(2026, 10), (2026, 9)]);
        assert!(!dir.join("2026-09.json.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_months_beside_one_are_found_across_the_turn_of_the_year_and_a_broken_one_counts_as_absent() {
        let dir = temp_dir("around");
        save(&dir, &new_sheet(2026, 12)).unwrap();
        save(&dir, &new_sheet(2027, 2)).unwrap();
        std::fs::write(dir.join("2027-03.json"), "{ nu e json").unwrap();
        let (before, after) = load_around(&dir, 2027, 1);
        assert_eq!((before.map(|s| (s.year, s.month)), after.map(|s| (s.year, s.month))), (Some((2026, 12)), Some((2027, 2))), "January 2027 sits between December 2026 and February 2027");
        let (before, after) = load_around(&dir, 2027, 2);
        assert_eq!((before.is_none(), after.is_none()), (true, true), "no January saved, March is broken");
        let (before, after) = load_around(&dir, 2026, 11);
        assert_eq!((before.is_none(), after.map(|s| (s.year, s.month))), (true, Some((2026, 12))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_file_is_reported_and_left_alone() {
        let dir = temp_dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-10.json"), "{ nu e json").unwrap();
        let error = load(&dir, 2026, 10).unwrap_err().to_string();
        assert!(error.contains("nu poate fi citită"), "{error}");
        assert!(dir.join("2026-10.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
