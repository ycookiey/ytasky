//! Google Calendar API v3 のレスポンス型 (必要なフィールドのみ)。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventList {
    #[serde(default)]
    pub items: Vec<Event>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    /// "confirmed" / "tentative" / "cancelled"
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub start: Option<EventDateTime>,
    pub end: Option<EventDateTime>,
    /// RRULE / EXDATE / RDATE の配列 (例: ["RRULE:FREQ=WEEKLY;BYDAY=MO"])
    #[serde(default)]
    pub recurrence: Option<Vec<String>>,
    /// 親イベント (繰り返し instance の場合)
    #[serde(default)]
    pub recurring_event_id: Option<String>,
    /// instance 元の開始時刻 (recurring_event_id がある場合)
    #[serde(default)]
    pub original_start_time: Option<EventDateTime>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDateTime {
    /// RFC3339 形式 (時刻指定イベント)
    #[serde(default)]
    pub date_time: Option<String>,
    /// YYYY-MM-DD (終日イベント)
    #[serde(default)]
    pub date: Option<String>,
    /// IANA tz 名 (例: "Asia/Tokyo")
    #[serde(default)]
    pub time_zone: Option<String>,
}

impl Event {
    /// 終日イベントか
    pub fn is_all_day(&self) -> bool {
        self.start
            .as_ref()
            .and_then(|s| s.date.as_deref())
            .is_some()
    }

    /// cancelled
    pub fn is_cancelled(&self) -> bool {
        self.status.as_deref() == Some("cancelled")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarList {
    #[serde(default)]
    pub items: Vec<CalendarListEntry>,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListEntry {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub primary: Option<bool>,
    #[serde(default)]
    pub time_zone: Option<String>,
}

// ---- テスト -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_list_minimal() {
        let raw = r#"{
            "items": [
                {
                    "id": "abc",
                    "status": "confirmed",
                    "summary": "Meeting",
                    "start": {"dateTime": "2026-05-16T10:00:00+09:00", "timeZone": "Asia/Tokyo"},
                    "end": {"dateTime": "2026-05-16T11:00:00+09:00", "timeZone": "Asia/Tokyo"}
                }
            ]
        }"#;
        let list: EventList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.items.len(), 1);
        let ev = &list.items[0];
        assert_eq!(ev.id, "abc");
        assert_eq!(ev.summary.as_deref(), Some("Meeting"));
        assert!(!ev.is_all_day());
        assert!(!ev.is_cancelled());
    }

    #[test]
    fn parse_all_day_and_cancelled() {
        let raw = r#"{
            "items": [
                {"id":"a","status":"cancelled","start":{"date":"2026-05-16"}},
                {"id":"b","start":{"date":"2026-05-17"}}
            ]
        }"#;
        let list: EventList = serde_json::from_str(raw).unwrap();
        assert!(list.items[0].is_cancelled());
        assert!(list.items[1].is_all_day());
        assert!(!list.items[1].is_cancelled());
    }

    #[test]
    fn parse_recurrence_and_instance() {
        let raw = r#"{
            "items": [
                {
                    "id": "parent",
                    "summary": "Weekly",
                    "start": {"dateTime":"2026-05-16T10:00:00+09:00"},
                    "end": {"dateTime":"2026-05-16T11:00:00+09:00"},
                    "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"]
                },
                {
                    "id": "parent_20260518T010000Z",
                    "recurringEventId": "parent",
                    "originalStartTime": {"dateTime":"2026-05-18T10:00:00+09:00"},
                    "start": {"dateTime":"2026-05-18T10:30:00+09:00"},
                    "end": {"dateTime":"2026-05-18T11:30:00+09:00"}
                }
            ]
        }"#;
        let list: EventList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(
            list.items[0].recurrence.as_ref().unwrap()[0],
            "RRULE:FREQ=WEEKLY;BYDAY=MO,WE"
        );
        assert_eq!(
            list.items[1].recurring_event_id.as_deref(),
            Some("parent")
        );
    }

    #[test]
    fn parse_calendar_list() {
        let raw = r#"{
            "items": [
                {"id":"primary","summary":"Me","primary":true,"timeZone":"Asia/Tokyo"},
                {"id":"work@example.com","summary":"Work"}
            ]
        }"#;
        let list: CalendarList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[0].primary, Some(true));
        assert_eq!(list.items[1].time_zone, None);
    }

    #[test]
    fn next_page_token_is_camel_case() {
        let raw = r#"{"items":[],"nextPageToken":"PAGE2"}"#;
        let list: EventList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.next_page_token.as_deref(), Some("PAGE2"));
    }
}
