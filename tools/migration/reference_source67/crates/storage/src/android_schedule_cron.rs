//! IANA wall-clock cron: first fold only, gaps skipped, no shifted or replayed wall times.
use super::*;
use chrono::{LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::{
    str::FromStr,
    time::{Duration, Instant},
};
fn invalid() -> Error {
    Error::Validation("invalid_schedule_cron_or_timezone".into())
}
pub fn normalize(expression: &str, timezone: &str) -> Result<String> {
    if expression.len() > 512 || timezone.len() > 200 {
        return Err(invalid());
    }
    let fields: Vec<_> = expression.split_whitespace().collect();
    let expression = match fields.len() {
        5 => format!("0 {}", fields.join(" ")),
        6 | 7 => fields.join(" "),
        _ => return Err(invalid()),
    };
    Schedule::from_str(&expression).map_err(|_| invalid())?;
    timezone.parse::<Tz>().map_err(|_| invalid())?;
    Ok(expression)
}
pub fn next(expression: &str, timezone: &str, after: i64) -> Result<Option<i64>> {
    let expression = normalize(expression, timezone)?;
    let schedule = Schedule::from_str(&expression).map_err(|_| invalid())?;
    let tz = timezone.parse::<Tz>().map_err(|_| invalid())?;
    let utc = Utc
        .timestamp_millis_opt(after)
        .single()
        .ok_or_else(invalid)?;
    // Enumerate cron wall values in a fictitious UTC zone, then resolve each
    // actual wall value once. The cron iterator's second ambiguous result is
    // not accidentally treated as a second grant to execute.
    let wall = Utc.from_utc_datetime(&utc.with_timezone(&tz).naive_local());
    let deadline = Instant::now() + Duration::from_millis(250);
    for (i, candidate) in schedule.after(&wall).enumerate() {
        if i >= 10000 || Instant::now() > deadline {
            return Err(Error::BudgetExceeded);
        }
        let instant = match tz.from_local_datetime(&candidate.naive_utc()) {
            LocalResult::Single(v) => v,
            LocalResult::Ambiguous(a, b) => a.min(b),
            LocalResult::None => continue,
        };
        if instant.timestamp_millis() > after {
            return Ok(Some(instant.timestamp_millis()));
        }
    }
    Ok(None)
}
pub fn preview(expression: &str, timezone: &str, after: i64) -> Result<AndroidSchedulePreview> {
    let cron = normalize(expression, timezone)?;
    let mut cursor = after;
    let mut dates = Vec::new();
    for _ in 0..5 {
        let Some(value) = next(&cron, timezone, cursor)? else {
            break;
        };
        dates.push(crate::android_devices::device_iso(value)?);
        cursor = value;
    }
    Ok(AndroidSchedulePreview {
        cron,
        timezone: timezone.into(),
        next_occurrences: dates,
        ambiguous_time_policy: "first_occurrence".into(),
        nonexistent_time_policy: "skip".into(),
    })
}
pub(crate) fn missed(
    expression: &str,
    timezone: &str,
    due: i64,
    current: i64,
) -> Result<(u32, bool)> {
    let mut n = 1u32;
    let mut at = due;
    let deadline = Instant::now() + Duration::from_millis(250);
    while n < 10000 && Instant::now() < deadline {
        let Some(next) = next(expression, timezone, at)? else {
            return Ok((n, false));
        };
        if next > current {
            return Ok((n, false));
        }
        at = next;
        n += 1;
    }
    Ok((n, true))
}
