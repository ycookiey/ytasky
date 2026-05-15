//! GCal events → ytasky tasks/recurrences の import オーケストレータ。
//!
//! - `import_range`: 期間内の events を取得し振り分け
//! - 単発 → `tasks` upsert
//! - 繰り返し (RRULE サポート内) → `recurrences` upsert
//! - 繰り返し (RRULE サポート外) → `events.instances` で展開し各 instance を `tasks` upsert
//! - cancelled / all-day / 親無し instance はスキップ
//!
//! EXDATE / RDATE の完全反映は将来拡張 (設計書 §12)。本フェーズでは
//! summary の `skipped_exdates` / `skipped_rdates` でカウントのみ。

use anyhow::{Context, Result};
use chrono::NaiveDate;
use ybasey::Database;

use crate::gcal::api;
use crate::gcal::auth;
use crate::gcal::rrule::{self, ParsedRecurrence, RruleError};
use crate::gcal::types::Event;
use crate::gcal::tz as gtz;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub calendar_id: String,
    pub category_id: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            calendar_id: "primary".into(),
            category_id: "6".into(), // 6 = 身支度・自由時間 (≒ personal)
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub skipped_exdates: usize,
    pub skipped_rdates: usize,
    pub errors: Vec<String>,
}

/// 指定期間の events を import する。
pub fn import_range(
    db: &mut Database,
    from: NaiveDate,
    to: NaiveDate,
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    let access_token = auth::get_valid_token()?;
    let data_dir = crate::db::data_dir()?;
    let tz = gtz::read_meta_tz(&data_dir);

    let time_min = gtz::date_to_rfc3339_at_midnight(from, tz)?;
    let time_max = gtz::date_to_rfc3339_at_midnight(to + chrono::Duration::days(1), tz)?;

    let events =
        api::list_events(&access_token, &opts.calendar_id, &time_min, &time_max, false)?;

    let mut summary = ImportSummary::default();
    for ev in &events {
        match handle_event(db, &access_token, ev, &time_min, &time_max, opts, tz, &mut summary) {
            Ok(()) => {}
            Err(e) => {
                summary
                    .errors
                    .push(format!("event {}: {}", ev.id, e));
                summary.skipped += 1;
            }
        }
    }
    Ok(summary)
}

// ---- per-event handler --------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_event(
    db: &mut Database,
    access_token: &str,
    event: &Event,
    time_min: &str,
    time_max: &str,
    opts: &ImportOptions,
    tz: chrono_tz::Tz,
    summary: &mut ImportSummary,
) -> Result<()> {
    if event.is_cancelled() {
        summary.skipped += 1;
        return Ok(());
    }
    if event.is_all_day() {
        summary.skipped += 1;
        return Ok(());
    }
    // 個別 instance (親イベントから展開済み) は親側で処理されるためスキップ
    if event.recurring_event_id.is_some() {
        summary.skipped += 1;
        return Ok(());
    }

    if let Some(rules) = &event.recurrence {
        let start_dt = event
            .start
            .as_ref()
            .and_then(|s| s.date_time.as_deref())
            .context("recurrence event の start.dateTime が無い")?;
        let (start_date, start_min) = gtz::rfc3339_to_local_minute(start_dt, tz)?;
        let end_dt = event
            .end
            .as_ref()
            .and_then(|s| s.date_time.as_deref())
            .context("recurrence event の end.dateTime が無い")?;
        let duration = gtz::duration_minutes(start_dt, end_dt)?;

        match rrule::parse_recurrence_rules(rules, start_date) {
            Ok(parsed) => {
                let inserted = upsert_recurrence(
                    db, event, opts, start_date, start_min, duration, &parsed,
                )?;
                summary_count_recurrence(summary, &parsed);
                if inserted {
                    summary.created += 1;
                } else {
                    summary.updated += 1;
                }
            }
            Err(RruleError::Unsupported(_)) => {
                // 個別 instance 展開フォールバック
                let instances = api::list_event_instances(
                    access_token,
                    &opts.calendar_id,
                    &event.id,
                    time_min,
                    time_max,
                )?;
                for inst in &instances {
                    if inst.is_cancelled() {
                        summary.skipped += 1;
                        continue;
                    }
                    if inst.is_all_day() {
                        summary.skipped += 1;
                        continue;
                    }
                    upsert_single_task(db, inst, opts, tz, summary)?;
                }
            }
            Err(RruleError::Invalid(msg)) => {
                summary.errors.push(format!("event {}: {}", event.id, msg));
                summary.skipped += 1;
            }
        }
    } else {
        upsert_single_task(db, event, opts, tz, summary)?;
    }
    Ok(())
}

