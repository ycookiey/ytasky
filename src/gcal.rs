use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use oauth2::TokenResponse;
use rusqlite::Connection;
use serde::Deserialize;

use crate::db;

// --- API response types ---

#[derive(Debug, Deserialize)]
struct CalendarListResponse {
    items: Option<Vec<CalendarListEntry>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarListEntry {
    pub id: String,
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    items: Option<Vec<GCalEvent>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GCalEvent {
    id: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    start: Option<GCalDateTime>,
    end: Option<GCalDateTime>,
}

#[derive(Debug, Deserialize)]
struct GCalDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

// --- Sync types ---

pub struct SyncConfig {
    pub access_token: String,
    pub calendars: Vec<(String, String)>, // (calendar_id, name)
    #[allow(dead_code)]
    pub sync_days: i32,
    pub time_min: String,
    pub time_max: String,
}

pub struct FetchedEvent {
    pub calendar_id: String,
    pub gcal_event_id: String,
    pub date: String,
    pub title: String,
    pub start_min: Option<i32>,
    pub duration_min: i32,
    pub is_all_day: bool,
}

pub struct FetchResult {
    pub calendar_id: String,
    pub date: String,
    pub events: Vec<FetchedEvent>,
}

pub struct SyncResult {
    pub events_synced: usize,
    pub events_deleted: usize,
}

// --- OAuth2 ---

pub async fn run_auth_flow(conn: &Connection, client_id: &str, client_secret: &str) -> Result<()> {
    use oauth2::{
        AuthUrl, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope,
        TokenUrl, basic::BasicClient,
    };

    let client = BasicClient::new(ClientId::new(client_id.to_string()))
        .set_client_secret(ClientSecret::new(client_secret.to_string()))
        .set_auth_uri(AuthUrl::new(
            "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
        )?)
        .set_token_uri(TokenUrl::new(
            "https://oauth2.googleapis.com/token".to_string(),
        )?)
        .set_redirect_uri(RedirectUrl::new(
            "http://127.0.0.1:8855/callback".to_string(),
        )?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let (auth_url, _csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(
            "https://www.googleapis.com/auth/calendar.readonly".to_string(),
        ))
        .set_pkce_challenge(pkce_challenge)
        .url();

    println!("ブラウザで認証画面を開く...");
    open::that(auth_url.as_str()).context("ブラウザを開けない")?;

    let code = wait_for_callback().await?;

    println!("トークンを取得中...");
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let token_result = client
        .exchange_code(oauth2::AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await
        .context("トークン交換に失敗")?;

    let access_token = token_result.access_token().secret().to_string();
    let refresh_token = token_result
        .refresh_token()
        .map(|t| t.secret().to_string())
        .context("refresh_tokenが取得できない (access_type=offlineが必要)")?;

    let expiry = token_result
        .expires_in()
        .map(|d: std::time::Duration| {
            chrono::Utc::now()
                .checked_add_signed(Duration::seconds(d.as_secs() as i64))
                .unwrap_or_else(chrono::Utc::now)
                .to_rfc3339()
        })
        .unwrap_or_default();

    db::gcal_set_config(conn, "client_id", client_id)?;
    db::gcal_set_config(conn, "client_secret", client_secret)?;
    db::gcal_set_config(conn, "access_token", &access_token)?;
    db::gcal_set_config(conn, "refresh_token", &refresh_token)?;
    db::gcal_set_config(conn, "token_expiry", &expiry)?;

    println!("認証完了");
    Ok(())
}

async fn wait_for_callback() -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:8855").await?;
    println!("認証コールバックを待機中 (127.0.0.1:8855)...");

    let (mut stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let code = request
        .lines()
        .next()
        .and_then(|line| {
            let path = line.split_whitespace().nth(1)?;
            let url = url_parse_query(path);
            url.into_iter()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v)
        })
        .context("認可コードが見つからない")?;

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h2>認証完了！このタブを閉じてください。</h2></body></html>";
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;

    Ok(code)
}

fn url_parse_query(path: &str) -> Vec<(String, String)> {
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

pub async fn refresh_access_token(conn: &Connection) -> Result<String> {
    let client_id = db::gcal_get_config(conn, "client_id")?
        .context("client_idが未設定")?;
    let client_secret = db::gcal_get_config(conn, "client_secret")?
        .context("client_secretが未設定")?;
    let refresh_token = db::gcal_get_config(conn, "refresh_token")?
        .context("refresh_tokenが未設定")?;

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = http_client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("トークンリフレッシュ失敗: {text}");
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in: Option<u64>,
    }

    let token_resp: TokenResponse = resp.json().await?;
    db::gcal_set_config(conn, "access_token", &token_resp.access_token)?;

    if let Some(expires_in) = token_resp.expires_in {
        let expiry = chrono::Utc::now()
            .checked_add_signed(Duration::seconds(expires_in as i64))
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339();
        db::gcal_set_config(conn, "token_expiry", &expiry)?;
    }

    Ok(token_resp.access_token)
}

pub fn get_valid_token(conn: &Connection) -> Result<Option<String>> {
    let Some(token) = db::gcal_get_config(conn, "access_token")? else {
        return Ok(None);
    };

    if let Some(expiry_str) = db::gcal_get_config(conn, "token_expiry")? {
        if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&expiry_str) {
            if expiry < chrono::Utc::now() + Duration::minutes(5) {
                return Ok(None); // expired, needs refresh
            }
        }
    }

    Ok(Some(token))
}

// --- API calls ---

async fn fetch_calendar_list(token: &str) -> Result<Vec<CalendarListEntry>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut all = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let mut url =
            "https://www.googleapis.com/calendar/v3/users/me/calendarList".to_string();
        if let Some(ref pt) = page_token {
            url.push_str(&format!("?pageToken={pt}"));
        }

        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("CalendarList API error: {text}");
        }

