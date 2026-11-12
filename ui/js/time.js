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

/** Hours as shown everywhere: whole hours as "8", with minutes as "7:30" (never "7,5"). */
export function hoursText(hours) {
  const total = Math.round(hours * 60);
  const [h, m] = [Math.floor(total / 60), total % 60];
  return m === 0 ? String(h) : `${h}:${two(m)}`;
}

/**
 * Hours typed by hand, written the way people say them: "8", "7:30", "730", "7,30", "7 30" or "7h30"
 * all mean 7 h 30 min; one digit after the comma is a decimal ("7,5" = 7 h 30 min).
 * Empty → null, not a number of hours → undefined. The sheet rounds the result to the half hour.
 */
export function parseHours(text) {
  const value = String(text ?? "").trim();
  if (value === "") return null;
  const whole = value.match(/^(\d{1,2})$/);
  const tenth = value.match(/^(\d{1,2})[.,](\d)$/);
  const clock = value.match(/^(\d{1,2})\s*(?:[:.,hH]\s*|\s)(\d{2})$/) ?? value.match(/^(\d{1,2})(\d{2})$/);
  const hundredth = value.match(/^(\d{1,2})[.,](\d{2})$/);
  let hours;
  if (whole) hours = Number(whole[1]);
  else if (tenth) hours = Number(`${tenth[1]}.${tenth[2]}`);
  else if (clock && Number(clock[2]) < 60) hours = Number(clock[1]) + Number(clock[2]) / 60;
  else if (hundredth) hours = Number(`${hundredth[1]}.${hundredth[2]}`); // "7,75": too big for minutes
  return hours !== undefined && hours <= 24 ? hours : undefined;
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
