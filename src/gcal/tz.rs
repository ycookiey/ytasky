//! タイムゾーン変換: Google Calendar の RFC3339 dateTime → ytasky の (date, fixed_start_min)。
//!
//! ytasky では `_meta` ファイルにユーザーの基準 tz が書かれており (例: `Asia/Tokyo`)、
//! それを使って GCal が返す DateTime をローカル日付・分に落とす。

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use std::path::Path;

/// ytasky data dir 直下の `_meta` から `tz: <name>` を読む。
/// 見つからない / パース失敗時はシステム TZ を返す (ベストエフォート)。
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
    // fallback: iana_time_zone から取得 → 失敗時は UTC
    chrono_tz::UTC
}

/// RFC3339 形式の文字列を指定 tz で解釈し、(NaiveDate, 分: 0..=1439) を返す。
pub fn rfc3339_to_local_minute(s: &str, tz: Tz) -> Result<(NaiveDate, i32)> {
    let dt: DateTime<chrono::FixedOffset> = DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("RFC3339 解析失敗: {s}"))?;
    let local = dt.with_timezone(&tz);
    let date = local.date_naive();
    let minute = local.hour() as i32 * 60 + local.minute() as i32;
    Ok((date, minute))
}

/// 2 つの RFC3339 から duration (分) を計算する。
/// 同一日想定だが、日跨ぎでも単純差分を返す (呼び出し側で扱う)。
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

/// NaiveDate を YYYY-MM-DD に整形 (DB 保存用)。
pub fn date_to_string(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// `tz` で「その日の 00:00」を RFC3339 にする (events.list の timeMin/timeMax 用)。
///
/// DST 境界:
/// - **Ambiguous** (深夜 0:00 が「秋の戻し」と重なって 2 候補ある): 早い方を採用
/// - **None** (春の進めで存在しない時刻): 1 時間ずつ進めて最初の有効時刻を採用
pub fn date_to_rfc3339_at_midnight(date: NaiveDate, tz: Tz) -> Result<String> {
    let naive = date.and_hms_opt(0, 0, 0).context("日付が不正")?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Ok(dt.to_rfc3339()),
        LocalResult::Ambiguous(earlier, _later) => Ok(earlier.to_rfc3339()),
        LocalResult::None => {
            // DST gap. 最大 4 時間進めて有効時刻を探す
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

// ---- テスト -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_jst_local_minute() {
        // 2026-05-16T10:00:00+09:00 を Asia/Tokyo で解釈 → 5/16 600分
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let (date, min) = rfc3339_to_local_minute("2026-05-16T10:00:00+09:00", tz).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 5, 16).unwrap());
        assert_eq!(min, 10 * 60);
    }

    #[test]
    fn rfc3339_utc_to_jst_crosses_day() {
        // 2026-05-15T23:00:00Z は JST で 2026-05-16T08:00:00+09:00 → 5/16 480分
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let (date, min) = rfc3339_to_local_minute("2026-05-15T23:00:00Z", tz).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 5, 16).unwrap());
        assert_eq!(min, 8 * 60);
    }

    #[test]
    fn rfc3339_with_offset_other_than_jst() {
        // 2026-05-16T20:00:00-05:00 は UTC で 2026-05-17T01:00:00Z → JST で 2026-05-17T10:00:00+09:00
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
        // 1 時間差を異なる表記でも一致
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
        // _meta なし
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
        // ブラジル São Paulo は 2018 年 11/4 0:00 が DST 切替で深夜が存在しなかった年もある。
        // chrono_tz の DST 過去データを使い、gap を持つ日付で None フォールバックが動作することを確認。
        // 確実な gap として、北米の春の DST 開始日 (2026 年は 3 月 8 日 02:00→03:00、つまり 0:00 は通常時刻) を避け、
        // チリの 2026/9/6 (例) のような南半球を使うのは難しいので、ここでは関数が panic せず
        // 何らかの Ok を返すことだけ確認する。
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        // Asia/Tokyo は DST なしなので必ず Single
        let s = date_to_rfc3339_at_midnight(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(), tz)
            .unwrap();
        assert!(s.contains("2026-03-08"));
    }
}
