// Thin wrappers around the Rust commands (src-tauri/src/lib.rs).
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

export const api = {
  loadSettings: () => invoke("load_settings"),
  saveSettings: (settings) => invoke("save_settings", { settings }),
  pickPdfs: () => invoke("pick_pdfs"),
  startupFiles: () => invoke("startup_files"),
  parsePdfs: (paths, settings) => invoke("parse_pdfs", { paths, settings }),
  recalculate: (timesheet, settings) => invoke("recalculate", { timesheet, settings }),
  exportExcel: (timesheet, settings) => invoke("export_excel", { timesheet, settings }),
  openFile: (path) => invoke("open_file", { path }),
};

/** Subscribes to native file drag & drop. `handlers`: { enter, leave, drop(paths) }. */
export function onFileDrop(handlers) {
  const { listen } = window.__TAURI__.event;
  listen("tauri://drag-enter", () => handlers.enter?.());
  listen("tauri://drag-leave", () => handlers.leave?.());
  listen("tauri://drag-drop", (e) => handlers.drop?.(e.payload?.paths ?? []));
}
