//! GCal `recurrence` 配列 (`["RRULE:...", "EXDATE:...", "RDATE:..."]`) を
//! ytasky の (pattern, PatternData, end_date, exceptions, additions) に変換する。
//!
//! サポート範囲は設計書 §4.1 のマトリクスのみ。範囲外の RRULE が来たら
//! [`RruleError::Unsupported`] を返し、呼び出し側で個別 instance 展開に
//! フォールバックする。

use chrono::{Duration, NaiveDate};

use crate::model::PatternData;

/// EXDATE / RDATE のリスト長上限。これを超える RRULE は敵対カレンダー
/// 起因の DoS とみなして Invalid を返す。
const MAX_EXDATE_RDATE_ITEMS: usize = 1000;
/// COUNT の上限。RFC 5545 上は無制限だが、ytasky の現実的な範囲では
/// 10000 を超える繰り返しは想定外として弾く。
const MAX_COUNT: u32 = 10_000;

#[derive(Debug)]
pub enum RruleError {
    /// ytasky の recurrence で表現不能。呼び出し側は個別 instance 展開へ。
    Unsupported(String),
    /// RRULE 文字列の構文・値が不正。
    Invalid(String),
}

impl std::fmt::Display for RruleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RruleError::Unsupported(msg) => write!(f, "unsupported RRULE: {msg}"),
            RruleError::Invalid(msg) => write!(f, "invalid RRULE: {msg}"),
        }
    }
}

impl std::error::Error for RruleError {}

/// 解析結果。
#[derive(Debug, Clone, Default)]
pub struct ParsedRecurrence {
    pub pattern: String,
    pub pattern_data: PatternData,
    /// UNTIL / COUNT から導出した end_date (含む)
    pub end_date: Option<NaiveDate>,
    /// EXDATE 行から抽出した除外日
    pub exdates: Vec<NaiveDate>,
    /// RDATE 行から抽出した追加日 (recurrence と独立に task 化する)
    pub rdates: Vec<NaiveDate>,
}