fn summary_count_recurrence(summary: &mut ImportSummary, parsed: &ParsedRecurrence) {
    if !parsed.exdates.is_empty() {
        summary.skipped_exdates += parsed.exdates.len();
    }
    if !parsed.rdates.is_empty() {
        summary.skipped_rdates += parsed.rdates.len();
    }
}

// ---- upsert helpers -----------------------------------------------------------

/// `gcal:{calendar_id}:{event_id}` の形式で external_id を生成する。
///
/// calendar_id 中の `:` は Google Calendar API の仕様上通常含まれないが、
/// 将来の逆分解が必要になった場合は `splitn(3, ':')` で先頭 2 要素を
/// 取って残りを event_id として扱える設計にしてある。
fn external_id(calendar_id: &str, event_id: &str) -> String {
    format!("gcal:{calendar_id}:{event_id}")
}

fn upsert_single_task(
    db: &mut Database,
    event: &Event,
    opts: &ImportOptions,
    tz: chrono_tz::Tz,
    summary: &mut ImportSummary,
) -> Result<()> {
    let start_dt = event
        .start
        .as_ref()
        .and_then(|s| s.date_time.as_deref())
        .context("event の start.dateTime が無い")?;
    let end_dt = event
        .end
        .as_ref()
        .and_then(|s| s.date_time.as_deref())
        .context("event の end.dateTime が無い")?;
    let (date, fixed_start) = gtz::rfc3339_to_local_minute(start_dt, tz)?;
    let duration = gtz::duration_minutes(start_dt, end_dt)?;
    let title = sanitize_text(event.summary.as_deref().unwrap_or("(無題)"));
    let ext = external_id(&opts.calendar_id, &event.id);
    let exists = task_exists_by_external_id(db, &ext)?;

    let mut fields = vec![
        ("date".into(), gtz::date_to_string(date)),
        ("title".into(), title),
        ("category_id".into(), opts.category_id.clone()),
        ("duration_min".into(), duration.to_string()),
        ("status".into(), "todo".into()),
        ("fixed_start".into(), fixed_start.to_string()),
        ("is_backlog".into(), "0".into()),
        ("external_id".into(), ext.clone()),
    ];
    if !exists {
        fields.push(("sort_order".into(), "0".into()));
    }

    let (_id, inserted) = db
        .upsert("tasks", "external_id", &ext, fields)
        .context("tasks upsert 失敗")?;
    if inserted {
        summary.created += 1;
    } else {
        summary.updated += 1;
    }
    Ok(())
}

