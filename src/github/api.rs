//! GitHub Search API (`GET /search/issues`) の薄いクライアント。
//!
//! - 認証済みで rate limit 30 req/min（lexical search）
//! - 1ページ最大 100 件、pagination は `Link` ヘッダの `rel="next"` を辿る
//! - 検索結果は最大 1000 件まで（API の硬制限）
//! - `advanced_search` は 2025-09-04 以降デフォルト有効のためパラメータ指定は不要
//!   （クエリ内のスペースは AND として解釈される）

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;
use std::time::Duration;

use crate::github::auth::GithubToken;
use crate::github::types::{Issue, SearchResult};

const SEARCH_URL: &str = "https://api.github.com/search/issues";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const PER_PAGE: u32 = 100;
/// 100 件 × 10 ページ = 1000 件（API の検索結果上限）。
const MAX_PAGES: u32 = 10;
/// 429/403 の Retry-After を尊重して 1 回だけ retry する上限秒数。
const MAX_RETRY_AFTER_SECS: u64 = 30;
/// GitHub API は User-Agent 必須。
const USER_AGENT: &str = concat!("ytasky/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "2022-11-28";

fn make_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP クライアント構築失敗")
}

/// 429（および Retry-After 付きの 403 = secondary rate limit）の場合、
/// Retry-After を尊重して 1 度だけ retry する。
fn send_with_retry(make_req: impl Fn() -> RequestBuilder) -> Result<Response> {
    let resp = make_req().send().context("HTTP 送信失敗")?;
    let status = resp.status();
    let is_rate_limited = status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::FORBIDDEN
            && resp.headers().contains_key(reqwest::header::RETRY_AFTER));
    if !is_rate_limited {
        return Ok(resp);
    }
    let wait_secs = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(parse_retry_after)
        .unwrap_or(1)
        .min(MAX_RETRY_AFTER_SECS);
    std::thread::sleep(Duration::from_secs(wait_secs));
    make_req().send().context("HTTP 送信失敗 (retry)")
}

/// 検索結果と非致命 warning のペア。呼び出し側が warnings を errors に集約できる。
#[derive(Debug, Default)]
pub struct SearchOutcome {
    pub issues: Vec<Issue>,
    /// 1000 件上限超え・incomplete_results 等、致命ではないが可視化したい状態。
    /// TUI では eprintln すると alt-screen を破壊するため戻り値経由で返す。
    pub warnings: Vec<String>,
}

/// 検索クエリに一致する issue を `Link` ヘッダで pagination 完走して取得する。
pub fn search_issues(token: &GithubToken, query: &str) -> Result<SearchOutcome> {
    let client = make_client()?;
    let mut all: Vec<Issue> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let per_page = PER_PAGE.to_string();

    // 1ページ目は q/per_page を query で指定。2ページ目以降は Link の next URL を
    // そのまま使う（next URL に q/per_page/page が全て含まれる）。
    let mut next_url: Option<String> = Some(SEARCH_URL.to_string());
    let mut is_first = true;

    for page in 0..MAX_PAGES {
        let Some(url) = next_url.take() else {
            break;
        };
        let resp = send_with_retry(|| {
            let mut req = client
                .get(&url)
                .bearer_auth(token.as_str())
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", API_VERSION);
            if is_first {
                req = req.query(&[("q", query), ("per_page", per_page.as_str())]);
            }
            req
        })?;
        is_first = false;

        let status = resp.status();
        // Link ヘッダは body 読み込み前に取り出す。
        let link_next = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_next_link);

        let body = resp.text().context("search/issues レスポンス読込失敗")?;
        match status {
            reqwest::StatusCode::UNAUTHORIZED => bail!(
                "search/issues 401 Unauthorized: トークンが無効です。`gh auth login` で再ログインしてください"
            ),
            reqwest::StatusCode::UNPROCESSABLE_ENTITY => bail!(
                "search/issues 422: 検索クエリが不正です: {}",
                crate::github::truncate_for_log(&body, 200)
            ),
            s if !s.is_success() => bail!(
                "search/issues エラー {s}: {}",
                crate::github::truncate_for_log(&body, 200)
            ),
            _ => {}
        }

        let parsed: SearchResult = serde_json::from_str(&body)
            .with_context(|| format!("search/issues JSON 解析失敗 (status={status})"))?;

        // 1000 件上限を超えるクエリは取りこぼす。1ページ目で警告する。
        if page == 0 && parsed.total_count > 1000 {
            warnings.push(format!(
                "query \"{query}\": マッチ数 {} 件が API 上限 1000 件を超えています。クエリを分割してください",
                parsed.total_count
            ));
        }
        // GitHub 側がタイムアウトで結果を打ち切った場合。
        if parsed.incomplete_results {
            warnings.push(format!(
                "query \"{query}\": 検索結果が不完全です (GitHub 側のタイムアウト)"
            ));
        }

        all.extend(parsed.items);
        next_url = link_next;
        if next_url.is_none() {
            break;
        }
    }
    Ok(SearchOutcome {
        issues: all,
        warnings,
    })
}

