//! Durations and dates in words, in the UI language: "на 1 неделю",
//! "через 2 ч 40 мин", "пт, 21 нояб., 15:00". Dates are shown in the
//! system time zone.

use jiff::tz::TimeZone;

use crate::config::Language;

/// The Russian plural form for `n`: 0 for 1, 21, …; 1 for 2–4, 22–24, …;
/// 2 for the rest (5–20, 0, …).
fn ru_form(n: u64) -> usize {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        0
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        1
    } else {
        2
    }
}

#[derive(Clone, Copy)]
enum Unit {
    Minute,
    Hour,
    Day,
    Week,
}

/// A ban's length, as the object of "for": "1 день", "1 неделю", "36 часов"
/// / "1 day", "1 week", "36 hours". The largest unit that divides it evenly
/// (weeks only below 4); otherwise minutes, rounded up.
pub fn duration(lang: Language, secs: u64) -> String {
    let (n, unit) = if secs >= 86_400 && secs.is_multiple_of(86_400) {
        let days = secs / 86_400;
        if days.is_multiple_of(7) && days < 28 { (days / 7, Unit::Week) } else { (days, Unit::Day) }
    } else if secs >= 3600 && secs.is_multiple_of(3600) {
        (secs / 3600, Unit::Hour)
    } else {
        (secs.div_ceil(60).max(1), Unit::Minute)
    };
    let word = match lang {
        // Accusative: "на 1 минуту", "на 1 неделю".
        Language::Ru => {
            let forms = match unit {
                Unit::Minute => ["минуту", "минуты", "минут"],
                Unit::Hour => ["час", "часа", "часов"],
                Unit::Day => ["день", "дня", "дней"],
                Unit::Week => ["неделю", "недели", "недель"],
            };
            forms[ru_form(n)]
        }
        Language::En => {
            let (one, many) = match unit {
                Unit::Minute => ("minute", "minutes"),
                Unit::Hour => ("hour", "hours"),
                Unit::Day => ("day", "days"),
                Unit::Week => ("week", "weeks"),
            };
            if n == 1 { one } else { many }
        }
    };
    format!("{n} {word}")
}

/// Time left, short: "5 дн.", "1 дн. 3 ч", "2 ч 40 мин", "40 мин" /
/// "5 d", "1 d 3 h", "2 h 40 min", "40 min". Rounded up to a minute.
pub fn left(lang: Language, secs: u64) -> String {
    let (d_, h_, m_) = match lang {
        Language::Ru => ("дн.", "ч", "мин"),
        Language::En => ("d", "h", "min"),
    };
    let minutes = secs.div_ceil(60).max(1);
    let (d, h, m) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    match (d, h, m) {
        (d, _, _) if d >= 3 => format!("{d} {d_}"),
        (d, 0, _) if d > 0 => format!("{d} {d_}"),
        (d, h, _) if d > 0 => format!("{d} {d_} {h} {h_}"),
        (0, h, 0) if h > 0 => format!("{h} {h_}"),
        (0, h, m) if h > 0 => format!("{h} {h_} {m} {m_}"),
        _ => format!("{m} {m_}"),
    }
}

fn zoned(ts: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    jiff::Timestamp::from_second(ts).ok().map(|t| t.to_zoned(tz.clone()))
}

/// "пт, 21 нояб., 15:00" / "Fri, 21 Nov, 15:00".
pub fn date_long(lang: Language, ts: i64) -> String {
    date_long_in(lang, ts, &TimeZone::system())
}

fn date_long_in(lang: Language, ts: i64, tz: &TimeZone) -> String {
    let Some(z) = zoned(ts, tz) else { return String::new() };
    let wd = z.weekday().to_monday_zero_offset() as usize;
    let month = (z.month() - 1) as usize;
    let time = z.strftime("%H:%M");
    match lang {
        Language::Ru => {
            const DAYS: [&str; 7] = ["пн", "вт", "ср", "чт", "пт", "сб", "вс"];
            const MONTHS: [&str; 12] =
                ["янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек."];
            format!("{}, {} {}, {time}", DAYS[wd], z.day(), MONTHS[month])
        }
        Language::En => {
            const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            const MONTHS: [&str; 12] =
                ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            format!("{}, {} {}, {time}", DAYS[wd], z.day(), MONTHS[month])
        }
    }
}

/// "21.11 15:00".
pub fn date_short(ts: i64) -> String {
    zoned(ts, &TimeZone::system()).map(|z| z.strftime("%d.%m %H:%M").to_string()).unwrap_or_default()
}

