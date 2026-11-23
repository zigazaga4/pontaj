// Keeps the row somebody is working on where it was on screen when its view is drawn again. A row is any element with
// `data-row="<id>"` (the id must not change from one drawing to the next). When the edit moves the row inside the list (a time
// that changes the order, a trip moved to another day) the rest of the list moves around it, and a row that ended up out of
// sight is brought back into view.

const findRow = (view, id) => (id ? view.querySelector(`[data-row="${CSS.escape(id)}"]`) : null);

/** The id of the row an element belongs to (the nearest `data-row` above it), or null. */
export const rowIdOf = (el) => el?.closest?.("[data-row]")?.dataset.row ?? null;

/**
 * Call before the view is replaced; call the returned function right after. `covered` = height of what stays pinned on top of the
 * scroll area (a sticky bar), so a row is never left hidden under it. The returned function gives back the row (or null) and
 * says whether the view had to scroll to show it (`jumped`).
 */
export function holdRow(view, id, covered = () => 0) {
  const before = findRow(view, id);
  const offset = before ? before.getBoundingClientRect().top - view.getBoundingClientRect().top : null;
  const scroll = view.scrollTop;
  return function restore() {
    const row = findRow(view, id);
    if (!row || offset === null) {
      view.scrollTop = scroll;
      return { row, jumped: false };
    }
    view.scrollTop += row.getBoundingClientRect().top - view.getBoundingClientRect().top - offset;
    const area = view.getBoundingClientRect();
    const box = row.getBoundingClientRect();
    const top = area.top + covered();
    // A row taller than what is left of the view cannot be shown whole: leave it where the anchor put it.
    if (box.height > area.bottom - top) return { row, jumped: false };
    if (box.top >= top && box.bottom <= area.bottom) return { row, jumped: false };
    view.scrollTop += box.top + box.height / 2 - (top + (area.bottom - top) / 2);
    return { row, jumped: true };
  };
}
