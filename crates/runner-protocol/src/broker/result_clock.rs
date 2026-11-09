//! Process-local result deadlines. Linux BOOTTIME includes suspended time;
//! neither clock sample nor deadline is serialized or reconstructed on restart.
use super::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) struct ResultReadClock {
    pub(crate) instant: Instant,
    pub(crate) boot_ns: Option<u64>,
}

pub(super) struct ResultReadDeadline {
    instant: Instant,
    boot: Option<(u64, u64)>,
}

impl ResultReadClock {
    pub(crate) fn sample() -> Result<Self> {
        let instant = Instant::now();
        #[cfg(target_os = "linux")]
        let boot_ns = {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // CLOCK_BOOTTIME is available on the qualified Linux runtime. An
            // unavailable clock fails closed; there is no weaker fallback.
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } < 0 {
                return Err(Error::Uncertain);
            }
            let seconds: u64 = value.tv_sec.try_into().map_err(|_| Error::Uncertain)?;
            let nanos: u64 = value.tv_nsec.try_into().map_err(|_| Error::Uncertain)?;
            if nanos >= 1_000_000_000 {
                return Err(Error::Uncertain);
            }
            Some(
                seconds
                    .checked_mul(1_000_000_000)
                    .and_then(|v| v.checked_add(nanos))
                    .ok_or(Error::Uncertain)?,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let boot_ns = None;
        Ok(Self { instant, boot_ns })
    }
}

impl ResultReadDeadline {
    pub(super) fn new(anchor: ResultReadClock, remaining_ms: i64) -> Result<Self> {
        let remaining: u64 = remaining_ms.try_into().map_err(|_| Error::Expired)?;
        if remaining == 0 || remaining > MANAGED_RESULT_READ_MILLIS as u64 {
            return Err(Error::Expired);
        }
        Ok(Self {
            instant: anchor
                .instant
                .checked_add(Duration::from_millis(remaining))
                .ok_or(Error::Expired)?,
            boot: anchor
                .boot_ns
                .map(|start| {
                    remaining
                        .checked_mul(1_000_000)
                        .and_then(|n| start.checked_add(n))
                        .map(|end| (start, end))
                        .ok_or(Error::Expired)
                })
                .transpose()?,
        })
    }

    pub(super) fn check(&self, current: ResultReadClock) -> Result<Instant> {
        if current.instant >= self.instant {
            return Err(Error::Expired);
        }
        match (self.boot, current.boot_ns) {
            (Some((start, end)), Some(now)) if now >= start && now < end => {}
            (None, None) => {}
            _ => return Err(Error::Expired),
        }
        Ok(self.instant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn actual_linux_boottime_is_present_and_monotonic_without_instant_fallback() {
        let first = ResultReadClock::sample().unwrap();
        let later = ResultReadClock::sample().unwrap();
        assert!(first.boot_ns.is_some_and(|value| value > 0));
        assert!(later.boot_ns.unwrap() >= first.boot_ns.unwrap());
        let deadline = ResultReadDeadline::new(first, 30_000).unwrap();
        deadline.check(later).unwrap();
    }

    #[test]
    fn suspend_expires_original_short_context_even_with_paused_instant_and_fresh_wall_time() {
        let instant = Instant::now();
        let anchor = ResultReadClock {
            instant,
            boot_ns: Some(1_000_000_000),
        };
        let short = ResultReadDeadline::new(anchor, 30_000).unwrap();
        // UTC is intentionally absent from this check. Even a valid signed
        // UTC window and long output retention cannot extend this private clock.
        assert!(
            short
                .check(ResultReadClock {
                    instant,
                    boot_ns: Some(61_000_000_000)
                })
                .is_err()
        );
        assert!(
            short
                .check(ResultReadClock {
                    instant,
                    boot_ns: Some(999_999_999)
                })
                .is_err()
        );
        assert!(
            short
                .check(ResultReadClock {
                    instant,
                    boot_ns: None
                })
                .is_err()
        );
        assert!(
            short
                .check(ResultReadClock {
                    instant,
                    boot_ns: Some(30_999_999_999)
                })
                .is_ok()
        );
        assert!(
            short
                .check(ResultReadClock {
                    instant,
                    boot_ns: Some(31_000_000_000)
                })
                .is_err()
        );
    }
}
