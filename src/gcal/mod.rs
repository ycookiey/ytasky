//! Google Calendar 連携モジュール。
//!
//! 公開 API:
//! - `auth::get_valid_token`: 必要なら refresh して有効な access token を返す
//! - `auth::login`: OAuth フローを実行して token を取得する
//! - `auth::logout`: 保存済み token を削除する
//! - `api`: Calendar API v3 (events.list / events.instances / calendarList.list)
//! - `tz`: RFC3339 ↔ (NaiveDate, fixed_start_min)
//! - `types`: API レスポンスの serde 型

pub mod api;
pub mod auth;
pub mod types;
pub mod tz;