/// GCal の `recurrence` 配列を解釈する。
///
/// `start_date` は COUNT を end_date に換算する際の起点。
pub fn parse_recurrence_rules(
    rules: &[String],
    start_date: NaiveDate,
) -> Result<ParsedRecurrence, RruleError> {
    let mut rrule_line: Option<&str> = None;
    let mut exdates: Vec<NaiveDate> = Vec::new();
    let mut rdates: Vec<NaiveDate> = Vec::new();
    for raw in rules {
        let line = raw.trim();
        if let Some(body) = line.strip_prefix("RRULE:") {
            if rrule_line.is_some() {
                return Err(RruleError::Unsupported(
                    "RRULE が複数行: ytasky では 1 本のみサポート".into(),
                ));
            }
            rrule_line = Some(body);
        } else if let Some(body) = line.strip_prefix("EXDATE") {
            for d in parse_date_list_after_colon(body)? {
                exdates.push(d);
                if exdates.len() > MAX_EXDATE_RDATE_ITEMS {
                    return Err(RruleError::Invalid(format!(
                        "EXDATE が {MAX_EXDATE_RDATE_ITEMS} 件を超える"
                    )));
                }
            }
        } else if let Some(body) = line.strip_prefix("RDATE") {
            for d in parse_date_list_after_colon(body)? {
                rdates.push(d);
                if rdates.len() > MAX_EXDATE_RDATE_ITEMS {
                    return Err(RruleError::Invalid(format!(
                        "RDATE が {MAX_EXDATE_RDATE_ITEMS} 件を超える"
                    )));
                }
            }
        }
    }

    let body = rrule_line
        .ok_or_else(|| RruleError::Invalid("RRULE 行が無い".into()))?;
    let parts = split_rule_parts(body)?;

    let mut parsed = ParsedRecurrence::default();
    parsed.exdates = exdates;
    parsed.rdates = rdates;

    let freq = parts
        .get("FREQ")
        .ok_or_else(|| RruleError::Invalid("FREQ が無い".into()))?
        .as_str();
    let interval_raw: Option<u8> = parts
        .get("INTERVAL")
        .map(|v| v.parse::<u8>())
        .transpose()
        .map_err(|_| RruleError::Invalid("INTERVAL は数値".into()))?;
    // INTERVAL=0 は RFC 5545 違反。サイレントに 1 と解釈せず明示的に弾く
    if interval_raw == Some(0) {
        return Err(RruleError::Invalid("INTERVAL=0 は不正".into()));
    }
    // pattern_data 上では interval=1 を None として持つ (DB に書かない)
    let interval: Option<u8> = interval_raw.filter(|n| *n != 1);
    let count: Option<u32> = parts
        .get("COUNT")
        .map(|v| v.parse::<u32>())
        .transpose()
        .map_err(|_| RruleError::Invalid("COUNT は数値".into()))?;
    if let Some(c) = count {
        if c == 0 {
            return Err(RruleError::Invalid("COUNT=0 は不正".into()));
        }
        if c > MAX_COUNT {
            return Err(RruleError::Invalid(format!(
                "COUNT={c} は上限 {MAX_COUNT} を超える"
            )));
        }
    }
    let until_date: Option<NaiveDate> = parts
        .get("UNTIL")
        .map(|v| parse_until_value(v))
        .transpose()?;

    if parts.contains_key("BYWEEKNO") || parts.contains_key("BYYEARDAY") {
        return Err(RruleError::Unsupported(format!("FREQ={freq} BYWEEKNO/BYYEARDAY")));
    }

    let interval_step: u32 = parts
        .get("INTERVAL")
        .map(|v| v.parse::<u32>())
        .transpose()
        .map_err(|_| RruleError::Invalid("INTERVAL は数値".into()))?
        .unwrap_or(1);

    match freq {
        "DAILY" => {
            if parts.contains_key("BYDAY")
                || parts.contains_key("BYMONTHDAY")
                || parts.contains_key("BYMONTH")
            {
                return Err(RruleError::Unsupported("DAILY + BY*".into()));
            }
            parsed.pattern = "daily".into();
            parsed.pattern_data.interval = interval;
        }
        "WEEKLY" => {
            if parts.contains_key("BYMONTHDAY") || parts.contains_key("BYMONTH") {
                return Err(RruleError::Unsupported("WEEKLY + BYMONTHDAY/BYMONTH".into()));
            }
            parsed.pattern = "weekly".into();
            parsed.pattern_data.interval = interval;
            // BYDAY が無ければ start_date の曜日を採用
            if let Some(byday) = parts.get("BYDAY") {
                let (days, _) = parse_byday_plain(byday)?;
                if days.is_empty() {
                    return Err(RruleError::Invalid("BYDAY が空".into()));
                }
                parsed.pattern_data.days = Some(days);
            } else {
                use chrono::Datelike;
                let wd = start_date.weekday().number_from_monday() as u8;
                parsed.pattern_data.days = Some(vec![wd]);
            }
        }
        "MONTHLY" => {
            if parts.contains_key("BYMONTH") {
                return Err(RruleError::Unsupported("MONTHLY + BYMONTH".into()));
            }
            parsed.pattern = "monthly".into();
            parsed.pattern_data.interval = interval;
            let has_byday = parts.contains_key("BYDAY");
            let has_bymonthday = parts.contains_key("BYMONTHDAY");
            let has_bysetpos = parts.contains_key("BYSETPOS");
            if has_byday && has_bymonthday {
                return Err(RruleError::Unsupported(
                    "MONTHLY + BYDAY + BYMONTHDAY 併用".into(),
                ));
            }
            if let Some(byday) = parts.get("BYDAY") {
                // 形式: "2TU" / "-1FR" / "TU" / "MO,WE" (複数は未対応)
                let (days, prefix_setpos) = parse_byday_with_prefix(byday)?;
                if days.len() != 1 {
                    return Err(RruleError::Unsupported("MONTHLY BYDAY 複数曜日".into()));
                }
                let setpos: i8 = if let Some(n) = prefix_setpos {
                    n
                } else if has_bysetpos {
                    parts["BYSETPOS"]
                        .parse::<i8>()
                        .map_err(|_| RruleError::Invalid("BYSETPOS は数値".into()))?
                } else {
                    return Err(RruleError::Unsupported(
                        "MONTHLY BYDAY without N prefix or BYSETPOS".into(),
                    ));
                };
                parsed.pattern_data.days = Some(days);
                parsed.pattern_data.setpos = Some(setpos);
            } else if let Some(bymonthday) = parts.get("BYMONTHDAY") {
                let day_nums: Vec<u8> = bymonthday
                    .split(',')
                    .map(|s| s.trim().parse::<u8>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| RruleError::Invalid("BYMONTHDAY は数値".into()))?;
                if day_nums.is_empty() {
                    return Err(RruleError::Invalid("BYMONTHDAY が空".into()));
                }
                parsed.pattern_data.days = Some(day_nums);
            } else {
                // BYDAY/BYMONTHDAY 無し → start_date の日付を採用
                use chrono::Datelike;
                parsed.pattern_data.days = Some(vec![start_date.day() as u8]);
            }
        }
        other => {
            return Err(RruleError::Unsupported(format!("FREQ={other}")));
        }
    }

    // end_date の決定: UNTIL > COUNT
    parsed.end_date = if let Some(d) = until_date {
        Some(d)
    } else if let Some(n) = count {
        compute_end_from_count(&parsed.pattern, &parsed.pattern_data, start_date, n, interval_step)
    } else {
        None
    };

    Ok(parsed)
}