        let body: CalendarListResponse = resp.json().await?;
        if let Some(items) = body.items {
            all.extend(items);
        }
        match body.next_page_token {
            Some(pt) => page_token = Some(pt),
            None => break,
        }
    }

    Ok(all)
}

async fn fetch_events(
    token: &str,
    calendar_id: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<GCalEvent>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut all = Vec::new();
    let mut page_token: Option<String> = None;
    let encoded_cal = urlencoding(calendar_id);

    loop {
        let mut url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events?timeMin={}&timeMax={}&singleEvents=true&orderBy=startTime&maxResults=250",
            encoded_cal, time_min, time_max
        );
        if let Some(ref pt) = page_token {
            url.push_str(&format!("&pageToken={pt}"));
        }

        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("Events API error ({}): {text}", calendar_id);
        }

        let body: EventsResponse = resp.json().await?;
        if let Some(items) = body.items {
            all.extend(items);
        }
        match body.next_page_token {
            Some(pt) => page_token = Some(pt),
            None => break,
        }
    }

    Ok(all)
}

fn urlencoding(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
    }
    encoded
}

// --- Sync (3-phase) ---

pub fn sync_prepare(conn: &Connection) -> Result<SyncConfig> {
    let access_token = get_valid_token(conn)?
        .context("有効なアクセストークンがない")?;

    let calendars = db::gcal_load_calendars(conn)?;
    let enabled: Vec<(String, String)> = calendars
        .into_iter()
        .filter(|c| c.enabled)
        .map(|c| (c.calendar_id, c.name))
        .collect();

    let sync_days = db::gcal_get_config(conn, "sync_days")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(30i32);

    let today = chrono::Local::now().date_naive();
    let time_min = format!("{}T00:00:00Z", today);
    let time_max = format!("{}T23:59:59Z", today + Duration::days(sync_days as i64));

    Ok(SyncConfig {
        access_token,
        calendars: enabled,
        sync_days,
        time_min,
        time_max,
    })
}

pub async fn sync_fetch(config: &SyncConfig) -> Result<Vec<FetchResult>> {
    let mut all_results = Vec::new();

    for (calendar_id, _name) in &config.calendars {
        let events =
            fetch_events(&config.access_token, calendar_id, &config.time_min, &config.time_max)
                .await?;

        let mut by_date: std::collections::HashMap<String, Vec<FetchedEvent>> =
            std::collections::HashMap::new();

        for event in events {
            if event.status.as_deref() == Some("cancelled") {
                continue;
            }
            let Some(event_id) = event.id else { continue };
            let title = event.summary.unwrap_or_else(|| "(無題)".to_string());

            let (start_dt, end_dt) = match (&event.start, &event.end) {
                (Some(s), Some(e)) => (s, e),
                _ => continue,
            };

            if let (Some(start_str), Some(end_str)) =
                (&start_dt.date_time, &end_dt.date_time)
            {
                // Timed event
                let parsed = parse_events_from_datetime(
                    &event_id,
                    calendar_id,
                    &title,
                    start_str,
                    end_str,
                );
                for fe in parsed {
                    by_date.entry(fe.date.clone()).or_default().push(fe);
                }
            } else if let (Some(start_date), Some(end_date)) =
                (&start_dt.date, &end_dt.date)
            {
                // All-day event
                let parsed = parse_all_day_event(
                    &event_id,
                    calendar_id,
                    &title,
                    start_date,
                    end_date,
                );
                for fe in parsed {
                    by_date.entry(fe.date.clone()).or_default().push(fe);
                }
            }
        }

        for (date, events) in by_date {
            all_results.push(FetchResult {
                calendar_id: calendar_id.clone(),
                date,
                events,
            });
        }
    }

    Ok(all_results)
}

