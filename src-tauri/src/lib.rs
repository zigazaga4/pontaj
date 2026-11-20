//! Tauri shell: exposes the pontaj-core pipeline to the web UI.

use std::path::PathBuf;

use pontaj_core::{catb, history, recompute, suggested_file_name, write_xlsx, Settings, Timesheet};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

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
        .and_then(|json| serde_json::from_str::<Settings>(&json).ok())
        .map_or_else(Settings::default, Settings::migrated)
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

fn history_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("pdf"))
}

/// Reads PDFs; with `previous` they are added to the existing timesheet. PDFs of a month that was read before are added to
/// that month, and the result is kept (see `history`), so the month is there the next time the app opens.
#[tauri::command]
async fn parse_pdfs(app: AppHandle, paths: Vec<String>, settings: Settings, previous: Option<Timesheet>) -> CmdResult<Timesheet> {
    blocking(move || history::add_pdfs(&history_dir(&app)?, previous, &paths, &settings).map_err(|e| e.to_string())).await
}

/// The months read from PDFs and kept on this computer, newest first.
#[tauri::command]
async fn pdf_months(app: AppHandle) -> CmdResult<Vec<history::SavedMonth>> {
    blocking(move || Ok(history::list(&history_dir(&app)?))).await
}

/// A kept month (calculated again with the current settings), or nothing when it was never read.
#[tauri::command]
async fn pdf_open(app: AppHandle, year: i32, month: u32, settings: Settings) -> CmdResult<Option<Timesheet>> {
    blocking(move || history::open(&history_dir(&app)?, year, month, &settings).map_err(|e| e.to_string())).await
}

/// Keeps the timesheet on screen, with the edits made so far.
#[tauri::command]
async fn pdf_save(app: AppHandle, timesheet: Timesheet) -> CmdResult<()> {
    blocking(move || history::save(&history_dir(&app)?, &timesheet).map_err(|e| e.to_string())).await
}