/// When something happened, compact: "20.11 14:00" within the last week,
/// "02.10" earlier this year, "02.10.2025" before that.
pub fn date_past(ts: i64, now: i64) -> String {
    date_past_in(ts, now, &TimeZone::system())
}

fn date_past_in(ts: i64, now: i64, tz: &TimeZone) -> String {
    let (Some(z), Some(n)) = (zoned(ts, tz), zoned(now, tz)) else { return String::new() };
    let format = if now - ts < 7 * 86_400 {
        "%d.%m %H:%M"
    } else if z.year() == n.year() {
        "%d.%m"
    } else {
        "%d.%m.%Y"
    };
    z.strftime(format).to_string()
}

/// "19.11.2026 09:12 (UTC+3)".
pub fn date_full(ts: i64) -> String {
    date_full_in(ts, &TimeZone::system())
}

fn date_full_in(ts: i64, tz: &TimeZone) -> String {
    let Some(z) = zoned(ts, tz) else { return String::new() };
    let secs = z.offset().seconds();
    let sign = if secs < 0 { '-' } else { '+' };
    let (h, m) = (secs.unsigned_abs() / 3600, secs.unsigned_abs() % 3600 / 60);
    let offset = if m == 0 { format!("{sign}{h}") } else { format!("{sign}{h}:{m:02}") };
    format!("{} (UTC{offset})", z.strftime("%d.%m.%Y %H:%M"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RU: Language = Language::Ru;
    const EN: Language = Language::En;

    #[test]
    fn durations_in_words() {
        assert_eq!(duration(RU, 3600), "1 час");
        assert_eq!(duration(RU, 86_400), "1 день");
        assert_eq!(duration(RU, 7 * 86_400), "1 неделю");
        assert_eq!(duration(RU, 30 * 86_400), "30 дней");
        assert_eq!(duration(RU, 3 * 86_400), "3 дня");
        assert_eq!(duration(RU, 21 * 86_400), "3 недели");
        assert_eq!(duration(RU, 36 * 3600), "36 часов");
        assert_eq!(duration(RU, 22 * 3600), "22 часа");
        assert_eq!(duration(RU, 1800), "30 минут");
        assert_eq!(duration(RU, 60), "1 минуту");
        assert_eq!(duration(RU, 11 * 86_400), "11 дней");
        assert_eq!(duration(RU, 21 * 3600), "21 час");
        assert_eq!(duration(EN, 3600), "1 hour");
        assert_eq!(duration(EN, 7 * 86_400), "1 week");
        assert_eq!(duration(EN, 30 * 86_400), "30 days");
        assert_eq!(duration(EN, 90), "2 minutes");
    }

    #[test]
    fn time_left() {
        assert_eq!(left(RU, 30), "1 мин");
        assert_eq!(left(RU, 40 * 60), "40 мин");
        assert_eq!(left(RU, 2 * 3600 + 40 * 60), "2 ч 40 мин");
        assert_eq!(left(RU, 2 * 3600), "2 ч");
        assert_eq!(left(RU, 86_400 - 60), "23 ч 59 мин");
        assert_eq!(left(RU, 86_400 + 3 * 3600), "1 дн. 3 ч");
        assert_eq!(left(RU, 5 * 86_400 + 3600), "5 дн.");
        assert_eq!(left(EN, 2 * 3600 + 40 * 60), "2 h 40 min");
        assert_eq!(left(EN, 5 * 86_400), "5 d");
    }

    #[test]
    fn dates() {
        let msk = TimeZone::fixed(jiff::tz::offset(3));
        // 2026-11-21 12:00 UTC = 15:00 in Moscow, a Saturday.
        let ts =
            jiff::civil::date(2026, 11, 21).at(12, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap().timestamp().as_second();
        assert_eq!(date_long_in(RU, ts, &msk), "сб, 21 нояб., 15:00");
        assert_eq!(date_long_in(EN, ts, &msk), "Sat, 21 Nov, 15:00");
        assert_eq!(date_long_in(EN, ts, &TimeZone::UTC), "Sat, 21 Nov, 12:00");
        assert_eq!(date_full_in(ts, &msk), "21.11.2026 15:00 (UTC+3)");
        assert_eq!(
            date_full_in(ts, &TimeZone::fixed(jiff::tz::Offset::from_seconds(-(3 * 3600 + 1800)).unwrap())),
            "21.11.2026 08:30 (UTC-3:30)"
        );
        assert_eq!(date_past_in(ts - 3600, ts, &msk), "21.11 14:00");
        assert_eq!(date_past_in(ts - 30 * 86_400, ts, &msk), "22.10");
        assert_eq!(date_past_in(ts - 400 * 86_400, ts, &msk), "17.10.2025");
    }
}