// ---- helpers ------------------------------------------------------------------

fn split_rule_parts(body: &str) -> Result<std::collections::HashMap<String, String>, RruleError> {
    let mut map = std::collections::HashMap::new();
    for part in body.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| RruleError::Invalid(format!("KEY=VALUE 形式でない: {part}")))?;
        map.insert(k.trim().to_uppercase(), v.trim().to_string());
    }
    Ok(map)
}

fn parse_until_value(v: &str) -> Result<NaiveDate, RruleError> {
    // 8 桁 (YYYYMMDD) or 15/16 桁 (YYYYMMDDTHHMMSSZ)
    let date_part = if v.len() >= 8 { &v[..8] } else { v };
    NaiveDate::parse_from_str(date_part, "%Y%m%d")
        .map_err(|_| RruleError::Invalid(format!("UNTIL 解析失敗: {v}")))
}

/// EXDATE / RDATE の `;TZID=...:`, `:` 区切り、`,` 区切りの値リストから NaiveDate を抜く。
fn parse_date_list_after_colon(body: &str) -> Result<Vec<NaiveDate>, RruleError> {
    // body 例: ":20260516,20260517" / ";TZID=Asia/Tokyo:20260516T100000"
    let colon_idx = body
        .rfind(':')
        .ok_or_else(|| RruleError::Invalid(format!("EXDATE/RDATE に ':' が無い: {body}")))?;
    let values = &body[colon_idx + 1..];
    let mut out = Vec::new();
    for v in values.split(',') {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        let date_part = if v.len() >= 8 { &v[..8] } else { v };
        let d = NaiveDate::parse_from_str(date_part, "%Y%m%d")
            .map_err(|_| RruleError::Invalid(format!("EXDATE/RDATE の日付解析失敗: {v}")))?;
        out.push(d);
    }
    Ok(out)
}

/// "MO,WE,FR" のような複数曜日。各曜日に N プレフィックス (例: "1MO") が
/// 付いていれば一律 setpos を返すが、ここでは prefix なし前提とする。
fn parse_byday_plain(byday: &str) -> Result<(Vec<u8>, Option<i8>), RruleError> {
    let mut days = Vec::new();
    let mut common_prefix: Option<i8> = None;
    for token in byday.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (prefix, day) = split_byday_token(token)?;
        if prefix.is_some() {
            // weekly では prefix 付きは未対応 (monthly 用)
            return Err(RruleError::Unsupported(format!(
                "WEEKLY で N プレフィックス: {token}"
            )));
        }
        days.push(day);
        common_prefix = common_prefix.or(prefix);
    }
    days.sort_unstable();
    days.dedup();
    Ok((days, common_prefix))
}

/// "2TU" / "TU" / "-1FR" のような単一トークン (monthly 用)。
fn parse_byday_with_prefix(byday: &str) -> Result<(Vec<u8>, Option<i8>), RruleError> {
    let mut days = Vec::new();
    let mut last_prefix: Option<i8> = None;
    let mut mixed = false;
    for token in byday.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (prefix, day) = split_byday_token(token)?;
        if let Some(p) = prefix {
            if let Some(prev) = last_prefix {
                if prev != p {
                    mixed = true;
                }
            }
            last_prefix = Some(p);
        }
        days.push(day);
    }
    if mixed {
        return Err(RruleError::Unsupported("MONTHLY BYDAY の N プレフィックス混在".into()));
    }
    Ok((days, last_prefix))
}

fn split_byday_token(token: &str) -> Result<(Option<i8>, u8), RruleError> {
    // 数値プレフィックス ( -? digits ) + 2 文字曜日コード
    let bytes = token.as_bytes();
    if bytes.len() < 2 {
        return Err(RruleError::Invalid(format!("BYDAY token: {token}")));
    }
    let split_idx = bytes.len() - 2;
    let prefix_str = &token[..split_idx];
    let code = &token[split_idx..];
    let day = weekday_code_to_number(code)?;
    let prefix = if prefix_str.is_empty() {
        None
    } else {
        Some(
            prefix_str
                .parse::<i8>()
                .map_err(|_| RruleError::Invalid(format!("BYDAY prefix: {prefix_str}")))?,
        )
    };
    Ok((prefix, day))
}

