//! Controller-local bounds, including sleep. No clock/deadline crosses the wire
//! or survives a process restart. The native runner checks its own windows too.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Clock {
    instant: Instant,
    boot_ms: u64,
}
pub(super) struct Window {
    anchor: Clock,
    duration_ms: u64,
}
impl Clock {
    pub(super) fn sample() -> Result<Self> {
        let instant = Instant::now();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let boot_ms = {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } != 0 {
                return Err(Error::Forbidden);
            }
            let seconds: u64 = value.tv_sec.try_into().map_err(|_| Error::Forbidden)?;
            let nanos: u64 = value.tv_nsec.try_into().map_err(|_| Error::Forbidden)?;
            if nanos >= 1_000_000_000 {
                return Err(Error::Forbidden);
            }
            seconds
                .checked_mul(1000)
                .and_then(|v| v.checked_add(nanos / 1_000_000))
                .ok_or(Error::Forbidden)?
        };
        #[cfg(windows)]
        let boot_ms = {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetTickCount64() -> u64;
            }
            // The Windows uptime counter includes time spent asleep.
            unsafe { GetTickCount64() }
        };
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        return Err(Error::Forbidden);
        #[cfg(any(target_os = "linux", target_os = "android", windows))]
        Ok(Self { instant, boot_ms })
    }
    pub(super) fn window(self, duration_ms: i64) -> Result<Window> {
        let duration_ms = u64::try_from(duration_ms).map_err(|_| Error::Forbidden)?;
        if duration_ms == 0 || duration_ms > MANAGED_RESULT_READ_MILLIS as u64 {
            return Err(Error::Forbidden);
        }
        self.boot_ms
            .checked_add(duration_ms)
            .ok_or(Error::Forbidden)?;
        Ok(Window {
            anchor: self,
            duration_ms,
        })
    }
}
impl Window {
    pub(super) fn check(&self) -> Result<()> {
        self.check_at(Clock::sample()?)
    }
    fn check_at(&self, current: Clock) -> Result<()> {
        if current
            .instant
            .checked_duration_since(self.anchor.instant)
            .is_none_or(|v| v >= Duration::from_millis(self.duration_ms))
            || current
                .boot_ms
                .checked_sub(self.anchor.boot_ms)
                .is_none_or(|v| v >= self.duration_ms)
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suspended_controller_cannot_extend_original_response_window_or_roll_clock_back() {
        let anchor = Clock {
            instant: Instant::now(),
            boot_ms: 1000,
        };
        let window = anchor.window(5000).unwrap();
        assert!(
            window
                .check_at(Clock {
                    boot_ms: 5999,
                    ..anchor
                })
                .is_ok()
        );
        for boot_ms in [999, 6000, 11000] {
            assert!(window.check_at(Clock { boot_ms, ..anchor }).is_err());
        }
    }
    #[test]
    fn actual_controller_sleep_aware_clock_is_monotonic() {
        let first = Clock::sample().unwrap();
        first.window(30000).unwrap().check().unwrap();
        assert!(Clock::sample().unwrap().boot_ms >= first.boot_ms);
    }
}