/// recurrences を upsert する。inserted/updated を `bool` で返し、
/// 呼び出し側で summary の created/updated を区別できるようにする。
fn upsert_recurrence(
    db: &mut Database,
    event: &Event,
    opts: &ImportOptions,
    start_date: NaiveDate,
    fixed_start: i32,
    duration: i32,
    parsed: &ParsedRecurrence,
) -> Result<bool> {
    let title = sanitize_text(
        event
            .summary
            .as_deref()
            .unwrap_or("(無題)"),
    );
    let ext = external_id(&opts.calendar_id, &event.id);
    let pattern_data_json = serde_json::to_string(&parsed.pattern_data)
        .context("pattern_data JSON 化失敗")?;

    let mut fields = vec![
        ("title".into(), title),
        ("category_id".into(), opts.category_id.clone()),
        ("duration_min".into(), duration.to_string()),
        ("fixed_start".into(), fixed_start.to_string()),
        ("pattern".into(), parsed.pattern.clone()),
        ("pattern_data".into(), pattern_data_json),
        ("start_date".into(), gtz::date_to_string(start_date)),
        ("external_id".into(), ext.clone()),
    ];
    // end_date は Some/None の両方を明示的に渡し、GCal 側で UNTIL が
    // 削除された場合に ytasky 側もクリアされるようにする。
    // ybasey の Null sentinel は "_" 文字列。
    let end_value = parsed
        .end_date
        .map(gtz::date_to_string)
        .unwrap_or_else(|| "_".to_string());
    fields.push(("end_date".into(), end_value));

    let (_id, inserted) = db
        .upsert("recurrences", "external_id", &ext, fields)
        .context("recurrences upsert 失敗")?;
    Ok(inserted)
}

fn task_exists_by_external_id(db: &Database, ext: &str) -> Result<bool> {
    let table = db.table("tasks").context("tasks テーブルが無い")?;
    Ok(!table.find_by_field("external_id", ext).is_empty())
}

/// GCal の summary / description には ANSI escape (`\x1b[...`) や NUL
/// が含まれうる。TUI / ログでの偽装表示や ybasey 側で文字列フィールドが
/// 壊れるのを避けるため、import 時点で制御文字を除去する。
fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect()
}

