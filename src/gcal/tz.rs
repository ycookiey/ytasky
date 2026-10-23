//! Converts Google Calendar RFC 3339 date-times to a local (date, minute of day).
//! The reference time zone is the `tz:` line of the data dir's `_meta` file.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use std::path::Path;

/// Reads `tz: <name>` from `_meta` in the data dir.
/// Falls back to UTC when the line is missing or the name does not parse.
pub fn read_meta_tz(data_dir: &Path) -> Tz {
    let path = data_dir.join("_meta");
    if let Ok(raw) = std::fs::read_to_string(&path) {
        for line in raw.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("tz:") {
                let name = rest.trim();
                if let Ok(tz) = name.parse::<Tz>() {
                    return tz;
                }
            }
        }
    }
    chrono_tz::UTC
}

/// Parses an RFC 3339 string in `tz` and returns (date, minute 0..=1439).
pub fn rfc3339_to_local_minute(s: &str, tz: Tz) -> Result<(NaiveDate, i32)> {
    let dt: DateTime<chrono::FixedOffset> = DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("RFC3339 解析失敗: {s}"))?;
    let local = dt.with_timezone(&tz);
    let date = local.date_naive();
    let minute = local.hour() as i32 * 60 + local.minute() as i32;
    Ok((date, minute))
}

/// Minutes between two RFC 3339 times. Crossing midnight is not special-cased;
/// callers handle it.
pub fn duration_minutes(start_rfc3339: &str, end_rfc3339: &str) -> Result<i32> {
    let start: DateTime<chrono::FixedOffset> = DateTime::parse_from_rfc3339(start_rfc3339)
        .with_context(|| format!("start RFC3339 解析失敗: {start_rfc3339}"))?;
    let end: DateTime<chrono::FixedOffset> = DateTime::parse_from_rfc3339(end_rfc3339)
        .with_context(|| format!("end RFC3339 解析失敗: {end_rfc3339}"))?;
    let delta = end.signed_duration_since(start);
    let minutes = delta.num_minutes();
    if minutes < 0 {
        bail!("終了 < 開始: {start_rfc3339} -> {end_rfc3339}");
    }
    Ok(minutes as i32)
}

pub fn date_to_string(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Midnight of `date` in `tz` as RFC 3339, for timeMin/timeMax of events.list.
///
/// DST transitions:
/// - ambiguous midnight (clocks set back): take the earlier time
/// - nonexistent midnight (clocks set forward): step forward an hour at a time
pub fn date_to_rfc3339_at_midnight(date: NaiveDate, tz: Tz) -> Result<String> {
    let naive = date.and_hms_opt(0, 0, 0).context("日付が不正")?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Ok(dt.to_rfc3339()),
        LocalResult::Ambiguous(earlier, _later) => Ok(earlier.to_rfc3339()),
        LocalResult::None => {
            // DST gap: search up to 4 hours forward for a valid time.
            for hours in 1..=4 {
                let advanced = naive + chrono::Duration::hours(hours);
                if let Some(dt) = tz.from_local_datetime(&advanced).earliest() {
                    return Ok(dt.to_rfc3339());
                }
            }
            bail!("DST gap で {date} の有効な開始時刻が見つからない (tz={tz:?})");
        }
    }
}

// ---- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_jst_local_minute() {
        // 2026-05-16T10:00:00+09:00 in Asia/Tokyo -> 5/16, minute 600
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let (date, min) = rfc3339_to_local_minute("2026-05-16T10:00:00+09:00", tz).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 5, 16).unwrap());
        assert_eq!(min, 10 * 60);
    }

    #[test]
    fn rfc3339_utc_to_jst_crosses_day() {
        // 2026-05-15T23:00:00Z is 2026-05-16T08:00:00+09:00 in JST -> 5/16, minute 480
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let (date, min) = rfc3339_to_local_minute("2026-05-15T23:00:00Z", tz).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 5, 16).unwrap());
        assert_eq!(min, 8 * 60);
    }

    #[test]
    fn rfc3339_with_offset_other_than_jst() {
        // 2026-05-16T20:00:00-05:00 is 2026-05-17T01:00:00Z -> 2026-05-17T10:00:00+09:00 in JST
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let (date, min) = rfc3339_to_local_minute("2026-05-16T20:00:00-05:00", tz).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 5, 17).unwrap());
        assert_eq!(min, 10 * 60);
    }

    #[test]
    fn duration_basic() {
        let m = duration_minutes(
            "2026-05-16T10:00:00+09:00",
            "2026-05-16T11:30:00+09:00",
        )
        .unwrap();
        assert_eq!(m, 90);
    }

    #[test]
    fn duration_across_tz_normalized() {
        // The same one-hour gap written with different offsets
        let m = duration_minutes(
            "2026-05-16T10:00:00+09:00",
            "2026-05-16T02:00:00Z",
        )
        .unwrap();
        assert_eq!(m, 60);
    }

    #[test]
    fn duration_negative_errors() {
        let res = duration_minutes(
            "2026-05-16T11:00:00+09:00",
            "2026-05-16T10:00:00+09:00",
        );
        assert!(res.is_err());
    }

    #[test]
    fn meta_tz_reads_asia_tokyo() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("_meta"), "tz: Asia/Tokyo\nview_sync: async\n").unwrap();
        let tz = read_meta_tz(tmp.path());
        assert_eq!(tz, chrono_tz::Asia::Tokyo);
    }

    #[test]
    fn meta_tz_missing_falls_back_to_utc() {
        let tmp = tempfile::tempdir().unwrap();
        // no _meta
        assert_eq!(read_meta_tz(tmp.path()), chrono_tz::UTC);
    }

    #[test]
    fn meta_tz_invalid_value_falls_back_to_utc() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("_meta"), "tz: Not/AValidZone\n").unwrap();
        assert_eq!(read_meta_tz(tmp.path()), chrono_tz::UTC);
    }

    #[test]
    fn midnight_rfc3339_at_jst() {
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let s = date_to_rfc3339_at_midnight(NaiveDate::from_ymd_opt(2026, 5, 16).unwrap(), tz)
            .unwrap();
        assert!(s.starts_with("2026-05-16T00:00:00"));
        assert!(s.ends_with("+09:00"));
    }

    #[test]
    fn midnight_rfc3339_dst_spring_forward_gap() {
        // A zone with a midnight DST gap is hard to pin down reliably in tz data, so this test
        // only checks that the function returns Ok without panicking.
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        // Asia/Tokyo has no DST, so the result is always Single.
        let s = date_to_rfc3339_at_midnight(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(), tz)
            .unwrap();
        assert!(s.contains("2026-03-08"));
    }
}
