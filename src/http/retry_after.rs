#[cfg(test)]
mod tests;

use crate::runtime::{RetryAfter, RetryWait};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Date {
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

fn days(year: u32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let m = i64::from(month) + if month > 2 { -3 } else { 9 };
    era * 146097 + yo * 365 + yo / 4 - yo / 100 + (153 * m + 2) / 5 + i64::from(day) - 1 - 719468
}

fn utc(now: SystemTime) -> Option<Date> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let day = i64::try_from(seconds / 86400).ok()?;
    if day >= days(10000, 1, 1) {
        return None;
    }
    let mut year = 1970;
    let mut high = 10000;
    while year + 1 < high {
        let mid = (year + high) / 2;
        if days(mid, 1, 1) <= day {
            year = mid;
        } else {
            high = mid;
        }
    }
    let mut month = 1;
    while month < 12 && days(year, month + 1, 1) <= day {
        month += 1;
    }
    Some(Date {
        year,
        month,
        day: u32::try_from(day - days(year, month, 1) + 1).ok()?,
        hour: (seconds % 86400 / 3600) as u32,
        minute: (seconds % 3600 / 60) as u32,
        second: (seconds % 60) as u32,
    })
}

fn number(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0u32, |n, b| {
        b.is_ascii_digit().then_some(())?;
        n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
    })
}

const MONTHS: [&[u8]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];
const WEEKDAYS: [&[u8]; 7] = [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"];
const LONG_DAYS: [&[u8]; 7] = [
    b"Sunday",
    b"Monday",
    b"Tuesday",
    b"Wednesday",
    b"Thursday",
    b"Friday",
    b"Saturday",
];

fn date(bytes: &[u8], clock: Option<Date>) -> Result<(Date, usize), bool> {
    let (weekday, day, month, year, time, rolling) = if bytes.len() == 29
        && &bytes[3..5] == b", "
        && bytes[7] == b' '
        && bytes[11] == b' '
        && bytes[16] == b' '
        && &bytes[25..] == b" GMT"
    {
        (
            WEEKDAYS.iter().position(|w| *w == &bytes[..3]),
            &bytes[5..7],
            &bytes[8..11],
            &bytes[12..16],
            &bytes[17..25],
            false,
        )
    } else if bytes.len() == 24
        && bytes[3] == b' '
        && bytes[7] == b' '
        && bytes[10] == b' '
        && bytes[19] == b' '
    {
        let day = if bytes[8] == b' ' {
            &bytes[9..10]
        } else {
            &bytes[8..10]
        };
        (
            WEEKDAYS.iter().position(|w| *w == &bytes[..3]),
            day,
            &bytes[4..7],
            &bytes[20..24],
            &bytes[11..19],
            false,
        )
    } else if let Some(comma) = bytes.iter().position(|b| *b == b',') {
        let tail = &bytes[comma + 1..];
        if tail.len() != 23
            || tail[0] != b' '
            || tail[3] != b'-'
            || tail[7] != b'-'
            || tail[10] != b' '
            || &tail[19..] != b" GMT"
        {
            return Err(false);
        }
        (
            LONG_DAYS.iter().position(|w| *w == &bytes[..comma]),
            &tail[1..3],
            &tail[4..7],
            &tail[8..10],
            &tail[11..19],
            true,
        )
    } else {
        return Err(false);
    };
    let weekday = weekday.ok_or(false)?;
    let month = MONTHS.iter().position(|m| *m == month).ok_or(false)? as u32 + 1;
    if time.len() != 8 || time[2] != b':' || time[5] != b':' {
        return Err(false);
    }
    let mut result = Date {
        year: number(year).ok_or(false)?,
        month,
        day: number(day).ok_or(false)?,
        hour: number(&time[..2]).ok_or(false)?,
        minute: number(&time[3..5]).ok_or(false)?,
        second: number(&time[6..]).ok_or(false)?,
    };
    if rolling {
        let Some(clock) = clock else {
            for century in 20..24 {
                let candidate = Date {
                    year: result.year + century * 100,
                    ..result
                };
                if validate(candidate, weekday).is_ok() {
                    return Err(true);
                }
            }
            return Err(false);
        };
        let boundary = Date {
            year: clock.year + 50,
            ..clock
        };
        result.year += boundary.year / 100 * 100;
        if result > boundary {
            result.year = result.year.checked_sub(100).ok_or(true)?;
        }
    }
    validate(result, weekday)?;
    Ok((result, weekday))
}

fn validate(date: Date, weekday: usize) -> Result<(), bool> {
    let leap = date.year.is_multiple_of(4)
        && (!date.year.is_multiple_of(100) || date.year.is_multiple_of(400));
    let max = match date.month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if date.year == 0
        || date.day == 0
        || date.day > max
        || date.hour > 23
        || date.minute > 59
        || date.second > 60
        || (days(date.year, date.month, date.day) + 4).rem_euclid(7) as usize != weekday
    {
        return Err(false);
    }
    Ok(())
}

fn put_number(out: &mut [u8], mut number: u32) {
    for byte in out.iter_mut().rev() {
        *byte = b'0' + (number % 10) as u8;
        number /= 10;
    }
}

fn value(bytes: &[u8], now: SystemTime, clock: Option<Date>) -> Result<RetryWait, bool> {
    if !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit) {
        let seconds = bytes.iter().try_fold(0u64, |n, b| {
            n.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        });
        return Ok(seconds.map_or(RetryWait::Unrepresentable, |s| {
            RetryWait::Delay(Duration::from_secs(s))
        }));
    }
    let (date, weekday) = date(bytes, clock)?;
    if date.year < 1970 || date.year > 9999 || clock.is_none() {
        return Err(true);
    }
    let mut canonical = *b"Sun, 01 Jan 1970 00:00:00 GMT";
    canonical[..3].copy_from_slice(WEEKDAYS[weekday]);
    put_number(&mut canonical[5..7], date.day);
    canonical[8..11].copy_from_slice(MONTHS[date.month as usize - 1]);
    put_number(&mut canonical[12..16], date.year);
    put_number(&mut canonical[17..19], date.hour);
    put_number(&mut canonical[20..22], date.minute);
    put_number(&mut canonical[23..25], date.second.min(59));
    let normalized = std::str::from_utf8(&canonical).unwrap();
    let instant = httpdate::parse_http_date(normalized).map_err(|_| false)?;
    let instant = if date.second == 60 {
        instant.checked_add(Duration::from_secs(1)).ok_or(true)?
    } else {
        instant
    };
    Ok(RetryWait::Delay(
        instant.duration_since(now).unwrap_or_default(),
    ))
}

