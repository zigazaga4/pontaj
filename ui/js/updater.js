// Automatic updates: finds a newer release on GitHub, then downloads and installs it.
import { api, onUpdateProgress } from "./api.js";
import { busy, toast } from "./dom.js";

const percent = ({ downloaded, total }) => (total ? ` ${Math.min(100, Math.round((downloaded / total) * 100))}%` : "");

let listening = false;
let current = "";

/** Downloads and installs `update`; the app restarts by itself when it is done. */
export async function installUpdate(update) {
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
 * - `canInstallNow()`: true when nothing would be lost by restarting (no sheet open). The update is then
 *   installed without asking; otherwise a message offers it.
 * - `manual`: the user asked, so "no update" and failures are reported too, and nothing is installed unasked.
 */
export async function checkForUpdates({ manual = false, canInstallNow = () => false } = {}) {
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
  if (!manual && canInstallNow()) return installUpdate(update);
  const warning = canInstallNow() ? "" : " Pontajul deschis nu se păstrează la repornire: exportă mai întâi Excel-ul.";
  toast(`Versiune nouă disponibilă: ${update.version}.${warning}`, {
    kind: "info",
    timeout: 0,
    action: { label: "Actualizează acum", run: () => installUpdate(update) },
  });
}
