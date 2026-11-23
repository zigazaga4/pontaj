//! Saving a month as one JSON file (`2026-10.json`) in a folder of the app. Used by Categoria B and by the office, garage and
//! city staff: the edits of the office live only here and in the Excel files, so a write never destroys the previous version.
//! The file is written beside the old one and moved into place, and the old one is kept as `2026-10.json.bak`.

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
