//! Tauri shell: exposes the pontaj-core pipeline to the web UI.

use std::path::PathBuf;

use pontaj_core::{process_pdfs, recompute, suggested_file_name, write_xlsx, Settings, Timesheet};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

type CmdResult<T> = Result<T, String>;

fn settings_path(app: &AppHandle) -> CmdResult<PathBuf> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("setari.json"))
}

/// Runs blocking work (PDF parsing, dialogs, file IO) off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Settings {
    settings_path(&app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> CmdResult<()> {
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_pdfs(app: AppHandle) -> CmdResult<Vec<String>> {
    blocking(move || {
        let files = app.dialog().file().set_title("Alege rapoartele de tahograf").add_filter("PDF", &["pdf", "PDF"]).blocking_pick_files();
        Ok(files
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect())
    })
    .await
}

/// PDFs passed on the command line (`pontaj raport.pdf`), loaded at startup.
#[tauri::command]
fn startup_files() -> Vec<String> {
    std::env::args().skip(1).filter(|a| a.to_lowercase().ends_with(".pdf")).collect()
}

#[tauri::command]
async fn parse_pdfs(paths: Vec<String>, settings: Settings) -> CmdResult<Timesheet> {
    blocking(move || process_pdfs(&paths, &settings).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn recalculate(mut timesheet: Timesheet, settings: Settings) -> Timesheet {
    recompute(&mut timesheet, &settings);
    timesheet
}

/// Asks where to save, writes the workbook and returns the chosen path.
#[tauri::command]
async fn export_excel(app: AppHandle, timesheet: Timesheet, settings: Settings) -> CmdResult<Option<String>> {
    blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Salvează pontajul")
            .set_file_name(suggested_file_name(&timesheet, &settings))
            .add_filter("Excel", &["xlsx"]);
        if let Ok(dir) = app.path().download_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else {
            return Ok(None);
        };
        let path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsx")) { path } else { path.with_extension("xlsx") };
        write_xlsx(&timesheet, &settings, &path).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
}

/// Opens a file with the system's default application (Excel, Numbers...).
#[tauri::command]
fn open_file(path: String) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    // `explorer <file>` opens the default app and, unlike `cmd /C start`, is
    // not tripped up by `&` or spaces in the path.
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![load_settings, save_settings, pick_pdfs, startup_files, parse_pdfs, recalculate, export_excel, open_file])
        .run(tauri::generate_context!())
        .expect("eroare la pornirea aplicației");
}
