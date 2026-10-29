//! Imports Google Calendar events into ytasky tasks and recurrences.
//!
//! - single event -> `tasks` upsert
//! - recurring event with a supported RRULE -> `recurrences` upsert
//! - recurring event with an unsupported RRULE -> expanded with `events.instances`,
//!   each instance upserted into `tasks`
//! - cancelled events, all-day events and instances of a recurring event are skipped
//!
//! EXDATE / RDATE are not applied yet; the summary only counts them in
//! `skipped_exdates` / `skipped_rdates`.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use ybasey::Database;

use crate::gcal::api;
use crate::gcal::auth;
use crate::gcal::rrule::{self, ParsedRecurrence, RruleError};
use crate::gcal::types::Event;
use crate::gcal::tz as gtz;
use crate::text::sanitize_text;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub calendar_id: String,
    pub category_id: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            calendar_id: "primary".into(),
            category_id: "6".into(), // 6 is the seeded personal-time category
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
    // An instance expanded from a parent event is covered by the parent.
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
                // Fall back to expanding the individual instances.
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

/// Builds the external_id `gcal:{calendar_id}:{event_id}`.
///
/// Calendar IDs do not normally contain `:`, and if one did, `splitn(3, ':')` would still
/// recover the event ID as the remainder.
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

/// Upserts a recurrence and returns whether it was inserted, so the summary can count
/// created and updated separately.
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
    // Always pass end_date, as a value or as null, so that removing UNTIL in
    // Google Calendar clears it here too.
    // The database's null sentinel is the string "_".
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

// ---- tests -------------------------------------------------------------------
//
// import_range calls the HTTP API, so testing it needs a mock. These tests cover the
// parts of the routing that are pure functions.

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
            start: Some(EventDateTime {
                date_time: Some(start.into()),
                date: None,
            }),
            end: Some(EventDateTime {
                date_time: Some(end.into()),
                date: None,
            }),
            recurrence: None,
            recurring_event_id: None,
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

        // second run: change the title -> update
        let ev_v2 = make_event(
            "evt1",
            "Meeting (renamed)",
            "2026-05-16T10:00:00+09:00",
            "2026-05-16T11:00:00+09:00",
        );
        upsert_single_task(&mut db, &ev_v2, &opts, jst(), &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(summary.updated, 1);

        // still one task
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
        // ANSI escape sequences start with \x1b (ESC) and are removed entirely
        assert_eq!(sanitize_text("foo\x1b[31mbar\x1b[0m"), "foo[31mbar[0m");
        // NUL bytes are removed
        assert_eq!(sanitize_text("a\0b"), "ab");
        // tabs stay
        assert_eq!(sanitize_text("a\tb"), "a\tb");
    }

    #[test]
    fn sanitize_text_strips_unicode_bidi_and_format() {
        // RTL override (U+202E) can disguise displayed text
        assert_eq!(sanitize_text("safe\u{202E}evil.exe"), "safeevil.exe");
        // LTR mark (U+200E)
        assert_eq!(sanitize_text("a\u{200E}b"), "ab");
        // ZWJ (U+200D), ZWSP (U+200B)
        assert_eq!(sanitize_text("a\u{200B}b\u{200D}c"), "abc");
        // Line/Paragraph Separator
        assert_eq!(sanitize_text("a\u{2028}b\u{2029}c"), "abc");
        // ordinary Japanese text and symbols stay
        assert_eq!(sanitize_text("会議 → A 室 (10:00)"), "会議 → A 室 (10:00)");
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
