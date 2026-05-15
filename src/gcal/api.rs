//! Google Calendar API v3 の薄いクライアント。
//!
//! - events.list: 期間内の全イベントを取得 (pageToken でページネーション完走)
//! - events.instances: 単一の繰り返しイベントの instance を取得 (フォールバック用)
//! - calendarList.list: ユーザーのカレンダー一覧

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use std::time::Duration;

use crate::gcal::types::{CalendarList, Event, EventList};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: u32 = 2500;

/// 認可ヘッダ付き reqwest クライアントを返す。
fn make_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("HTTP クライアント構築失敗")
}

/// `calendars/{id}/events` を timeMin..timeMax で取得し pageToken でページネーション完走する。
///
/// `single_events=true` を渡すと RRULE をサーバ側で個別 instance に展開して返す。
pub fn list_events(
    access_token: &str,
    calendar_id: &str,
    time_min_rfc3339: &str,
    time_max_rfc3339: &str,
    single_events: bool,
) -> Result<Vec<Event>> {
    let client = make_client()?;
    let mut all: Vec<Event> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let url = format!(
            "{API_BASE}/calendars/{}/events",
            urlencoding(calendar_id)
        );
        let mut req = client
            .get(&url)
            .bearer_auth(access_token)
            .query(&[
                ("timeMin", time_min_rfc3339),
                ("timeMax", time_max_rfc3339),
                ("singleEvents", if single_events { "true" } else { "false" }),
                ("maxResults", &MAX_RESULTS.to_string()),
                ("showDeleted", "false"),
            ]);
        if let Some(ref t) = page_token {
            req = req.query(&[("pageToken", t.as_str())]);
        }
        let resp = req.send().context("events.list 送信失敗")?;
        let status = resp.status();
        let body = resp.text().context("events.list レスポンス読込失敗")?;
        if !status.is_success() {
            bail!(
                "events.list エラー {status}: {}",
                truncate_for_log(&body, 200)
            );
        }
        let parsed: EventList = serde_json::from_str(&body)
            .with_context(|| format!("events.list レスポンス JSON 解析失敗 (status={status})"))?;
        all.extend(parsed.items);
        match parsed.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(all)
}

/// 単一の繰り返しイベントの instance を取得 (未対応 RRULE のフォールバック用)。
pub fn list_event_instances(
    access_token: &str,
    calendar_id: &str,
    event_id: &str,
    time_min_rfc3339: &str,
    time_max_rfc3339: &str,
) -> Result<Vec<Event>> {
    let client = make_client()?;
    let mut all: Vec<Event> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let url = format!(
            "{API_BASE}/calendars/{}/events/{}/instances",
            urlencoding(calendar_id),
            urlencoding(event_id),
        );
        let mut req = client
            .get(&url)
            .bearer_auth(access_token)
            .query(&[
                ("timeMin", time_min_rfc3339),
                ("timeMax", time_max_rfc3339),
                ("maxResults", &MAX_RESULTS.to_string()),
                ("showDeleted", "false"),
            ]);
        if let Some(ref t) = page_token {
            req = req.query(&[("pageToken", t.as_str())]);
        }
        let resp = req.send().context("events.instances 送信失敗")?;
        let status = resp.status();
        let body = resp.text().context("events.instances レスポンス読込失敗")?;
        if !status.is_success() {
            bail!(
                "events.instances エラー {status}: {}",
                truncate_for_log(&body, 200)
            );
        }
        let parsed: EventList = serde_json::from_str(&body)
            .with_context(|| format!("events.instances JSON 解析失敗 (status={status})"))?;
        all.extend(parsed.items);
        match parsed.next_page_token {
            Some(t) if !t.is_empty() => page_token = Some(t),
            _ => break,
        }
    }
    Ok(all)
}

/// `users/me/calendarList` を取得する (`--calendar` 選択肢提示用)。
pub fn list_calendars(access_token: &str) -> Result<CalendarList> {
    let client = make_client()?;
    let url = format!("{API_BASE}/users/me/calendarList");
    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .context("calendarList.list 送信失敗")?;
    let status = resp.status();
    let body = resp.text().context("calendarList.list レスポンス読込失敗")?;
    if !status.is_success() {
        bail!(
            "calendarList.list エラー {status}: {}",
            truncate_for_log(&body, 200)
        );
    }
    serde_json::from_str::<CalendarList>(&body)
        .with_context(|| format!("calendarList.list JSON 解析失敗 (status={status})"))
}

/// 長大なレスポンス本文をエラーメッセージに乗せる際の安全な truncate。
fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}... ({} 文字省略)", s.chars().count() - max)
}

/// path segment 用の percent encoding。
/// `form_urlencoded::byte_serialize` は application/x-www-form-urlencoded 用で
/// スペースを `+` にエンコードしてしまい、URL path には不適切。
/// `NON_ALPHANUMERIC` で `@`, `/`, space, `%` などすべてを `%XX` 化する。
fn urlencoding(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ---- テスト -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_keeps_alphanumeric() {
        assert_eq!(urlencoding("primary"), "primary");
    }

    #[test]
    fn urlencoding_escapes_at() {
        // NON_ALPHANUMERIC は '.' も escape する
        assert_eq!(urlencoding("work@example.com"), "work%40example%2Ecom");
    }

    #[test]
    fn urlencoding_escapes_space_as_percent_20_not_plus() {
        // form_urlencoded は ' ' を '+' にしてしまうが、path には不適
        assert_eq!(urlencoding("a b"), "a%20b");
    }

    #[test]
    fn urlencoding_escapes_slash() {
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }
}
