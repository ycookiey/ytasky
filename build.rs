//! ビルド時に `gcal_client.json` があれば読み込み、Google OAuth の
//! client_id / client_secret を `YTASKY_GCAL_CLIENT_ID` /
//! `YTASKY_GCAL_CLIENT_SECRET` 環境変数としてコンパイルに埋め込む。
//!
//! - `gcal_client.json` は Google Cloud Console の Desktop app credential
//!   (`{"installed":{...}}` 形式) をそのまま置く。`.gitignore` 済み。
//! - ファイルが無ければ何もしない (auth.rs 側の option_env! が None になり、
//!   ~/.config/ytasky/gcal.json での上書きが必須となる)。

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
    // Google Cloud Console の JSON は {"installed":{...}} or {"web":{...}}
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
