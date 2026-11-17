//! Saving the months: one JSON file per month (`2026-10.json`) in a folder of the app.
//! The office's edits live only here and in the Excel files, so a write never destroys the previous version:
//! the file is written beside the old one and moved into place, and the old one is kept as `2026-10.json.bak`.

use std::path::{Path, PathBuf};

use super::model::Sheet;
use crate::{Error, Result};

fn file(dir: &Path, year: i32, month: u32) -> PathBuf {
    dir.join(format!("{year}-{month:02}.json"))
}

/// Saves a month.
pub fn save(dir: &Path, sheet: &Sheet) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::new(format!("Nu pot crea dosarul pentru salvare: {e}")))?;
    let json = serde_json::to_vec(sheet).map_err(|e| Error::new(format!("Nu pot pregăti luna pentru salvare: {e}")))?;
    let target = file(dir, sheet.year, sheet.month);
    let temp = target.with_extension("json.tmp");
    std::fs::write(&temp, json).map_err(|e| Error::new(format!("Nu pot salva luna: {e}")))?;
    if target.exists() {
        // Best effort: the backup is a convenience, the new file is what matters.
        let _ = std::fs::rename(&target, target.with_extension("json.bak"));
    }
    std::fs::rename(&temp, &target).map_err(|e| Error::new(format!("Nu pot salva luna: {e}")))
}

/// A saved month, if there is one. A file that cannot be read is an error (and stays where it is).
pub fn load(dir: &Path, year: i32, month: u32) -> Result<Option<Sheet>> {
    let path = file(dir, year, month);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::new(format!("Nu pot citi luna salvată: {e}"))),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| Error::new(format!("Luna salvată {year}-{month:02} nu poate fi citită ({e}). Copia de siguranță este lângă ea, cu extensia .bak.")))
}

/// The saved months, newest first.
pub fn list(dir: &Path) -> Vec<(i32, u32)> {
    let mut months: Vec<(i32, u32)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (year, month) = name.strip_suffix(".json")?.split_once('-')?;
            let (year, month): (i32, u32) = (year.parse().ok()?, month.parse().ok()?);
            (1..=12).contains(&month).then_some((year, month))
        })
        .collect();
    months.sort_unstable_by(|a, b| b.cmp(a));
    months
}

#[cfg(test)]
mod tests {
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
