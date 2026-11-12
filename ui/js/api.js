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