/// Forgets a kept month.
#[tauri::command]
async fn pdf_delete(app: AppHandle, year: i32, month: u32) -> CmdResult<()> {
    blocking(move || history::remove(&history_dir(&app)?, year, month).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn recalculate(mut timesheet: Timesheet, settings: Settings) -> Timesheet {
    recompute(&mut timesheet, &settings);
    timesheet
}

/// Asks where to save an Excel file (Downloads by default). `None` when the person cancels.
fn ask_xlsx_path(app: &AppHandle, file_name: String) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().set_title("Salvează pontajul").set_file_name(file_name).add_filter("Excel", &["xlsx"]);
    if let Ok(dir) = app.path().download_dir() {
        dialog = dialog.set_directory(dir);
    }
    let path = dialog.blocking_save_file().and_then(|p| p.into_path().ok())?;
    Some(if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsx")) { path } else { path.with_extension("xlsx") })
}

/// Asks where to save, writes the workbook and returns the chosen path.
#[tauri::command]
async fn export_excel(app: AppHandle, timesheet: Timesheet, settings: Settings) -> CmdResult<Option<String>> {
    blocking(move || {
        let Some(path) = ask_xlsx_path(&app, suggested_file_name(&timesheet, &settings)) else { return Ok(None) };
        write_xlsx(&timesheet, &settings, &path).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
}

// ---- Categoria B: drivers' trips from the ALT API ----

fn catb_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("catb"))
}

/// The months saved on this computer, newest first.
#[tauri::command]
fn catb_months(app: AppHandle) -> CmdResult<Vec<(i32, u32)>> {
    Ok(catb::store::list(&catb_dir(&app)?))
}

/// A saved month (recomputed with the current settings), or nothing when it was never downloaded.
#[tauri::command]
async fn catb_open(app: AppHandle, year: i32, month: u32, settings: Settings) -> CmdResult<Option<catb::Sheet>> {
    blocking(move || {
        let dir = catb_dir(&app)?;
        let mut sheet = catb::store::load(&dir, year, month).map_err(|e| e.to_string())?;
        if let Some(sheet) = &mut sheet {
            // A month downloaded before the days around it were kept still gets the drives between months: the months
            // saved beside it have exactly those trips. The drivers taken out of the pontaj stay out of every month.
            let (previous, next) = catb::store::load_around(&dir, year, month);
            catb::open_saved(sheet, previous, next, &settings);
        }
        Ok(sheet)
    })
    .await
}

/// The network half of a download: asks the API for the month and gives the rows back, merging nothing. The screen merges
/// them (`catb_merge`) into the sheet as it is at that moment, so a quiet refresh never overwrites an edit made meanwhile.
#[tauri::command]
async fn catb_download(year: i32, month: u32, settings: Settings) -> CmdResult<catb::api::Parsed> {
    blocking(move || catb::api::fetch_month(&settings.catb, year, month).map_err(|e| e.to_string())).await
}

/// The other half: puts rows from `catb_download` into the open sheet (nothing is saved here; the screen saves).
#[tauri::command]
async fn catb_merge(sheet: Option<catb::Sheet>, parsed: catb::api::Parsed, year: i32, month: u32, settings: Settings) -> CmdResult<catb::Sheet> {
    blocking(move || Ok(catb::merge_download(sheet, year, month, &parsed, &settings))).await
}

#[tauri::command]
async fn catb_recalculate(mut sheet: catb::Sheet, settings: Settings) -> catb::Sheet {
    catb::recompute(&mut sheet, &settings);
    sheet
}

/// After an edit of one driver only that driver is recomputed (the whole month would be a lot to send each time).
#[tauri::command]
fn catb_recalculate_driver(mut driver: catb::Driver, year: i32, month: u32, settings: Settings) -> catb::Driver {
    catb::recompute_driver(&mut driver, year, month, &settings);
    driver
}

/// The "Rezolvă suprapunerea" button: moves the times of the two overlapping trips and returns the driver with a sentence about it.
#[tauri::command]
fn catb_resolve_overlap(driver: catb::Driver, prev: String, next: String, year: i32, month: u32, settings: Settings) -> CmdResult<catb::Resolved> {
    catb::resolve_overlap(driver, &prev, &next, year, month, &settings).map_err(|e| e.to_string())
}

/// The move buttons of a drive made by the app: shifts it by whole days (negative = earlier) and returns the recomputed driver.
#[tauri::command]
fn catb_move_trip(driver: catb::Driver, trip: String, days: i32, year: i32, month: u32, settings: Settings) -> CmdResult<catb::Driver> {
    catb::move_transfer(driver, &trip, days, year, month, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn catb_save(app: AppHandle, sheet: catb::Sheet) -> CmdResult<()> {
    blocking(move || catb::store::save(&catb_dir(&app)?, &sheet).map_err(|e| e.to_string())).await
}

/// Asks where to save, writes the Categoria B workbook and returns the chosen path.
#[tauri::command]
async fn catb_export(app: AppHandle, sheet: catb::Sheet, settings: Settings) -> CmdResult<Option<String>> {
    blocking(move || {
        let Some(path) = ask_xlsx_path(&app, catb::suggested_file_name(&sheet)) else { return Ok(None) };
        catb::excel::write_xlsx(&sheet, &settings, &path).map_err(|e| e.to_string())?;
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

// ---- Updates: the app looks for a newer release on GitHub (see `plugins.updater` in tauri.conf.json) ----

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// The newer release, if there is one. No network or no published release comes back as an error string.
#[tauri::command]
async fn check_update(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| UpdateInfo { version: u.version, notes: u.body }))
}

/// Downloads the newer release (emitting `update-progress`), installs it and restarts the app.
#[tauri::command]
async fn install_update(app: AppHandle) -> CmdResult<()> {
    let Some(update) = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let mut downloaded: u64 = 0;
    let progress = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress.emit("update-progress", UpdateProgress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            pick_pdfs,
            startup_files,
            parse_pdfs,
            pdf_months,
            pdf_open,
            pdf_save,
            pdf_delete,
            recalculate,
            export_excel,
            catb_months,
            catb_open,
            catb_download,
            catb_merge,
            catb_recalculate,
            catb_recalculate_driver,
            catb_resolve_overlap,
            catb_move_trip,
            catb_save,
            catb_export,
            open_file,
            app_version,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("eroare la pornirea aplicației");
}
