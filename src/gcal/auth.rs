//! OAuth 2.0 Loopback Redirect + PKCE による Google Calendar への認証。
//!
//! - credential: 同梱の client_id/client_secret (ビルド時環境変数で埋め込み) を
//!   デフォルトとし、`~/.config/ytasky/gcal.json` があればそちらで上書きする
//! - token: `~/.config/ytasky/gcal_token.json` に保存する
//! - 同時ログインは `~/.config/ytasky/gcal_login.lock` で排他

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::PathBuf;
use std::time::Duration;

const AUTH_URI_DEFAULT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URI_DEFAULT: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);

/// ビルド時に `YTASKY_GCAL_CLIENT_ID` / `YTASKY_GCAL_CLIENT_SECRET` を指定すると埋め込まれる。
/// 未指定なら `~/.config/ytasky/gcal.json` が必須。
const BUNDLED_CLIENT_ID: Option<&str> = option_env!("YTASKY_GCAL_CLIENT_ID");
const BUNDLED_CLIENT_SECRET: Option<&str> = option_env!("YTASKY_GCAL_CLIENT_SECRET");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_auth_uri")]
    pub auth_uri: String,
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_auth_uri() -> String {
    AUTH_URI_DEFAULT.to_string()
}

fn default_token_uri() -> String {
    TOKEN_URI_DEFAULT.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub scope: String,
}

impl Token {
    pub fn is_expired(&self, now_epoch: i64) -> bool {
        now_epoch >= self.expires_at - 30
    }
}

// ---- パス解決 -----------------------------------------------------------------

fn config_dir() -> Result<PathBuf> {
    crate::recurrence::config_dir()
}

fn credential_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("gcal.json"))
}

fn token_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("gcal_token.json"))
}

fn lock_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("gcal_login.lock"))
}

// ---- 公開 API -----------------------------------------------------------------

/// OAuth フローを実行して token を取得・保存する。
pub fn login() -> Result<()> {
    let _lock = acquire_login_lock()?;
    let credential = load_credential()?;
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("ローカルサーバー起動失敗: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .context("ローカルサーバーのポート取得失敗")?;
    let redirect_uri = format!("http://127.0.0.1:{port}/cb");

    let url = build_authorize_url(&credential, &redirect_uri, &challenge, &state);
    // state を含む URL は同マシン上のローカルプロセスから観測されると
    // 偽コールバック攻撃の素材になる。eprintln は state を伏字化し、
    // 完全 URL はブラウザにのみ渡す。
    eprintln!(
        "ブラウザで認可してください: {}",
        redact_state_in_url(&url)
    );
    if let Err(e) = webbrowser::open(&url) {
        eprintln!(
            "ブラウザを自動起動できませんでした ({e})。設定で許可した上で再試行してください。"
        );
        eprintln!(
            "(注意: ブラウザに渡されるべき state を含む完全 URL は安全のため stderr に出力しません)"
        );
    }

    let code = run_loopback_callback(&server, &state, CALLBACK_TIMEOUT)?;
    let token = exchange_code(&credential, &code, &verifier, &redirect_uri)?;
    save_token(&token)?;
    eprintln!("認証成功。token を保存しました。");
    Ok(())
}

/// 保存済み token を削除する。
pub fn logout() -> Result<()> {
    let path = token_path()?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("削除失敗: {}", path.display()))?;
        eprintln!("token を削除した: {}", path.display());
    } else {
        eprintln!("token は保存されていない");
    }
    Ok(())
}

/// 有効な access token を返す。期限切れなら refresh、無ければエラー。
pub fn get_valid_token() -> Result<String> {
    let mut token = load_token()?.context("未認証。`ytasky gcal-login` を先に実行してください")?;
    let now = chrono::Utc::now().timestamp();
    if token.is_expired(now) {
        let credential = load_credential()?;
        let refresh = token
            .refresh_token
            .as_deref()
            .context("refresh_token が無い。再ログインが必要")?;
        let refreshed = refresh_access_token(&credential, refresh)?;
        // refresh レスポンスは refresh_token を含まないことがある → 既存値を維持
        let merged = Token {
            refresh_token: refreshed.refresh_token.clone().or(token.refresh_token.clone()),
            ..refreshed
        };
        save_token(&merged)?;
        token = merged;
    }
    Ok(token.access_token)
}

// ---- credential / token I/O ---------------------------------------------------

