//! Minimal client for Google Calendar API v3.
//!
//! - events.list: all events in a range, following pageToken to the last page
//! - events.instances: instances of one recurring event, used when its RRULE is unsupported

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder, Response};
use std::time::Duration;

use crate::gcal::types::{Event, EventList};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULTS: u32 = 2500;
/// Guard against runaway pagination. 2500 events per page x 200 pages is far above real use.
const MAX_PAGES: u32 = 200;
/// Upper bound for honoring Retry-After on the single 429 retry.
const MAX_RETRY_AFTER_SECS: u64 = 30;

fn make_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("HTTP クライアント構築失敗")
}

/// Retries once on 429, honoring Retry-After. 5xx and 401 are returned as is;
/// the caller decides whether to refresh on 401.
fn send_with_retry(make_req: impl Fn() -> RequestBuilder) -> Result<Response> {
    let resp = make_req().send().context("HTTP 送信失敗")?;
    if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok(resp);
    }
    // Retry-After is seconds or an HTTP-date. Only seconds are parsed, and the wait is capped.
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

/// Fetches `calendars/{id}/events` for timeMin..timeMax, following pageToken to the last page.
///
/// With `single_events=true` the server expands RRULEs into individual instances.
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
                // Stop if the server keeps returning the same token.
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

/// Instances of one recurring event, for RRULEs the import cannot convert.
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

/// Percent-encoding for a path segment. `form_urlencoded::byte_serialize` targets
/// application/x-www-form-urlencoded and turns spaces into `+`, which is wrong in a path.
fn urlencoding(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ---- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_keeps_alphanumeric() {
        assert_eq!(urlencoding("primary"), "primary");
    }

    #[test]
    fn urlencoding_escapes_at() {
        // NON_ALPHANUMERIC also escapes '.'
        assert_eq!(urlencoding("work@example.com"), "work%40example%2Ecom");
    }

    #[test]
    fn urlencoding_escapes_space_as_percent_20_not_plus() {
        // form_urlencoded would turn ' ' into '+', which is wrong in a path
        assert_eq!(urlencoding("a b"), "a%20b");
    }

    #[test]
    fn urlencoding_escapes_slash() {
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }
}
