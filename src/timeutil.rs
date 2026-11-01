//! Minimal local-time support without pulling in a date crate
//! (the common ones need Windows APIs newer than Windows 9x/7 provide).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(unix)]
pub fn local_offset(ts: i64) -> i64 {
    unsafe {
        let t: libc::time_t = ts as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
pub fn local_offset(_ts: i64) -> i64 {
    #[repr(C)]
    struct SystemTimeW {
        w: [u16; 8],
    }
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTimeW,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTimeW,
        daylight_bias: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
    }
    unsafe {
        let mut tz: TimeZoneInformation = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let bias = match r {
            1 => tz.bias + tz.standard_bias,
            2 => tz.bias + tz.daylight_bias,
            0 => tz.bias,
            _ => 0,
        };
        -(bias as i64) * 60
    }
}

#[cfg(not(any(unix, windows)))]
pub fn local_offset(_ts: i64) -> i64 {
    0
}

#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn local(ts: i64) -> DateTime {
    let t = ts + local_offset(ts);
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    DateTime { year, month, day, hour: (secs / 3600) as u32, minute: ((secs % 3600) / 60) as u32, second: (secs % 60) as u32 }
}

pub fn fmt_time(ts: i64, seconds: bool) -> String {
    let d = local(ts);
    if seconds {
        format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second)
    } else {
        format!("{:02}:{:02}", d.hour, d.minute)
    }
}

pub fn fmt_date(ts: i64) -> String {
    let d = local(ts);
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

pub fn fmt_datetime(ts: i64) -> String {
    format!("{} {}", fmt_date(ts), fmt_time(ts, true))
}

pub fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m, s) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// A duration as "m:ss", rounded up so it never shows 0:00 while waiting.
pub fn mmss(d: std::time::Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