// ---- テスト -------------------------------------------------------------------
//
// 注: import_range は HTTP に依存するため統合テストにはモックが要る。
// ここでは振り分けロジックの一部を純粋関数として検証する。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcal::types::{Event, EventDateTime};

    fn setup_in_memory_db() -> (tempfile::TempDir, Database) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("_meta"),
            "tz: Asia/Tokyo\nview_sync: async\n",
        )
        .unwrap();
        let mut db = Database::open(tmp.path(), Some("test-gcal-import")).unwrap();
        crate::init::apply_schema(&mut db).unwrap();
        (tmp, db)
    }

    fn jst() -> chrono_tz::Tz {
        "Asia/Tokyo".parse().unwrap()
    }

    fn make_event(id: &str, summary: &str, start: &str, end: &str) -> Event {
        Event {
            id: id.into(),
            status: Some("confirmed".into()),
            summary: Some(summary.into()),
            description: None,
            start: Some(EventDateTime {
                date_time: Some(start.into()),
                date: None,
                time_zone: Some("Asia/Tokyo".into()),
            }),
            end: Some(EventDateTime {
                date_time: Some(end.into()),
                date: None,
                time_zone: Some("Asia/Tokyo".into()),
            }),
            recurrence: None,
            recurring_event_id: None,
            original_start_time: None,
        }
    }

    #[test]
    fn external_id_format() {
        assert_eq!(external_id("primary", "abc123"), "gcal:primary:abc123");
        assert_eq!(
            external_id("work@example.com", "evt_1"),
            "gcal:work@example.com:evt_1"
        );
    }

    #[test]
    fn import_options_default() {
        let o = ImportOptions::default();
        assert_eq!(o.calendar_id, "primary");
        assert_eq!(o.category_id, "6");
    }

    #[test]
    fn upsert_single_task_inserts_then_updates() {
        let (_tmp, mut db) = setup_in_memory_db();
        let opts = ImportOptions::default();
        let mut summary = ImportSummary::default();

        let ev_v1 = make_event(
            "evt1",
            "Meeting",
            "2026-05-16T10:00:00+09:00",
            "2026-05-16T10:30:00+09:00",
        );
        upsert_single_task(&mut db, &ev_v1, &opts, jst(), &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(summary.updated, 0);

        // 2 回目: title を変えて update
        let ev_v2 = make_event(
            "evt1",
            "Meeting (renamed)",
            "2026-05-16T10:00:00+09:00",
            "2026-05-16T11:00:00+09:00",
        );
        upsert_single_task(&mut db, &ev_v2, &opts, jst(), &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(summary.updated, 1);

        // tasks は 1 件のみ
        let count = db.query("tasks", "| count").unwrap();
        assert!(count.contains("count=1"), "got {count}");
    }

    #[test]
    fn upsert_recurrence_inserts() {
        let (_tmp, mut db) = setup_in_memory_db();
        let opts = ImportOptions::default();
        let parsed = ParsedRecurrence {
            pattern: "weekly".into(),
            pattern_data: crate::model::PatternData {
                days: Some(vec![1, 3]),
                interval: None,
                setpos: None,
            },
            end_date: None,
            exdates: vec![],
            rdates: vec![],
        };
        let ev = Event {
            id: "rec1".into(),
            summary: Some("Standup".into()),
            ..make_event("rec1", "Standup", "2026-05-18T09:00:00+09:00", "2026-05-18T09:15:00+09:00")
        };
        upsert_recurrence(&mut db, &ev, &opts, NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(), 540, 15, &parsed).unwrap();

        let count = db.query("recurrences", "| count").unwrap();
        assert!(count.contains("count=1"), "got {count}");
    }

    #[test]
    fn upsert_recurrence_returns_inserted_then_updated() {
        let (_tmp, mut db) = setup_in_memory_db();
        let opts = ImportOptions::default();
        let parsed = ParsedRecurrence {
            pattern: "daily".into(),
            pattern_data: Default::default(),
            end_date: None,
            exdates: vec![],
            rdates: vec![],
        };
        let ev = make_event(
            "rec1",
            "Standup",
            "2026-05-18T09:00:00+09:00",
            "2026-05-18T09:15:00+09:00",
        );
        let inserted_1 = upsert_recurrence(
            &mut db,
            &ev,
            &opts,
            NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(),
            540,
            15,
            &parsed,
        )
        .unwrap();
        assert!(inserted_1, "first call should report inserted");
        let inserted_2 = upsert_recurrence(
            &mut db,
            &ev,
            &opts,
            NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(),
            540,
            15,
            &parsed,
        )
        .unwrap();
        assert!(!inserted_2, "second call should report updated");
    }

    #[test]
    fn sanitize_text_strips_control_characters() {
        assert_eq!(sanitize_text("hello"), "hello");
        // ANSI escape は \x1b (ESC) で始まる制御シーケンス → 全部除去
        assert_eq!(sanitize_text("foo\x1b[31mbar\x1b[0m"), "foo[31mbar[0m");
        // NUL バイトの除去
        assert_eq!(sanitize_text("a\0b"), "ab");
        // タブは残す
        assert_eq!(sanitize_text("a\tb"), "a\tb");
    }

    #[test]
    fn upsert_recurrence_is_idempotent_via_external_id() {
        let (_tmp, mut db) = setup_in_memory_db();
        let opts = ImportOptions::default();
        let parsed = ParsedRecurrence {
            pattern: "daily".into(),
            pattern_data: Default::default(),
            end_date: None,
            exdates: vec![],
            rdates: vec![],
        };
        let ev = make_event(
            "rec_dup",
            "Daily",
            "2026-05-18T09:00:00+09:00",
            "2026-05-18T09:30:00+09:00",
        );
        upsert_recurrence(&mut db, &ev, &opts, NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(), 540, 30, &parsed).unwrap();
        upsert_recurrence(&mut db, &ev, &opts, NaiveDate::from_ymd_opt(2026, 5, 18).unwrap(), 540, 30, &parsed).unwrap();

        let count = db.query("recurrences", "| count").unwrap();
        assert!(count.contains("count=1"), "got {count}");
    }
}
