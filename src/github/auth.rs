//! 認証: `gh` CLI のトークンを流用する。ytasky 自身はトークンを保管しない。

use anyhow::{Context, Result, bail};
use std::process::Command;

/// `gh` のアクセストークン。`Debug` を手動実装し、ログ・エラーに平文が漏れないようマスクする。
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

/// `gh auth token` を実行してトークンを取得する。
///
/// トークン管理（保管・更新・スコープ）は `gh` に委譲する。
/// エラーメッセージにはトークンを含めない（stdout 側にあるトークンは捨てる）。
pub fn get_token() -> Result<GithubToken> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .context("`gh` の実行に失敗しました。GitHub CLI をインストールして `gh auth login` してください")?;

    if !output.status.success() {
        // stderr にトークンは含まれない。未ログイン等の理由を案内に乗せる。
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
