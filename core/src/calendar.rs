//! Romanian working calendar: legal holidays, working days, labels.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// Orthodox Easter Sunday (Julian computus converted to the Gregorian calendar,
/// valid for 1900–2099).
pub fn orthodox_easter(year: i32) -> NaiveDate {
    let (a, b, c) = (year % 4, year % 7, year % 19);
    let d = (19 * c + 15) % 30;
    let e = (2 * a + 4 * b - d + 34) % 7;
    let month = (d + e + 114) / 31;
    let day = (d + e + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid Julian Easter") + Duration::days(13)
}

/// Legal non-working days in Romania (Codul muncii, art. 139).
pub fn legal_holidays(year: i32) -> Vec<(NaiveDate, &'static str)> {
    let d = |m, day| NaiveDate::from_ymd_opt(year, m, day).expect("valid date");
    let easter = orthodox_easter(year);
    let mut list = vec![
        (d(1, 1), "Anul Nou"),
        (d(1, 2), "Anul Nou"),
        (d(1, 6), "Boboteaza"),
        (d(1, 7), "Sf. Ioan Botezătorul"),
        (d(1, 24), "Unirea Principatelor"),
        (easter - Duration::days(2), "Vinerea Mare"),
        (easter, "Paștele"),
        (easter + Duration::days(1), "Paștele"),
        (d(5, 1), "Ziua Muncii"),
        (d(6, 1), "Ziua Copilului"),
        (easter + Duration::days(49), "Rusaliile"),
        (easter + Duration::days(50), "Rusaliile"),
        (d(8, 15), "Adormirea Maicii Domnului"),
        (d(11, 30), "Sf. Andrei"),
        (d(12, 1), "Ziua Națională"),
        (d(12, 25), "Crăciunul"),
        (d(12, 26), "Crăciunul"),
    ];
    list.sort_by_key(|(date, _)| *date);
    list
}

pub fn holiday_name(date: NaiveDate) -> Option<&'static str> {
    legal_holidays(date.year()).into_iter().find(|(d, _)| *d == date).map(|(_, name)| name)
}

pub fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let next = first.checked_add_months(chrono::Months::new(1)).expect("valid next month");
    (next - first).num_days() as u32
}

/// Mon–Fri days that are not legal holidays.
pub fn working_days(year: i32, month: u32) -> u32 {
    let holidays = legal_holidays(year);
    (1..=days_in_month(year, month))
        .filter_map(|d| NaiveDate::from_ymd_opt(year, month, d))
        .filter(|date| !is_weekend(*date) && !holidays.iter().any(|(h, _)| h == date))
        .count() as u32
}

pub fn weekday_short(date: NaiveDate) -> &'static str {
    match date.weekday() {
        Weekday::Mon => "Lu",
        Weekday::Tue => "Ma",
        Weekday::Wed => "Mi",
        Weekday::Thu => "Jo",
        Weekday::Fri => "Vi",
        Weekday::Sat => "Sâ",
        Weekday::Sun => "Du",
    }
}

pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "IANUARIE", "FEBRUARIE", "MARTIE", "APRILIE", "MAI", "IUNIE",
        "IULIE", "AUGUST", "SEPTEMBRIE", "OCTOMBRIE", "NOIEMBRIE", "DECEMBRIE",
    ];
    NAMES.get(month.saturating_sub(1) as usize).copied().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_dates() {
        assert_eq!(orthodox_easter(2026), NaiveDate::from_ymd_opt(2026, 4, 12).unwrap());
        assert_eq!(orthodox_easter(2025), NaiveDate::from_ymd_opt(2025, 4, 20).unwrap());
        assert_eq!(orthodox_easter(2024), NaiveDate::from_ymd_opt(2024, 5, 5).unwrap());
    }

    #[test]
    fn monthly_norm_matches_existing_timesheets() {
        assert_eq!(working_days(2026, 8) * 8, 168);
        assert_eq!(working_days(2026, 9) * 8, 176);
        assert_eq!(days_in_month(2026, 2), 28);
    }
}