fn weekday_code_to_number(code: &str) -> Result<u8, RruleError> {
    Ok(match code.to_uppercase().as_str() {
        "MO" => 1,
        "TU" => 2,
        "WE" => 3,
        "TH" => 4,
        "FR" => 5,
        "SA" => 6,
        "SU" => 7,
        other => return Err(RruleError::Invalid(format!("曜日コード: {other}"))),
    })
}

/// COUNT を end_date に換算する近似計算。
/// 簡易: daily=interval 日, weekly=interval 週×当該曜日数, monthly=interval 月。
/// 真の値より遠めに見積もり、生成側で UNTIL 込みで切る前提。
fn compute_end_from_count(
    pattern: &str,
    pd: &PatternData,
    start: NaiveDate,
    count: u32,
    interval: u32,
) -> Option<NaiveDate> {
    let n = count.saturating_sub(1) as i64;
    let step = interval.max(1) as i64;
    match pattern {
        "daily" => Some(start + Duration::days(n * step)),
        "weekly" => {
            let per_week = pd.days.as_ref().map(|v| v.len() as i64).unwrap_or(1).max(1);
            let weeks = (n + per_week - 1) / per_week;
            Some(start + Duration::weeks(weeks * step))
        }
        "monthly" => {
            // chrono::Months で n ヶ月加算
            let months = (n * step) as u32;
            start.checked_add_months(chrono::Months::new(months))
        }
        _ => None,
    }
}