fn load_credential() -> Result<Credential> {
    let path = credential_path()?;
    if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("credential 読込失敗: {}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("credential JSON が不正: {}", path.display()))?;
        // Google Cloud Console の JSON は {"installed": {...}} or {"web": {...}} 形式
        let inner = value
            .get("installed")
            .or_else(|| value.get("web"))
            .cloned()
            .unwrap_or(value);
        let cred: Credential = serde_json::from_value(inner)
            .with_context(|| format!("credential JSON が不正: {}", path.display()))?;
        validate_oauth_endpoints(&cred)?;
        return Ok(cred);
    }
    if let (Some(id), Some(secret)) = (BUNDLED_CLIENT_ID, BUNDLED_CLIENT_SECRET) {
        if !id.is_empty() && !secret.is_empty() {
            return Ok(Credential {
                client_id: id.to_string(),
                client_secret: secret.to_string(),
                auth_uri: AUTH_URI_DEFAULT.to_string(),
                token_uri: TOKEN_URI_DEFAULT.to_string(),
            });
        }
    }
    bail!(
        "Google OAuth credential が見つかりません。\n\
         1) ytasky 同梱版を使う場合: ビルド時に YTASKY_GCAL_CLIENT_ID/SECRET を設定\n\
         2) 自前で用意する場合: {} に Google Cloud Console から取得した JSON を配置\n\
            (Desktop app タイプの OAuth client)",
        credential_path()?.display()
    )
}

/// 悪意ある gcal.json で `auth_uri` / `token_uri` を攻撃者管理 URL に
/// 差し替えられると `code` / `code_verifier` / `client_secret` が漏洩する。
/// Google 公式エンドポイントの prefix にマッチすることを必須にする。
fn validate_oauth_endpoints(cred: &Credential) -> Result<()> {
    const ALLOWED_AUTH_PREFIXES: &[&str] = &[
        "https://accounts.google.com/",
    ];
    const ALLOWED_TOKEN_PREFIXES: &[&str] = &[
        "https://oauth2.googleapis.com/",
        "https://accounts.google.com/",
    ];
    if !ALLOWED_AUTH_PREFIXES
        .iter()
        .any(|p| cred.auth_uri.starts_with(p))
    {
        bail!(
            "credential.auth_uri が許可された prefix に一致しません: {}",
            cred.auth_uri
        );
    }
    if !ALLOWED_TOKEN_PREFIXES
        .iter()
        .any(|p| cred.token_uri.starts_with(p))
    {
        bail!(
            "credential.token_uri が許可された prefix に一致しません: {}",
            cred.token_uri
        );
    }
    Ok(())
}

/// token を `~/.config/ytasky/gcal_token.json` に保存する。
///
/// セキュリティ:
/// - **Unix**: `chmod 600` で所有者のみアクセス可
/// - **Windows**: ユーザープロファイル配下の ACL に依存。共有環境では別ユーザーが
///   読み取れるリスクが残る。詳細は README の Google Calendar 連携節を参照。
fn save_token(token: &Token) -> Result<()> {
    let path = token_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(token)?;
    std::fs::write(&path, raw).with_context(|| format!("token 保存失敗: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            eprintln!(
                "ytasky: warning: token ファイルの権限設定に失敗: {} ({e})",
                path.display()
            );
        }
    }
    Ok(())
}

fn load_token() -> Result<Option<Token>> {
    let path = token_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("token 読込失敗: {}", path.display()))?;
    let token: Token = serde_json::from_str(&raw)
        .with_context(|| format!("token JSON が不正: {}", path.display()))?;
    Ok(Some(token))
}

// ---- PKCE / state / URL -------------------------------------------------------

/// (code_verifier, code_challenge) を生成する。
fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = sha2::Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

