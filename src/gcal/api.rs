//! Google Calendar API v3 の薄いクライアント。
//!
//! - events.list: 期間内の全イベントを取得 (pageToken でページネーション完走)
//! - events.instances: 単一の繰り返しイベントの instance を取得 (フォールバック用)
//! - calendarList.list: ユーザーのカレンダー一覧

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder, Response};
use std::time::Duration;

use crate::gcal::types::{Event, EventList};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: u32 = 2500;
/// ページネーション暴走防止。1ページ最大 2500 件 × 200 ページ = 50万件で十分。
const MAX_PAGES: u32 = 200;
/// 429 Retry-After を尊重して 1 回だけリトライする上限秒数。
const MAX_RETRY_AFTER_SECS: u64 = 30;

/// 認可ヘッダ付き reqwest クライアントを返す。
fn make_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("HTTP クライアント構築失敗")
}

/// 429 (Too Many Requests) の場合 Retry-After を尊重して 1 度だけ retry する。
/// 5xx と 401 はそのまま返す (401 は呼び出し側で refresh を判断)。
fn send_with_retry(make_req: impl Fn() -> RequestBuilder) -> Result<Response> {
    let resp = make_req().send().context("HTTP 送信失敗")?;
    if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok(resp);
    }
    // Retry-After は秒数または HTTP-date。秒数のみハンドリングし、暴走を避けるため上限を設ける
    let wait_secs = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1)
        .min(MAX_RETRY_AFTER_SECS);
    std::thread::sleep(Duration::from_secs(wait_secs));
    make_req().send().context("HTTP 送信失敗 (retry)")
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
    let url = format!("{API_BASE}/calendars/{}/events", urlencoding(calendar_id));
    let max_results = MAX_RESULTS.to_string();
    for page in 0..MAX_PAGES {
        let pt = page_token.clone();
        let resp = send_with_retry(|| {
            let mut req = client
                .get(&url)
                .bearer_auth(access_token)
                .query(&[
                    ("timeMin", time_min_rfc3339),
                    ("timeMax", time_max_rfc3339),
                    ("singleEvents", if single_events { "true" } else { "false" }),
                    ("maxResults", max_results.as_str()),
                    ("showDeleted", "false"),
                ]);
            if let Some(ref t) = pt {
                req = req.query(&[("pageToken", t.as_str())]);
            }
            req
        })?;
        let status = resp.status();
        let body = resp.text().context("events.list レスポンス読込失敗")?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!(
                "events.list 401 Unauthorized: access_token が失効。`ytasky gcal-login` で再認証してください"
            );
        }
        if !status.is_success() {
            bail!(
                "events.list エラー {status}: {}",
                crate::gcal::truncate_for_log(&body, 200)
            );
        }
        let parsed: EventList = serde_json::from_str(&body)
            .with_context(|| format!("events.list レスポンス JSON 解析失敗 (status={status})"))?;
        all.extend(parsed.items);
        match parsed.next_page_token {
            Some(t) if !t.is_empty() => {
                // 同一 token を返し続ける異常ループを防ぐ
                if page_token.as_deref() == Some(t.as_str()) {
                    break;
                }
                page_token = Some(t);
            }
            _ => return Ok(all),
        }
        if page + 1 == MAX_PAGES {
            bail!(
                "events.list がページネーション上限 ({MAX_PAGES} ページ) に到達。範囲を狭めて再試行してください"
            );
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
    let url = format!(
        "{API_BASE}/calendars/{}/events/{}/instances",
        urlencoding(calendar_id),
        urlencoding(event_id),
    );
    let max_results = MAX_RESULTS.to_string();
    for page in 0..MAX_PAGES {
        let pt = page_token.clone();
        let resp = send_with_retry(|| {
            let mut req = client
                .get(&url)
                .bearer_auth(access_token)
                .query(&[
                    ("timeMin", time_min_rfc3339),
                    ("timeMax", time_max_rfc3339),
                    ("maxResults", max_results.as_str()),
                    ("showDeleted", "false"),
                ]);
            if let Some(ref t) = pt {
                req = req.query(&[("pageToken", t.as_str())]);
            }
            req
        })?;
        let status = resp.status();
        let body = resp.text().context("events.instances レスポンス読込失敗")?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!(
                "events.instances 401 Unauthorized: access_token が失効。`ytasky gcal-login` で再認証してください"
            );
        }
        if !status.is_success() {
            bail!(
                "events.instances エラー {status}: {}",
                crate::gcal::truncate_for_log(&body, 200)
            );
        }
        let parsed: EventList = serde_json::from_str(&body)
            .with_context(|| format!("events.instances JSON 解析失敗 (status={status})"))?;
        all.extend(parsed.items);
        match parsed.next_page_token {
            Some(t) if !t.is_empty() => {
                if page_token.as_deref() == Some(t.as_str()) {
                    break;
                }
                page_token = Some(t);
            }
            _ => return Ok(all),
        }
        if page + 1 == MAX_PAGES {
            bail!(
                "events.instances がページネーション上限 ({MAX_PAGES} ページ) に到達"
            );
        }
    }
    Ok(all)
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