// ---- テスト -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn parse(rules: &[&str], start: NaiveDate) -> Result<ParsedRecurrence, RruleError> {
        let v: Vec<String> = rules.iter().map(|s| s.to_string()).collect();
        parse_recurrence_rules(&v, start)
    }

    #[test]
    fn daily_simple() {
        let r = parse(&["RRULE:FREQ=DAILY"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern, "daily");
        assert_eq!(r.pattern_data.interval, None);
        assert!(r.end_date.is_none());
    }

    #[test]
    fn daily_interval_3() {
        let r = parse(&["RRULE:FREQ=DAILY;INTERVAL=3"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern, "daily");
        assert_eq!(r.pattern_data.interval, Some(3));
    }

    #[test]
    fn weekly_byday_mo_we_fr() {
        let r = parse(&["RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern, "weekly");
        assert_eq!(r.pattern_data.days, Some(vec![1, 3, 5]));
    }

    #[test]
    fn weekly_interval_2_tuesday() {
        let r = parse(
            &["RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU"],
            d(2026, 5, 16),
        )
        .unwrap();
        assert_eq!(r.pattern, "weekly");
        assert_eq!(r.pattern_data.interval, Some(2));
        assert_eq!(r.pattern_data.days, Some(vec![2]));
    }

    #[test]
    fn weekly_default_byday_uses_start_weekday() {
        // 2026-05-16 = 土曜 (6)
        let r = parse(&["RRULE:FREQ=WEEKLY"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern_data.days, Some(vec![6]));
    }

    #[test]
    fn monthly_bymonthday_15() {
        let r = parse(&["RRULE:FREQ=MONTHLY;BYMONTHDAY=15"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern, "monthly");
        assert_eq!(r.pattern_data.days, Some(vec![15]));
        assert_eq!(r.pattern_data.setpos, None);
    }

    #[test]
    fn monthly_byday_2tu() {
        let r = parse(&["RRULE:FREQ=MONTHLY;BYDAY=2TU"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern_data.days, Some(vec![2]));
        assert_eq!(r.pattern_data.setpos, Some(2));
    }

    #[test]
    fn monthly_byday_bysetpos() {
        let r = parse(
            &["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"],
            d(2026, 5, 16),
        )
        .unwrap();
        assert_eq!(r.pattern_data.days, Some(vec![2]));
        assert_eq!(r.pattern_data.setpos, Some(2));
    }

    #[test]
    fn monthly_byday_minus_1fr() {
        let r = parse(&["RRULE:FREQ=MONTHLY;BYDAY=-1FR"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern_data.days, Some(vec![5]));
        assert_eq!(r.pattern_data.setpos, Some(-1));
    }

    #[test]
    fn monthly_default_uses_start_day() {
        let r = parse(&["RRULE:FREQ=MONTHLY"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern_data.days, Some(vec![16]));
    }

    #[test]
    fn until_extracts_end_date() {
        let r = parse(
            &["RRULE:FREQ=DAILY;UNTIL=20261231T000000Z"],
            d(2026, 5, 16),
        )
        .unwrap();
        assert_eq!(r.end_date, Some(d(2026, 12, 31)));
    }

    #[test]
    fn count_extracts_end_date_daily() {
        // COUNT=10, FREQ=DAILY → start から 9 日後
        let r = parse(&["RRULE:FREQ=DAILY;COUNT=10"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.end_date, Some(d(2026, 5, 25)));
    }

    #[test]
    fn count_extracts_end_date_weekly() {
        // COUNT=10, FREQ=WEEKLY;BYDAY=MO,WE → 5 週分
        let r = parse(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=10"],
            d(2026, 5, 16),
        )
        .unwrap();
        // 5 週後の同曜日 = 2026-05-16 + 5*7 = 2026-06-20
        assert_eq!(r.end_date, Some(d(2026, 6, 20)));
    }

    #[test]
    fn exdate_parsed() {
        let r = parse(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=Asia/Tokyo:20260518T100000",
                "EXDATE:20260525",
            ],
            d(2026, 5, 11),
        )
        .unwrap();
        assert_eq!(r.exdates, vec![d(2026, 5, 18), d(2026, 5, 25)]);
    }

    #[test]
    fn rdate_parsed() {
        let r = parse(
            &[
                "RRULE:FREQ=DAILY",
                "RDATE:20260601,20260615",
            ],
            d(2026, 5, 16),
        )
        .unwrap();
        assert_eq!(r.rdates, vec![d(2026, 6, 1), d(2026, 6, 15)]);
    }

    #[test]
    fn unsupported_yearly() {
        let err = parse(&["RRULE:FREQ=YEARLY"], d(2026, 5, 16)).unwrap_err();
        assert!(matches!(err, RruleError::Unsupported(_)));
    }

    #[test]
    fn unsupported_byweekno() {
        let err = parse(
            &["RRULE:FREQ=WEEKLY;BYWEEKNO=1,2;BYDAY=MO"],
            d(2026, 5, 16),
        )
        .unwrap_err();
        assert!(matches!(err, RruleError::Unsupported(_)));
    }

    #[test]
    fn unsupported_byday_and_bymonthday_together() {
        let err = parse(
            &["RRULE:FREQ=MONTHLY;BYDAY=MO;BYMONTHDAY=1"],
            d(2026, 5, 16),
        )
        .unwrap_err();
        assert!(matches!(err, RruleError::Unsupported(_)));
    }

    #[test]
    fn unsupported_monthly_byday_multiple_weekdays() {
        let err = parse(
            &["RRULE:FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=1"],
            d(2026, 5, 16),
        )
        .unwrap_err();
        assert!(matches!(err, RruleError::Unsupported(_)));
    }

    #[test]
    fn invalid_missing_freq() {
        let err = parse(&["RRULE:INTERVAL=2"], d(2026, 5, 16)).unwrap_err();
        assert!(matches!(err, RruleError::Invalid(_)));
    }

    #[test]
    fn invalid_interval_zero() {
        let err = parse(&["RRULE:FREQ=DAILY;INTERVAL=0"], d(2026, 5, 16)).unwrap_err();
        assert!(matches!(err, RruleError::Invalid(_)));
    }

    #[test]
    fn invalid_count_zero() {
        let err = parse(&["RRULE:FREQ=DAILY;COUNT=0"], d(2026, 5, 16)).unwrap_err();
        assert!(matches!(err, RruleError::Invalid(_)));
    }

    #[test]
    fn invalid_count_above_max() {
        let err = parse(&["RRULE:FREQ=DAILY;COUNT=99999"], d(2026, 5, 16)).unwrap_err();
        match err {
            RruleError::Invalid(msg) => assert!(msg.contains("COUNT")),
            _ => panic!("expected Invalid"),
        }
    }

    #[test]
    fn invalid_exdate_over_limit() {
        // 1001 個の EXDATE をカンマ区切りで作る
        let dates: Vec<String> = (1..=1001)
            .map(|i| format!("2026{:02}{:02}", (i / 28) + 1, (i % 28) + 1))
            .collect();
        let exdate_line = format!("EXDATE:{}", dates.join(","));
        let err = parse(
            &["RRULE:FREQ=DAILY", exdate_line.as_str()],
            d(2026, 5, 16),
        )
        .unwrap_err();
        assert!(matches!(err, RruleError::Invalid(_)));
    }

    #[test]
    fn interval_1_normalized_to_none() {
        let r = parse(&["RRULE:FREQ=DAILY;INTERVAL=1"], d(2026, 5, 16)).unwrap();
        assert_eq!(r.pattern_data.interval, None);
    }
}
