//! 日期换算：IMAP 搜索日期、RFC 5322 邮件时间与 ISO-8601 输出。
//!
//! 只做纯计算，不碰数据库与网络；供同步引擎把服务器发来的时间统一成 UTC。
//! 日历算法来自 Howard Hinnant 的 `civil_from_days` / `days_from_civil`（公有领域）。

/// 英文月份缩写，IMAP 与 RFC 5322 都用它。
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// 把「距今多少天」换算成 IMAP 搜索用的日期（如 `04-Oct-2026`）。
///
/// 以 UTC 为准，只用于 `UID SEARCH SINCE` 这类查询。
pub fn format_imap_date(days_ago: u32) -> String {
    let now = unix_now();
    let days = now.div_euclid(86_400) - i64::from(days_ago);
    let (year, month, day) = civil_from_days(days);
    format!("{day:02}-{}-{year:04}", MONTHS[(month - 1) as usize])
}

/// 当前 Unix 时间戳（秒）。
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// 时间戳（秒）转 UTC 的 ISO-8601（如 `2026-10-04T01:02:03Z`）。
pub fn format_iso8601_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// 解析 RFC 5322 邮件时间（也兼容 IMAP 的 INTERNALDATE），转成 UTC 的 ISO-8601。
///
/// 认不出来时返回 `None`，由调用方决定回落方案；不认识的时区名不会瞎猜。
pub fn parse_mail_date(raw: &str) -> Option<String> {
    let cleaned = strip_comments(raw);
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    let mut index = usize::from(matches!(tokens.first(), Some(token) if is_weekday(token)));
    let (year, month, day) = match parse_dashed_date(tokens.get(index)?) {
        Some(parsed) => {
            index += 1;
            parsed
        }
        None => {
            let day: u32 = tokens.get(index)?.trim_end_matches(',').parse().ok()?;
            let month = month_number(tokens.get(index + 1)?)?;
            let year = parse_year(tokens.get(index + 2)?)?;
            index += 3;
            (year, month, day)
        }
    };
    let (hour, minute, second) = parse_time(tokens.get(index)?)?;
    let offset = parse_zone(tokens.get(index + 1).copied().unwrap_or("+0000"))?;
    if day == 0 || day > 31 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let local = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    Some(format_iso8601_utc(local - offset))
}

/// 公历日期转「距 1970-01-01 的天数」。
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// 「距 1970-01-01 的天数」转公历日期，返回 `(年, 月, 日)`。
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn strip_comments(input: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

fn is_weekday(token: &str) -> bool {
    let head = token.trim_end_matches(',').get(..3).unwrap_or("");
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .iter()
        .any(|name| head.eq_ignore_ascii_case(name))
}

fn parse_dashed_date(token: &str) -> Option<(i64, u32, u32)> {
    let mut parts = token.split('-');
    let day: u32 = parts.next()?.parse().ok()?;
    let month = month_number(parts.next()?)?;
    let year = parse_year(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

fn month_number(token: &str) -> Option<u32> {
    let head = token.get(..3)?;
    MONTHS
        .iter()
        .position(|name| head.eq_ignore_ascii_case(name))
        .map(|index| index as u32 + 1)
}

fn parse_year(token: &str) -> Option<i64> {
    let value: i64 = token.parse().ok()?;
    match value {
        0..=49 => Some(value + 2000),
        50..=99 => Some(value + 1900),
        100..=9999 => Some(value),
        _ => None,
    }
}

fn parse_time(token: &str) -> Option<(u32, u32, u32)> {
    let mut parts = token.split(':');
    let hour = parts.next()?.parse().ok()?;
    let minute = parts.next()?.parse().ok()?;
    let second = match parts.next() {
        Some(value) => value.parse().ok()?,
        None => 0,
    };
    Some((hour, minute, second))
}

fn parse_zone(token: &str) -> Option<i64> {
    let token = token.trim();
    if let Some(rest) = token.strip_prefix('+').or_else(|| token.strip_prefix('-')) {
        if rest.len() != 4 || !rest.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        let hours: i64 = rest[..2].parse().ok()?;
        let minutes: i64 = rest[2..].parse().ok()?;
        let total = hours * 3600 + minutes * 60;
        return Some(if token.starts_with('-') { -total } else { total });
    }
    match token.to_ascii_uppercase().as_str() {
        "UT" | "UTC" | "GMT" | "Z" => Some(0),
        "EST" => Some(-5 * 3600),
        "EDT" => Some(-4 * 3600),
        "CST" => Some(-6 * 3600),
        "CDT" => Some(-5 * 3600),
        "MST" => Some(-7 * 3600),
        "MDT" => Some(-6 * 3600),
        "PST" => Some(-8 * 3600),
        "PDT" => Some(-7 * 3600),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil, format_imap_date, format_iso8601_utc, parse_mail_date};

    #[test]
    fn 天数与公历能往返() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 10, 4) - days_from_civil(2026, 10, 3), 1);
        assert_eq!(civil_from_days(days_from_civil(2026, 10, 4)), (2026, 10, 4));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
    }

    #[test]
    fn 时间戳格式化为utc() {
        assert_eq!(format_iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_iso8601_utc(days_from_civil(2025, 10, 4) * 86_400),
            "2025-10-04T00:00:00Z"
        );
        assert_eq!(format_iso8601_utc(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn 邮件时间换算到utc() {
        assert_eq!(
            parse_mail_date("Wed, 01 Oct 2026 10:20:30 +0800").as_deref(),
            Some("2026-10-01T02:20:30Z")
        );
        assert_eq!(
            parse_mail_date("1 Oct 2026 10:20:30 -0500 (CST)").as_deref(),
            Some("2026-10-01T15:20:30Z")
        );
        assert_eq!(
            parse_mail_date("17-Jul-1996 02:44:25 -0700").as_deref(),
            Some("1996-07-17T09:44:25Z")
        );
        assert_eq!(
            parse_mail_date("Wed, 01 Oct 26 10:20 GMT").as_deref(),
            Some("2026-10-01T10:20:00Z")
        );
    }

    #[test]
    fn 认不出的时间返回空() {
        assert_eq!(parse_mail_date(""), None);
        assert_eq!(parse_mail_date("不是时间"), None);
        assert_eq!(parse_mail_date("1 个 2026 10:20 +0800"), None);
        assert_eq!(parse_mail_date("1 Oct 2026 10:20 火星时区"), None);
    }

    #[test]
    fn 搜索日期是两位数日期加月份缩写() {
        let text = format_imap_date(0);
        let parts: Vec<&str> = text.split('-').collect();
        assert_eq!(parts.len(), 3, "格式应为 dd-Mon-yyyy：{text}");
        assert_eq!(parts[0].len(), 2);
        assert_eq!(parts[1].len(), 3);
        assert_eq!(parts[2].len(), 4);
    }
}