fn generate_state() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn build_authorize_url(
    credential: &Credential,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let mut url = url::Url::parse(&credential.auth_uri).expect("auth_uri は有効な URL");
    url.query_pairs_mut()
        .append_pair("client_id", &credential.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", state);
    url.to_string()
}

// ---- Loopback サーバー --------------------------------------------------------

fn run_loopback_callback(
    server: &tiny_http::Server,
    expected_state: &str,
    timeout: Duration,
) -> Result<String> {
    let request = server
        .recv_timeout(timeout)
        .map_err(|e| anyhow::anyhow!("ローカルサーバー受信エラー: {e}"))?
        .context("認可がタイムアウト (120秒)")?;

    let url = format!("http://localhost{}", request.url());
    let parsed = url::Url::parse(&url).context("コールバック URL の解析失敗")?;
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut error: Option<String> = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    // ユーザー誤認を防ぐため、レスポンス送信より先に state を検証する
    // (CSRF / 偽コールバックの場合に「認証完了」を表示しない)
    if let Some(err) = error {
        respond_plain(request, "認証失敗。ターミナルを確認してください。");
        bail!("OAuth エラー: {err}");
    }
    let received_state = match state {
        Some(s) => s,
        None => {
            respond_plain(request, "認証失敗。ターミナルを確認してください。");
            bail!("state パラメータが無い");
        }
    };
    if !constant_time_eq(&received_state, expected_state) {
        respond_plain(request, "認証失敗。ターミナルを確認してください。");
        bail!("state 不一致 (CSRF 検知)");
    }
    let code = match code {
        Some(c) => c,
        None => {
            respond_plain(request, "認証失敗。ターミナルを確認してください。");
            bail!("code パラメータが無い");
        }
    };
    respond_plain(request, "認証完了。このタブを閉じてください。");
    Ok(code)
}

fn respond_plain(request: tiny_http::Request, body: &str) {
    let response = tiny_http::Response::from_string(body).with_header(
        tiny_http::Header::from_bytes(
            &b"Content-Type"[..],
            &b"text/plain; charset=utf-8"[..],
        )
        .unwrap(),
    );
    let _ = request.respond(response);
}

// ---- token 交換 / refresh -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

fn exchange_code(
    credential: &Credential,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Token> {
    let params = [
        ("client_id", credential.client_id.as_str()),
        ("client_secret", credential.client_secret.as_str()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    post_token(&credential.token_uri, &params)
}

fn refresh_access_token(credential: &Credential, refresh_token: &str) -> Result<Token> {
    let params = [
        ("client_id", credential.client_id.as_str()),
        ("client_secret", credential.client_secret.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    post_token(&credential.token_uri, &params)
}

fn post_token(token_uri: &str, params: &[(&str, &str)]) -> Result<Token> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let resp = client.post(token_uri).form(params).send()?;
    let status = resp.status();
    let body = resp.text()?;
    if !status.is_success() {
        // エラーレスポンス body は Google のエラー JSON で token は含まれないが
        // 念のため先頭 200 文字に truncate して長大な漏洩を防ぐ
        bail!(
            "token endpoint エラー {status}: {}",
            truncate_for_log(&body, 200)
        );
    }
    // 200 OK の body には access_token / refresh_token が平文で含まれる。
    // パース失敗時に body を anyhow にそのまま乗せると token が伝搬するため
    // status のみを残す。
    let parsed: TokenResponse = serde_json::from_str(&body)
        .with_context(|| format!("token JSON 解析失敗 (status={status})"))?;
    let expires_at = chrono::Utc::now().timestamp() + parsed.expires_in;
    Ok(Token {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at,
        scope: parsed.scope.unwrap_or_else(|| SCOPE.to_string()),
    })
}

/// 認可 URL の `state=...` を `state=REDACTED` に置換する (表示用)。
fn redact_state_in_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    loop {
        match rest.find("state=") {
            Some(pos) => {
                out.push_str(&rest[..pos + "state=".len()]);
                out.push_str("REDACTED");
                let after = &rest[pos + "state=".len()..];
                match after.find('&') {
                    Some(amp) => rest = &after[amp..],
                    None => return out,
                }
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// state パラメータの定数時間比較。文字列の長さも長さ自体で先漏れ
/// しないよう、長さが異なれば直ちに不一致を返す。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 長大なレスポンス本文をエラーメッセージに乗せる際の安全な truncate。
fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}... ({} 文字省略)", s.chars().count() - max)
}

// ---- ロック -------------------------------------------------------------------

fn acquire_login_lock() -> Result<LoginLock> {
    use fs2::FileExt as _;
    let path = lock_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)
        .with_context(|| format!("lock ファイルを開けない: {}", path.display()))?;
    file.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("別の ytasky プロセスが認証中です"))?;
    Ok(LoginLock { file })
}

struct LoginLock {
    #[allow(dead_code)]
    file: std::fs::File,
}

impl Drop for LoginLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

// ---- テスト -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_match_spec() {
        let (verifier, challenge) = generate_pkce();
        // RFC 7636: 43 <= len(verifier) <= 128, unreserved chars
        assert!((43..=128).contains(&verifier.len()));
        assert!(verifier.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~'
        }));
        // SHA256 → base64url no padding = 43 chars
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }

    #[test]
    fn state_is_url_safe() {
        let s = generate_state();
        assert!(s.len() >= 32);
        assert!(s.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_'
        }));
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let cred = Credential {
            client_id: "abc".into(),
            client_secret: "shh".into(),
            auth_uri: AUTH_URI_DEFAULT.into(),
            token_uri: TOKEN_URI_DEFAULT.into(),
        };
        let url = build_authorize_url(&cred, "http://127.0.0.1:1234/cb", "CHAL", "STATE");
        for needle in [
            "client_id=abc",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcb",
            "response_type=code",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.readonly",
            "code_challenge=CHAL",
            "code_challenge_method=S256",
            "access_type=offline",
            "prompt=consent",
            "state=STATE",
        ] {
            assert!(url.contains(needle), "URL に '{needle}' が無い: {url}");
        }
    }

    #[test]
    fn token_roundtrip() {
        let token = Token {
            access_token: "AT".into(),
            refresh_token: Some("RT".into()),
            expires_at: 1747400000,
            scope: SCOPE.into(),
        };
        let json = serde_json::to_string(&token).unwrap();
        let back: Token = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, "AT");
        assert_eq!(back.refresh_token.as_deref(), Some("RT"));
        assert_eq!(back.expires_at, 1747400000);
    }

    #[test]
    fn token_is_expired_with_skew() {
        let t = Token {
            access_token: "AT".into(),
            refresh_token: None,
            expires_at: 1000,
            scope: "".into(),
        };
        // 30 秒のスキューで余裕を持って expired
        assert!(t.is_expired(970));
        assert!(t.is_expired(1000));
        // 31 秒以上前ならまだ有効
        assert!(!t.is_expired(969));
    }

    #[test]
    fn constant_time_eq_matches_normal_eq() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc", "abc1"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn redact_state_replaces_value_only() {
        let url = "https://example.com/auth?client_id=foo&state=SECRET_VALUE&prompt=consent";
        let red = redact_state_in_url(url);
        assert!(red.contains("state=REDACTED"));
        assert!(!red.contains("SECRET_VALUE"));
        assert!(red.contains("client_id=foo"));
        assert!(red.contains("prompt=consent"));
    }

    #[test]
    fn redact_state_handles_state_at_end() {
        let url = "https://example.com/auth?client_id=foo&state=SECRET";
        let red = redact_state_in_url(url);
        assert!(red.ends_with("state=REDACTED"));
        assert!(!red.contains("SECRET"));
    }

    #[test]
    fn validate_oauth_endpoints_accepts_google() {
        let cred = Credential {
            client_id: "x".into(),
            client_secret: "y".into(),
            auth_uri: AUTH_URI_DEFAULT.into(),
            token_uri: TOKEN_URI_DEFAULT.into(),
        };
        validate_oauth_endpoints(&cred).unwrap();
    }

    #[test]
    fn validate_oauth_endpoints_rejects_evil_auth_uri() {
        let cred = Credential {
            client_id: "x".into(),
            client_secret: "y".into(),
            auth_uri: "https://evil.example.com/auth".into(),
            token_uri: TOKEN_URI_DEFAULT.into(),
        };
        let err = validate_oauth_endpoints(&cred).unwrap_err();
        assert!(err.to_string().contains("auth_uri"));
    }

    #[test]
    fn validate_oauth_endpoints_rejects_evil_token_uri() {
        let cred = Credential {
            client_id: "x".into(),
            client_secret: "y".into(),
            auth_uri: AUTH_URI_DEFAULT.into(),
            token_uri: "https://evil.example.com/token".into(),
        };
        let err = validate_oauth_endpoints(&cred).unwrap_err();
        assert!(err.to_string().contains("token_uri"));
    }

    #[test]
    fn credential_parses_installed_wrapper() {
        // Google Cloud Console の JSON 形式を直接書ける検証用 unit (ファイル経由ではない)
        let raw = r#"{"installed":{"client_id":"X","client_secret":"Y","auth_uri":"https://a","token_uri":"https://t"}}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let inner = value.get("installed").cloned().unwrap();
        let cred: Credential = serde_json::from_value(inner).unwrap();
        assert_eq!(cred.client_id, "X");
        assert_eq!(cred.client_secret, "Y");
        assert_eq!(cred.auth_uri, "https://a");
    }
}
