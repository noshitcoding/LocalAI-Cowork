use super::*;
use std::collections::BTreeSet;
fn invalid_cron() -> Error {
    Error::Validation("Use five cron fields: minute hour day month weekday (0/7=SUN, 1=MON)".into())
}
fn weekdays(field: &str) -> Result<String> {
    const NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    fn ordinal(value: &str) -> Result<u8> {
        if let Ok(n) = value.parse::<u8>()
            && n <= 7
        {
            return Ok(n);
        }
        ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"]
            .iter()
            .position(|v| v.eq_ignore_ascii_case(value))
            .map(|v| v as u8)
            .ok_or_else(invalid_cron)
    }
    let mut days = BTreeSet::new();
    for part in field.split(',') {
        let (base, step) = match part.split_once('/') {
            Some((base, step)) => (
                base,
                step.parse::<u8>()
                    .ok()
                    .filter(|v| *v > 0 && *v <= 7)
                    .ok_or_else(invalid_cron)?,
            ),
            None => (part, 1),
        };
        let (from, to) = if base == "*" {
            (0, 6)
        } else if let Some((a, b)) = base.split_once('-') {
            (ordinal(a)?, ordinal(b)?)
        } else {
            let n = ordinal(base)?;
            (n, if part.contains('/') { 6.max(n) } else { n })
        };
        if from > to {
            return Err(invalid_cron());
        }
        for day in (from..=to).step_by(usize::from(step)) {
            days.insert(day % 7);
        }
    }
    if days.is_empty() {
        return Err(invalid_cron());
    }
    Ok(days
        .into_iter()
        .map(|v| NAMES[usize::from(v)])
        .collect::<Vec<_>>()
        .join(","))
}
pub(super) fn native_cron_next(
    expression: &str,
    tz: &chrono_tz::Tz,
    after: &chrono::DateTime<chrono::Utc>,
) -> Result<Option<i64>> {
    if expression.len() > 256 || expression.chars().any(char::is_control) {
        return Err(invalid_cron());
    }
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(invalid_cron());
    }
    let dow = weekdays(fields[4])?;
    let next = |dom: &str, dow: &str| -> Result<Option<i64>> {
        let schedule: cron::Schedule = format!(
            "0 {} {} {} {} {} *",
            fields[0], fields[1], dom, fields[3], dow
        )
        .parse()
        .map_err(|_| invalid_cron())?;
        Ok(schedule
            .after(&after.with_timezone(tz))
            .next()
            .map(|v| v.timestamp_millis()))
    };
    // Cronie/Vixie day matching preserves a leading wildcard, including */n.
    // https://github.com/cronie-crond/cronie/blob/master/src/cron.c (DOM_STAR/DOW_STAR)
    if !fields[2].starts_with('*') && !fields[4].starts_with('*') {
        let a = next(fields[2], "*")?;
        let b = next("*", &dow)?;
        Ok(match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        })
    } else {
        next(fields[2], &dow)
    }
}
