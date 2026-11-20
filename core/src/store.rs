//! Saving the months: one JSON file per month (`2026-10.json`) in a folder of the app. It serves every kind of month the app
//! keeps (the Categoria B sheets and the tachograph timesheets): each lives in a folder of its own.
//! The office's edits live only here and in the Excel files, so a write never destroys the previous version:
//! the file is written beside the old one and moved into place, and the old one is kept as `2026-10.json.bak`.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{Error, Result};

fn file(dir: &Path, year: i32, month: u32) -> PathBuf {
    dir.join(format!("{year}-{month:02}.json"))
}

/// Saves a month.
pub fn save<T: Serialize>(dir: &Path, year: i32, month: u32, value: &T) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::new(format!("Nu pot crea dosarul pentru salvare: {e}")))?;
    let json = serde_json::to_vec(value).map_err(|e| Error::new(format!("Nu pot pregăti luna pentru salvare: {e}")))?;
    let target = file(dir, year, month);
    let temp = target.with_extension("json.tmp");
    std::fs::write(&temp, json).map_err(|e| Error::new(format!("Nu pot salva luna: {e}")))?;
    if target.exists() {
        // Best effort: the backup is a convenience, the new file is what matters.
        let _ = std::fs::rename(&target, target.with_extension("json.bak"));
    }
    std::fs::rename(&temp, &target).map_err(|e| Error::new(format!("Nu pot salva luna: {e}")))
}

/// A saved month, if there is one. A file that cannot be read is an error (and stays where it is).
pub fn load<T: DeserializeOwned>(dir: &Path, year: i32, month: u32) -> Result<Option<T>> {
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

/// Takes a month out of the list. Its `.bak` (the version before the last save) stays beside it, so a mistake can be undone by hand.
pub fn remove(dir: &Path, year: i32, month: u32) -> Result<()> {
    match std::fs::remove_file(file(dir, year, month)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::new(format!("Nu pot șterge luna: {e}"))),
    }
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
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pontaj-months-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_removed_month_is_gone_but_its_backup_stays() {
        let dir = temp_dir("remove");
        save(&dir, 2026, 9, &vec![1]).unwrap();
        save(&dir, 2026, 9, &vec![1, 2]).unwrap();
        remove(&dir, 2026, 9).unwrap();
        assert_eq!(load::<Vec<i32>>(&dir, 2026, 9).unwrap(), None);
        assert!(list(&dir).is_empty());
        assert!(dir.join("2026-09.json.bak").exists(), "the version before the last save is kept");
        remove(&dir, 2026, 9).expect("removing a month that is not there is not an error");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn what_is_not_a_month_file_is_not_listed() {
        let dir = temp_dir("list");
        save(&dir, 2026, 10, &"a").unwrap();
        for name in ["2026-13.json", "notes.json", "2026-09.json.bak", "2026-08.json.tmp"] {
            std::fs::write(dir.join(name), "{}").unwrap();
        }
        assert_eq!(list(&dir), vec![(2026, 10)]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
