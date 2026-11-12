//! Helpers for working with clock times expressed as minutes.
//!
//! Minutes are always relative to the midnight of a reference day, so a value
//! can be negative (the evening before) or above 1440 (after midnight).

pub const DAY: i32 = 24 * 60;

/// Parses `"7:39"`, `"07:39"` or `"170:35"` into minutes.
pub fn parse_hm(s: &str) -> Option<i32> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || m.len() != 2 || !h.bytes().all(|b| b.is_ascii_digit()) || !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    (m < 60).then_some(h * 60 + m)
}

/// Formats a duration as `H:MM` (e.g. `6:25`).
pub fn fmt_duration(min: i32) -> String {
    let sign = if min < 0 { "-" } else { "" };
    let m = min.abs();
    format!("{sign}{}:{:02}", m / 60, m % 60)
}

/// Formats a clock time as `HH:MM`, wrapping around midnight.
pub fn fmt_clock(min: i32) -> String {
    let m = min.rem_euclid(DAY);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Rounds a duration to whole hours: the leftover minutes round up only when
/// they reach `round_up_from` (e.g. 20 → 6:19 = 6h, 6:20 = 7h).
pub fn round_hours(min: i32, round_up_from: i32) -> i32 {
    let min = min.max(0);
    let (h, rest) = (min / 60, min % 60);
    if rest > 0 && rest >= round_up_from { h + 1 } else { h }
}

/// Rounds a clock time to the nearest multiple of `slot` minutes.
pub fn round_to_slot(min: i32, slot: i32) -> i32 {
    let slot = slot.max(1);
    (min + slot / 2).div_euclid(slot) * slot
}

/// Rounds a duration down to a multiple of `slot` minutes.
pub fn floor_to_slot(min: i32, slot: i32) -> i32 {
    let slot = slot.max(1);
    min.div_euclid(slot) * slot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_hm("7:39"), Some(459));
        assert_eq!(parse_hm("170:35"), Some(10235));
        assert_eq!(parse_hm("0:00"), Some(0));
        assert_eq!(parse_hm("7:3"), None);
        assert_eq!(parse_hm("Zile"), None);
    }

    #[test]
    fn rounds_like_the_payroll_team() {
        // Pairs observed in the September 2026 timesheet.
        for (pdf, xl) in [("6:12", 6), ("6:25", 7), ("5:21", 6), ("3:23", 4), ("8:13", 8), ("7:16", 7), ("5:59", 6), ("2:12", 2)] {
            assert_eq!(round_hours(parse_hm(pdf).unwrap(), 20), xl, "{pdf}");
        }
    }

    #[test]
    fn rounds_to_slots() {
        assert_eq!(round_to_slot(parse_hm("7:39").unwrap(), 30), parse_hm("7:30").unwrap());
        assert_eq!(round_to_slot(parse_hm("16:13").unwrap(), 30), parse_hm("16:00").unwrap());
        assert_eq!(round_to_slot(parse_hm("4:16").unwrap(), 30), parse_hm("4:30").unwrap());
        assert_eq!(round_to_slot(-55, 30), -60);
        assert_eq!(fmt_clock(1470), "00:30");
    }
}