/// Retry-After ヘッダ値 (RFC7231) を秒に変換する。
/// - delta-seconds 形式: 数値そのまま
/// - HTTP-date 形式: 現在時刻との差分を秒に (past 日付や parse 失敗は 5s の保守 fallback)
fn parse_retry_after(v: &str) -> u64 {
    if let Ok(secs) = v.parse::<u64>() {
        return secs;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(v) {
        let now = chrono::Utc::now();
        let diff = (dt.with_timezone(&chrono::Utc) - now).num_seconds();
        if diff > 0 {
            return diff as u64;
        }
    }
    // parse 不能 or 過去日付 → 保守的な 5 秒
    5
}

/// issue の状態（open / closed）。REST API のレスポンス `state` フィールドに対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// `GET /repos/:owner/:repo/issues/:number` で単一 issue の state を取る。
/// 404 は `Ok(None)`（repo 削除・権限喪失等: 勝手に done 化しない判断のため呼び出し側で無視できる）。
/// closed → done 検出用途。search API とは別レート枠（REST 5000 req/hour）。
pub fn fetch_issue_state(
    token: &GithubToken,
    owner: &str,
    repo: &str,
    number: u64,
) -> Result<Option<IssueState>> {
    let client = make_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}");
    let resp = send_with_retry(|| {
        client
            .get(&url)
            .bearer_auth(token.as_str())
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
    })?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body = resp.text().context("issues/# レスポンス読込失敗")?;
    match status {
        reqwest::StatusCode::UNAUTHORIZED => bail!(
            "issues/# 401 Unauthorized: トークンが無効です。`gh auth login` で再ログインしてください"
        ),
        s if !s.is_success() => bail!(
            "issues/# エラー {s}: {}",
            crate::github::truncate_for_log(&body, 200)
        ),
        _ => {}
    }
    #[derive(Deserialize)]
    struct StateOnly {
        state: String,
    }
    let parsed: StateOnly = serde_json::from_str(&body)
        .with_context(|| format!("issues/# JSON 解析失敗 (status={status})"))?;
    Ok(Some(if parsed.state == "closed" {
        IssueState::Closed
    } else {
        IssueState::Open
    }))
}

/// `Link` ヘッダから `rel="next"` の URL を取り出す。
/// 例: `<https://api.github.com/search/issues?q=x&page=2>; rel="next", <...>; rel="last"`
fn parse_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let mut segs = part.split(';');
        let Some(url_seg) = segs.next().map(str::trim) else {
            continue;
        };
        let is_next = segs.any(|s| {
            let s = s.trim();
            s == r#"rel="next""# || s == "rel=next"
        });
        if !is_next {
            continue;
        }
        // `< >` 欠落セグメントは skip して次の rel="next" 候補を試す
        // (旧実装は ? で関数全体が None を返し、後続の valid link を捨てていた)。
        if let Some(url) = url_seg
            .strip_prefix('<')
            .and_then(|u| u.strip_suffix('>'))
        {
            return Some(url.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_next_link_basic() {
        let h = r#"<https://api.github.com/search/issues?q=x&page=2>; rel="next", <https://api.github.com/search/issues?q=x&page=5>; rel="last""#;
        assert_eq!(
            parse_next_link(h).as_deref(),
            Some("https://api.github.com/search/issues?q=x&page=2")
        );
    }

    #[test]
    fn parse_next_link_absent() {
        // 最終ページは next を含まない
        let h = r#"<https://api.github.com/search/issues?q=x&page=1>; rel="prev", <https://api.github.com/search/issues?q=x&page=1>; rel="first""#;
        assert_eq!(parse_next_link(h), None);
    }

    #[test]
    fn parse_next_link_empty() {
        assert_eq!(parse_next_link(""), None);
    }

    #[test]
    fn parse_retry_after_numeric() {
        assert_eq!(parse_retry_after("30"), 30);
        assert_eq!(parse_retry_after("0"), 0);
    }

    #[test]
    fn parse_retry_after_past_http_date_returns_fallback() {
        // 過去日付は fallback (5s) にフォールバック
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2000 07:28:00 GMT"),
            5
        );
    }

    #[test]
    fn parse_retry_after_invalid_returns_fallback() {
        assert_eq!(parse_retry_after("garbage"), 5);
    }

    #[test]
    fn parse_next_link_skips_malformed_and_picks_valid() {
        // 最初の rel=next セグメントは <> 無しで malformed、2 番目が valid
        // `?` で早期 return していた旧実装ではここが None を返していた。
        let h = r#"https://api.github.com/search/issues?q=x&page=2; rel="next", <https://api.github.com/search/issues?q=x&page=3>; rel="next""#;
        assert_eq!(
            parse_next_link(h).as_deref(),
            Some("https://api.github.com/search/issues?q=x&page=3")
        );
    }
}
