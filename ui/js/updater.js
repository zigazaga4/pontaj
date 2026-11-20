// Automatic updates: finds a newer release on GitHub, then downloads and installs it.
import { api, onUpdateProgress } from "./api.js";
import { $, busy, toast } from "./dom.js";

const percent = ({ downloaded, total }) => (total ? ` ${Math.min(100, Math.round((downloaded / total) * 100))}%` : "");

/** How often a running app looks for a newer version (an app left open for days must not stay on an old one). */
const CHECK_EVERY_MS = 30 * 60 * 1000;

let listening = false;
let current = "";
let announced = ""; // the version the message was already shown for

/** Downloads and installs `update`; the app restarts by itself when it is done. `beforeInstall` saves what is pending
 * and may answer `false` (the user changed his mind). */
export async function installUpdate(update, beforeInstall) {
  let go = true;
  try {
    go = (await beforeInstall?.()) !== false;
  } catch {
    // A failed save reports itself; it must not keep the update from installing.
  }
  if (!go) return;
  current = `Se descarcă versiunea ${update.version}…`;
  busy(current);
  if (!listening) {
    listening = true;
    onUpdateProgress((progress) => busy(`${current}${percent(progress)}`));
  }
  try {
    await api.installUpdate();
    current = "Se instalează versiunea nouă și se repornește aplicația…";
    busy(current);
  } catch (err) {
    busy(null);
    toast(`Actualizarea nu a reușit: ${err}`, { kind: "error", timeout: 0 });
  }
}

/**
 * Looks for an update and acts on it.
 * - `canInstallNow()`: true when a restart would not get in the way (no sheet open, nothing loading). The update is then
 *   installed without asking; otherwise a message offers it. What is open is saved by `beforeInstall` either way.
 * - `manual`: the user asked, so "no update" and failures are reported too, and nothing is installed unasked.
 */
export async function checkForUpdates({ manual = false, canInstallNow = () => false, beforeInstall } = {}) {
  let update;
  try {
    update = await api.checkUpdate();
  } catch (err) {
    // Offline or no release yet: say so only when the user asked.
    if (manual) toast(`Nu am putut verifica actualizările: ${err}`, { kind: "warn" });
    return;
  }
  if (!update) {
    if (manual) toast("Ai deja cea mai nouă versiune.", { kind: "ok" });
    return;
  }
  if (!manual && canInstallNow()) return installUpdate(update, beforeInstall);
  // A button stays in the top bar until the update is installed: a message that disappears is easy to miss.
  const ready = $("#btn-update-ready");
  ready.textContent = `⬆ Actualizare ${update.version}`;
  ready.onclick = () => installUpdate(update, beforeInstall);
  ready.hidden = false;
  if (!manual && announced === update.version) return;
  announced = update.version;
  toast(`Versiune nouă disponibilă: ${update.version}. Ce ai lucrat se salvează singur și rămâne după repornire.`, {
    kind: "info",
    timeout: 0,
    action: { label: "Actualizează acum", run: () => installUpdate(update, beforeInstall) },
  });
}

/** Looks again every half hour, and when the window comes back after a while; silent unless there is something new. */
export function watchForUpdates(options) {
  let last = Date.now();
  const check = () => {
    last = Date.now();
    return checkForUpdates(options);
  };
  setInterval(check, CHECK_EVERY_MS);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && Date.now() - last > CHECK_EVERY_MS / 3) check();
  });
}
