//! Puts the key of the ALT API into the program while it is built, so the people who use the app have nothing to fill in.
//! The key is never part of the source code (the repository is public): it comes from the build environment —
//! the secret `ALT_API_KEY` of GitHub Actions, or the variable `PONTAJ_ALT_API_KEY` — and, on a developer machine,
//! from the file `~/.config/pontaj/alt-api.env` (line `ALT_API_KEY=...`). Without any of them the app is built
//! without a key and asks for one in the settings.

use std::{env, fs, path::PathBuf};

const VARIABLE: &str = "PONTAJ_ALT_API_KEY";
const FILE_LINE: &str = "ALT_API_KEY=";

fn key_file() -> Option<PathBuf> {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config").join("pontaj").join("alt-api.env"))
}

fn clean(value: &str) -> String {
    value.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string()
}

fn main() {
    println!("cargo:rerun-if-env-changed={VARIABLE}");
    println!("cargo:rerun-if-changed=build.rs");

    let mut key = env::var(VARIABLE).map(|v| clean(&v)).unwrap_or_default();
    if key.is_empty() {
        if let Some(path) = key_file() {
            println!("cargo:rerun-if-changed={}", path.display());
            if let Ok(text) = fs::read_to_string(&path) {
                key = text.lines().find_map(|line| line.trim().strip_prefix(FILE_LINE)).map(clean).unwrap_or_default();
            }
        }
    }
    // A value with a line break would break the build instruction below; real keys never have one.
    if key.contains(['\n', '\r']) {
        key.clear();
    }
    println!("cargo:rustc-env=PONTAJ_BUILT_IN_ALT_KEY={key}");
}
