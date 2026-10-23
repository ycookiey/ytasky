//! Authentication: reuses the token of the `gh` CLI. ytasky does not store the token.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// Access token from `gh`. `Debug` is implemented by hand to keep the token out of logs and errors.
#[derive(Clone)]
pub struct GithubToken(String);

impl GithubToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GithubToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GithubToken([REDACTED])")
    }
}

/// Gets the token by running `gh auth token`.
///
/// Storing, refreshing and scoping the token are left to `gh`.
/// Error messages never include the token, which is on stdout.
pub fn get_token() -> Result<GithubToken> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .context("`gh` の実行に失敗しました。GitHub CLI をインストールして `gh auth login` してください")?;

    if !output.status.success() {
        // stderr holds no token; pass its reason (such as not logged in) on to the message.
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`gh auth token` が失敗しました（未ログインの可能性）。`gh auth login` でログインしてください: {}",
            stderr.trim()
        );
    }

    let token = String::from_utf8(output.stdout)
        .context("`gh auth token` の出力が UTF-8 ではありません")?
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("`gh auth token` が空のトークンを返しました。`gh auth login` でログインしてください");
    }
    Ok(GithubToken(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_masks_token() {
        let t = GithubToken("ghp_secret_value".to_string());
        let dbg = format!("{t:?}");
        assert!(!dbg.contains("ghp_secret_value"));
        assert!(dbg.contains("REDACTED"));
    }

    #[test]
    fn as_str_returns_token() {
        let t = GithubToken("abc".to_string());
        assert_eq!(t.as_str(), "abc");
    }
}