pub(crate) fn parse<'a>(values: impl Iterator<Item = &'a [u8]>, now: SystemTime) -> RetryAfter {
    let clock = utc(now);
    let mut result = RetryAfter::default();
    let mut count = 0;
    for bytes in values {
        count += 1;
        let start = bytes
            .iter()
            .position(|b| !matches!(b, b' ' | b'\t'))
            .unwrap_or(bytes.len());
        let end = bytes
            .iter()
            .rposition(|b| !matches!(b, b' ' | b'\t'))
            .map_or(start, |i| i + 1);
        let bytes = &bytes[start..end];
        match value(bytes, now, clock) {
            Ok(wait) => {
                result.valid += 1;
                result.wait = match (result.wait, wait) {
                    (Some(RetryWait::Unrepresentable), _) | (_, RetryWait::Unrepresentable) => {
                        Some(RetryWait::Unrepresentable)
                    }
                    (Some(RetryWait::Delay(a)), RetryWait::Delay(b)) => {
                        Some(RetryWait::Delay(a.max(b)))
                    }
                    (None, wait) => Some(wait),
                };
            }
            Err(true) => {
                result.unsupported += 1;
                result.wait = Some(RetryWait::Unrepresentable);
            }
            Err(false) => result.invalid += 1,
        }
    }
    result.missing = count == 0;
    result.duplicate = count > 1;
    result
}
