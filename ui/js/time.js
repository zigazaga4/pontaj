// Clock/duration formatting and parsing shared by every view. Times are minutes
// from midnight; values may be negative (evening before) or above 1440 (next day).

export const DAY = 1440;

const two = (n) => String(n).padStart(2, "0");

/** "HH:MM", wrapping around midnight. */
export function hhmm(min) {
  const m = ((min % DAY) + DAY) % DAY;
  return `${two(Math.floor(m / 60))}:${two(m % 60)}`;
}

/** Clock time with a note when it falls outside the day ("23:30 (ziua ant.)", "02:00 (+1)"). */
export function clock(min) {
  if (min < 0) return `${hhmm(min)} (ziua ant.)`;
  if (min >= DAY) return `${hhmm(min)} (+1)`;
  return hhmm(min);
}

/** "H:MM" for a duration in minutes. */
export const duration = (min) => `${Math.floor(Math.abs(min) / 60)}:${two(Math.abs(min) % 60)}`;

/** "+0:33" / "−1:05" / "0:00". */
export function signed(min) {
  if (min === 0) return "0:00";
  return `${min > 0 ? "+" : "−"}${duration(min)}`;
}

/** Parses "7:39", "07:39", "7.39", "739" or "0730". Empty → null, invalid → NaN. */
export function parseHm(text) {
  const value = String(text ?? "").trim();
  if (value === "") return null;
  const match = value.match(/^(\d{1,2})[:.](\d{2})$/) ?? value.match(/^(\d{1,2})(\d{2})$/);
  if (!match) return NaN;
  const [h, m] = [Number(match[1]), Number(match[2])];
  return m < 60 && h < 48 ? h * 60 + m : NaN;
}