pub fn sync_write(conn: &Connection, results: &[FetchResult]) -> Result<SyncResult> {
    let mut events_synced = 0usize;
    let mut events_deleted = 0usize;

    for result in results {
        let current_ids: Vec<String> = result
            .events
            .iter()
            .map(|e| e.gcal_event_id.clone())
            .collect();

        for event in &result.events {
            db::gcal_upsert_event(
                conn,
                &event.gcal_event_id,
                &event.calendar_id,
                &event.date,
                &event.title,
                event.start_min,
                event.duration_min,
                event.is_all_day,
            )?;
            events_synced += 1;
        }

        events_deleted +=
            db::gcal_delete_stale_events(conn, &result.calendar_id, &result.date, &current_ids)?;
    }

    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = db::gcal_set_config(conn, "last_sync", &now);

    Ok(SyncResult {
        events_synced,
        events_deleted,
    })
}

pub async fn sync_all(conn: &Connection) -> Result<SyncResult> {
    // Check if token needs refresh
    if get_valid_token(conn)?.is_none() {
        refresh_access_token(conn).await?;
    }

    let config = sync_prepare(conn)?;

    // If no calendars configured, fetch and auto-enable all
    if config.calendars.is_empty() {
        let cal_list = fetch_calendar_list(&config.access_token).await?;
        for cal in &cal_list {
            let name = cal.summary.as_deref().unwrap_or(&cal.id);
            db::gcal_upsert_calendar(conn, &cal.id, name, true)?;
        }
        // Re-prepare with updated calendars
        let config = sync_prepare(conn)?;
        let results = sync_fetch(&config).await?;
        return sync_write(conn, &results);
    }

    let results = sync_fetch(&config).await?;
    sync_write(conn, &results)
}

pub async fn fetch_and_store_calendars(conn: &Connection) -> Result<Vec<CalendarListEntry>> {
    let token = get_valid_token(conn)?
        .context("有効なアクセストークンがない")?;
    let cal_list = fetch_calendar_list(&token).await?;

    for cal in &cal_list {
        let name = cal.summary.as_deref().unwrap_or(&cal.id);
        // Only insert new calendars, don't overwrite enabled status
        let existing = db::gcal_load_calendars(conn)?;
        let already_exists = existing.iter().any(|c| c.calendar_id == cal.id);
        if !already_exists {
            db::gcal_upsert_calendar(conn, &cal.id, name, true)?;
        }
    }

    Ok(cal_list)
}

// --- Event parsing helpers ---

fn parse_events_from_datetime(
    event_id: &str,
    calendar_id: &str,
    title: &str,
    start_str: &str,
    end_str: &str,
) -> Vec<FetchedEvent> {
    let Some(start_dt) = parse_gcal_datetime(start_str) else {
        return Vec::new();
    };
    let Some(end_dt) = parse_gcal_datetime(end_str) else {
        return Vec::new();
    };

    let start_date = start_dt.date();
    let end_date = end_dt.date();

    if start_date == end_date {
        let start_min = start_dt.time().num_seconds_from_midnight() as i32 / 60;
        let duration = (end_dt - start_dt).num_minutes().max(1) as i32;
        return vec![FetchedEvent {
            calendar_id: calendar_id.to_string(),
            gcal_event_id: event_id.to_string(),
            date: start_date.format("%Y-%m-%d").to_string(),
            title: title.to_string(),
            start_min: Some(start_min),
            duration_min: duration,
            is_all_day: false,
        }];
    }

    // Multi-day timed event: split by day
    let mut results = Vec::new();
    let mut current = start_date;
    while current <= end_date {
        let day_start = if current == start_date {
            start_dt.time().num_seconds_from_midnight() as i32 / 60
        } else {
            0
        };
        let day_end = if current == end_date {
            end_dt.time().num_seconds_from_midnight() as i32 / 60
        } else {
            24 * 60
        };
        let duration = (day_end - day_start).max(0);
        if duration > 0 {
            results.push(FetchedEvent {
                calendar_id: calendar_id.to_string(),
                gcal_event_id: event_id.to_string(),
                date: current.format("%Y-%m-%d").to_string(),
                title: title.to_string(),
                start_min: Some(day_start),
                duration_min: duration,
                is_all_day: false,
            });
        }
        current += Duration::days(1);
    }
    results
}

fn parse_all_day_event(
    event_id: &str,
    calendar_id: &str,
    title: &str,
    start_date_str: &str,
    end_date_str: &str,
) -> Vec<FetchedEvent> {
    let Ok(start) = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d") else {
        return Vec::new();
    };
    let Ok(end) = NaiveDate::parse_from_str(end_date_str, "%Y-%m-%d") else {
        return Vec::new();
    };

    // GCal end date is exclusive for all-day events
    let mut results = Vec::new();
    let mut current = start;
    while current < end {
        results.push(FetchedEvent {
            calendar_id: calendar_id.to_string(),
            gcal_event_id: event_id.to_string(),
            date: current.format("%Y-%m-%d").to_string(),
            title: title.to_string(),
            start_min: None,
            duration_min: 24 * 60,
            is_all_day: true,
        });
        current += Duration::days(1);
    }
    results
}

fn parse_gcal_datetime(s: &str) -> Option<NaiveDateTime> {
    // Format: "2024-01-15T09:00:00+09:00" or "2024-01-15T09:00:00Z"
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local).naive_local())
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
}
