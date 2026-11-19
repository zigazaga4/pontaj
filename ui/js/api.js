// Thin wrappers around the Rust commands (src-tauri/src/lib.rs).
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

export const api = {
  loadSettings: () => invoke("load_settings"),
  saveSettings: (settings) => invoke("save_settings", { settings }),
  pickPdfs: () => invoke("pick_pdfs"),
  startupFiles: () => invoke("startup_files"),
  // `previous` (the sheet on screen) lets new PDFs be added without losing edits.
  parsePdfs: (paths, settings, previous) => invoke("parse_pdfs", { paths, settings, previous }),
  recalculate: (timesheet, settings) => invoke("recalculate", { timesheet, settings }),
  exportExcel: (timesheet, settings) => invoke("export_excel", { timesheet, settings }),
  openFile: (path) => invoke("open_file", { path }),
  // Categoria B: the months are saved on this computer, so reopening one needs no download.
  catbMonths: () => invoke("catb_months"),
  catbOpen: (year, month, settings) => invoke("catb_open", { year, month, settings }),
  /** Asks the API for the month (the network half; nothing is merged). Resolves with the rows for `catbMerge`. */
  catbDownload: (year, month, settings) => invoke("catb_download", { year, month, settings }),
  /** Puts the rows of `catbDownload` into `sheet` (the API's version replaces its data; the office's changes stay). */
  catbMerge: (sheet, parsed, year, month, settings) => invoke("catb_merge", { sheet, parsed, year, month, settings }),
  catbRecalculate: (sheet, settings) => invoke("catb_recalculate", { sheet, settings }),
  /** After an edit only that driver is sent and recomputed. */
  catbRecalculateDriver: (driver, year, month, settings) => invoke("catb_recalculate_driver", { driver, year, month, settings }),
  /** Moves the times of two overlapping trips so that they no longer overlap; resolves with { driver, summary }. */
  catbResolveOverlap: (driver, prev, next, year, month, settings) => invoke("catb_resolve_overlap", { driver, prev, next, year, month, settings }),
  /** Moves a drive made by the app by whole days (negative = earlier); resolves with the recomputed driver. */
  catbMoveTrip: (driver, trip, days, year, month, settings) => invoke("catb_move_trip", { driver, trip, days, year, month, settings }),
  catbSave: (sheet) => invoke("catb_save", { sheet }),
  catbExport: (sheet, settings) => invoke("catb_export", { sheet, settings }),
  appVersion: () => invoke("app_version"),
  /** { version, notes } of a newer release, or null. Rejects when GitHub cannot be reached. */
  checkUpdate: () => invoke("check_update"),
  /** Downloads and installs the newer release; the app restarts when it is done. */
  installUpdate: () => invoke("install_update"),
};

/** Calls `handler({ downloaded, total })` while an update downloads. */
export function onUpdateProgress(handler) {
  window.__TAURI__.event.listen("update-progress", (e) => handler(e.payload));
}

/** Subscribes to native file drag & drop. `handlers`: { enter, leave, drop(paths) }. */
export function onFileDrop(handlers) {
  const { listen } = window.__TAURI__.event;
  listen("tauri://drag-enter", () => handlers.enter?.());
  listen("tauri://drag-leave", () => handlers.leave?.());
  listen("tauri://drag-drop", (e) => handlers.drop?.(e.payload?.paths ?? []));
}
