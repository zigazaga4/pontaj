// Saving by itself: a change is written a moment after the last one (a burst of edits is one write), and `flush` writes
// at once what is waiting (before another month opens, before the window closes, before an update restarts the app).

/**
 * @param {() => Promise<void>} save    writes the current state
 * @param {{ delay?: number, onError?: (err: unknown) => void }} [options]
 * @returns {{ markDirty: () => void, flush: () => Promise<void> }}
 */
export function createAutosave(save, { delay = 1200, onError } = {}) {
  let dirty = false;
  let timer = null;

  async function flush() {
    clearTimeout(timer);
    if (!dirty) return;
    dirty = false;
    try {
      await save();
    } catch (err) {
      dirty = true; // the next change (or flush) tries again
      onError?.(err);
    }
  }

  function markDirty() {
    dirty = true;
    clearTimeout(timer);
    timer = setTimeout(flush, delay);
  }

  return { markDirty, flush };
}
