//! Embeds a Google OAuth client into the binary when `gcal_client.json` exists at build
//! time: client_id / client_secret become the `YTASKY_GCAL_CLIENT_ID` /
//! `YTASKY_GCAL_CLIENT_SECRET` compile-time env vars.
//!
//! - `gcal_client.json` is the Desktop app credential downloaded from Google Cloud
//!   Console (`{"installed":{...}}`). It is git-ignored so the secret stays out of the repo.
//! - Without the file nothing is embedded, and users must provide `gcal.json` in the
//!   config dir.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=gcal_client.json");
    println!("cargo:rerun-if-changed=build.rs");

    let path = Path::new("gcal_client.json");
    if !path.exists() {
        return;
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("cargo:warning=gcal_client.json を読めません: {e}");
            return;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            println!("cargo:warning=gcal_client.json の JSON 解析に失敗: {e}");
            return;
        }
    };
    // Google Cloud Console wraps the credential in {"installed":{...}} or {"web":{...}}.
    let inner = value
        .get("installed")
        .or_else(|| value.get("web"))
        .unwrap_or(&value);

    if let Some(id) = inner.get("client_id").and_then(|v| v.as_str()) {
        println!("cargo:rustc-env=YTASKY_GCAL_CLIENT_ID={id}");
    } else {
        println!("cargo:warning=gcal_client.json に client_id がありません");
    }
    if let Some(secret) = inner.get("client_secret").and_then(|v| v.as_str()) {
        println!("cargo:rustc-env=YTASKY_GCAL_CLIENT_SECRET={secret}");
    } else {
        println!("cargo:warning=gcal_client.json に client_secret がありません");
    }
}
