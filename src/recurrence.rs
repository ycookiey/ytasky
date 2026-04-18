//! recurrence.rs — Material 方式の繰り返し展開 (horizon 永続化 + incremental 展開)

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use ybasey::{Database, NewRecord};

use crate::model::Recurrence;

// ---- horizon config -----------------------------------------------------------

pub struct HorizonConfig {
    pub horizon_date: NaiveDate,
    pub years_ahead: u32,
}

impl Default for HorizonConfig {
    fn default() -> Self {
        Self {
            // ファイル不存在時: まだ何も展開していない扱いにする
            horizon_date: chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            years_ahead: 2,
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YTASKY_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::config_dir().context("OS config dir not found")?;
    Ok(base.join("ytasky"))
}

fn horizon_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("recurrence_horizon"))
}

pub fn read_horizon() -> Result<HorizonConfig> {
    let path = horizon_path()?;
    if !path.exists() {
        return Ok(HorizonConfig::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let mut horizon_date: Option<NaiveDate> = None;
    let mut years_ahead: u32 = 2;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "horizon_date" => {
                    horizon_date = NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok();
                }
                "years_ahead" => {
                    years_ahead = v.trim().parse().unwrap_or(2);
                }
                _ => {}
            }
        }
    }
    Ok(HorizonConfig {
        horizon_date: horizon_date.unwrap_or_else(|| HorizonConfig::default().horizon_date),
        years_ahead,
    })
}

pub fn write_horizon(config: &HorizonConfig) -> Result<()> {
    let path = horizon_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = format!(
        "# ytasky recurrence horizon\nhorizon_date: {}\nyears_ahead: {}\n",
        config.horizon_date.format("%Y-%m-%d"),
        config.years_ahead,
    );
    std::fs::write(&path, content)?;
    Ok(())
}

// ---- pattern matching (db.rs から移動) ----------------------------------------

pub fn matches_recurrence_pattern(
    pattern: &str,
    pattern_data: Option<&str>,
    target_date: NaiveDate,
) -> Result<bool> {
    use chrono::Datelike;
    match pattern {
        "daily" => Ok(true),
        "weekly" => {
            let weekday = target_date.weekday().number_from_monday() as u8;
            let days = parse_pattern_days(pattern_data)?;
            Ok(days.contains(&weekday))
        }
        "monthly" => {
            let day = target_date.day() as u8;
            let days = parse_pattern_days(pattern_data)?;
            Ok(days.contains(&day))
        }
        _ => Ok(false),
    }
}

pub fn parse_pattern_days(pattern_data: Option<&str>) -> Result<Vec<u8>> {
    let Some(raw) = pattern_data else {
        return Ok(Vec::new());
    };
    let parsed = serde_json::from_str::<crate::model::PatternData>(raw)
        .with_context(|| format!("pattern_data の JSON が不正です: {raw}"))?;
    Ok(parsed.days.unwrap_or_default())
}

// ---- Sub-task 2: 展開ロジック --------------------------------------------------

/// from..=to の範囲で recurrence pattern に合致する日付列を返す
pub fn generate_dates_for_recurrence(
    rec: &Recurrence,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<NaiveDate>> {
    let mut dates = Vec::new();
    let mut d = from;
    while d <= to {
        if matches_recurrence_pattern(&rec.pattern, rec.pattern_data.as_deref(), d)? {
            dates.push(d);
        }
        d += chrono::Duration::days(1);
    }
    Ok(dates)
}

/// 日付列を NewRecord 列に変換 (sort_order は sort_start から連番)
#[allow(dead_code)]
pub fn build_task_records(
    rec: &Recurrence,
    dates: &[NaiveDate],
    sort_start: i32,
) -> Vec<NewRecord> {
    dates
        .iter()
        .enumerate()
        .map(|(i, date)| {
            let mut fields = vec![
                ("date".into(), date.format("%Y-%m-%d").to_string()),
                ("title".into(), rec.title.clone()),
                ("category_id".into(), rec.category_id.clone()),
                ("duration_min".into(), rec.duration_min.to_string()),
                ("status".into(), "todo".into()),
                ("sort_order".into(), (sort_start + i as i32).to_string()),
                ("is_backlog".into(), "0".into()),
                ("recurrence_id".into(), rec.id.to_string()),
            ];
            if let Some(fs) = rec.fixed_start {
                fields.push(("fixed_start".into(), fs.to_string()));
            }
            NewRecord::from(fields)
        })
        .collect()
}

// ---- helper: exception / existing map ----------------------------------------

fn build_exception_map(db: &Database) -> Result<HashMap<i64, HashSet<String>>> {
    let table = db.table("recurrence_exceptions")?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for r in table.list() {
        let rec_id = match r.get("recurrence_id") {
            Some(ybasey::schema::Value::Int(v)) => *v,
            _ => continue,
        };
        let date = match r.get("exception_date") {
            Some(ybasey::schema::Value::Str(s)) => s.clone(),
            _ => continue,
        };
        map.entry(rec_id).or_default().insert(date);
    }
    Ok(map)
}

fn build_existing_map(db: &Database) -> Result<HashMap<i64, HashSet<String>>> {
    let table = db.table("tasks")?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for r in table.list() {
        let rec_id = match r.get("recurrence_id") {
            Some(ybasey::schema::Value::Int(v)) => *v,
            _ => continue,
        };
        let date = match r.get("date") {
            Some(ybasey::schema::Value::Str(s)) => s.clone(),
            _ => continue,
        };
        let is_backlog = match r.get("is_backlog") {
            Some(ybasey::schema::Value::Int(v)) => *v != 0,
            _ => false,
        };
        if !is_backlog {
            map.entry(rec_id).or_default().insert(date);
        }
    }
    Ok(map)
}

/// 日付別の次の sort_order を一括取得
fn build_date_sort_map(db: &Database) -> Result<HashMap<String, i32>> {
    let table = db.table("tasks")?;
    let mut map: HashMap<String, i32> = HashMap::new();
    for r in table.list() {
        let date = match r.get("date") {
            Some(ybasey::schema::Value::Str(s)) => s.clone(),
            _ => continue,
        };
        let is_backlog = match r.get("is_backlog") {
            Some(ybasey::schema::Value::Int(v)) => *v != 0,
            _ => false,
        };
        if is_backlog {
            continue;
        }
        let sort = match r.get("sort_order") {
            Some(ybasey::schema::Value::Int(v)) => *v as i32,
            _ => 0,
        };
        let entry = map.entry(date).or_insert(-1);
        if sort > *entry {
            *entry = sort;
        }
    }
    // max → next (max + 1)
    for v in map.values_mut() {
        *v += 1;
    }
    Ok(map)
}

// ---- Sub-task 3: expand_recurrences_to_horizon --------------------------------

pub struct ExpandResult {
    pub total_inserted: usize,
    pub elapsed_ms: u64,
}

pub fn expand_recurrences_to_horizon(db: &mut Database) -> Result<ExpandResult> {
    let start_time = std::time::Instant::now();
    let today = chrono::Local::now().date_naive();
    let config = read_horizon()?;
    let target_horizon = today + chrono::Months::new(config.years_ahead * 12);

    if config.horizon_date >= target_horizon {
        return Ok(ExpandResult {
            total_inserted: 0,
            elapsed_ms: 0,
        });
    }

    let expand_from = std::cmp::max(
        config.horizon_date + chrono::Duration::days(1),
        today,
    );

    let recurrences = crate::db::load_recurrences(db)?;
    if recurrences.is_empty() {
        write_horizon(&HorizonConfig {
            horizon_date: target_horizon,
            years_ahead: config.years_ahead,
        })?;
        return Ok(ExpandResult {
            total_inserted: 0,
            elapsed_ms: start_time.elapsed().as_millis() as u64,
        });
    }

    let exceptions = build_exception_map(db)?;
    let existing = build_existing_map(db)?;
    let mut date_sort_map = build_date_sort_map(db)?;

    let mut all_records: Vec<NewRecord> = Vec::new();

    for rec in &recurrences {
        let expand_to = match &rec.end_date {
            Some(ed) => {
                let ed = NaiveDate::parse_from_str(ed, "%Y-%m-%d")
                    .with_context(|| format!("end_date の形式が不正です: {ed}"))?;
                std::cmp::min(ed, target_horizon)
            }
            None => target_horizon,
        };
        let start = NaiveDate::parse_from_str(&rec.start_date, "%Y-%m-%d")
            .with_context(|| format!("start_date の形式が不正です: {}", rec.start_date))?;
        let from = std::cmp::max(expand_from, start);
        if from > expand_to {
            continue;
        }

        let dates = generate_dates_for_recurrence(rec, from, expand_to)?;
        let exc_set = exceptions.get(&rec.id);
        let exist_set = existing.get(&rec.id);

        for date in dates {
            let ds = date.format("%Y-%m-%d").to_string();
            if exc_set.is_some_and(|s| s.contains(&ds)) {
                continue;
            }
            if exist_set.is_some_and(|s| s.contains(&ds)) {
                continue;
            }
            let sort = *date_sort_map.entry(ds.clone()).or_insert(0);
            let mut fields = vec![
                ("date".into(), ds.clone()),
                ("title".into(), rec.title.clone()),
                ("category_id".into(), rec.category_id.clone()),
                ("duration_min".into(), rec.duration_min.to_string()),
                ("status".into(), "todo".into()),
                ("sort_order".into(), sort.to_string()),
                ("is_backlog".into(), "0".into()),
                ("recurrence_id".into(), rec.id.to_string()),
            ];
            if let Some(fs) = rec.fixed_start {
                fields.push(("fixed_start".into(), fs.to_string()));
            }
            all_records.push(NewRecord::from(fields));
            // 同日の次の sort_order をインクリメント
            *date_sort_map.get_mut(&ds).unwrap() += 1;
        }
    }

    let total = all_records.len();
    const CHUNK_SIZE: usize = 500;
    for chunk in all_records.chunks(CHUNK_SIZE) {
        db.batch_insert("tasks", chunk.to_vec())?;
    }

    write_horizon(&HorizonConfig {
        horizon_date: target_horizon,
        years_ahead: config.years_ahead,
    })?;

    Ok(ExpandResult {
        total_inserted: total,
        elapsed_ms: start_time.elapsed().as_millis() as u64,
    })
}

// ---- Sub-task 4: replace_future_tasks ----------------------------------------

pub struct ReplaceResult {
    pub deleted: usize,
    pub inserted: usize,
}

pub fn replace_future_tasks(db: &mut Database, recurrence_id: i64) -> Result<ReplaceResult> {
    let today = chrono::Local::now().date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();
    let config = read_horizon()?;

    let to_delete: Vec<u64> = {
        let table = db.table("tasks")?;
        let rec_id_str = recurrence_id.to_string();
        table
            .find_by_field("recurrence_id", &rec_id_str)
            .iter()
            .filter(|r| {
                let date = match r.get("date") {
                    Some(ybasey::schema::Value::Str(s)) => s.clone(),
                    _ => return false,
                };
                let actual_start = r.get("actual_start");
                let is_backlog = match r.get("is_backlog") {
                    Some(ybasey::schema::Value::Int(v)) => *v != 0,
                    _ => false,
                };
                date > today_str
                    && actual_start.is_none_or(|v| matches!(v, ybasey::schema::Value::Null))
                    && !is_backlog
            })
            .map(|r| r.id)
            .collect()
    };

    let rec = crate::db::load_recurrences(db)?
        .into_iter()
        .find(|r| r.id == recurrence_id)
        .context("繰り返しルールが見つかりません")?;

    let expand_to = match &rec.end_date {
        Some(ed) => {
            let ed = NaiveDate::parse_from_str(ed, "%Y-%m-%d")
                .with_context(|| format!("end_date の形式が不正です: {ed}"))?;
            std::cmp::min(ed, config.horizon_date)
        }
        None => config.horizon_date,
    };
    let expand_from = today + chrono::Duration::days(1);

    let deleted_count = to_delete.len();

    if expand_from > expand_to {
        let delete_ops: Vec<ybasey::Op> = to_delete
            .iter()
            .map(|&id| ybasey::Op::Delete { id })
            .collect();
        if !delete_ops.is_empty() {
            db.batch("tasks", delete_ops)?;
        }
        return Ok(ReplaceResult {
            deleted: deleted_count,
            inserted: 0,
        });
    }

    let dates = generate_dates_for_recurrence(&rec, expand_from, expand_to)?;
    let exceptions = build_exception_map(db)?;
    let exc_set = exceptions.get(&recurrence_id);
    let filtered: Vec<NaiveDate> = dates
        .into_iter()
        .filter(|d| {
            let ds = d.format("%Y-%m-%d").to_string();
            !exc_set.is_some_and(|s| s.contains(&ds))
        })
        .collect();

    // sort_order 衝突を避けるため delete 対象を除外した日付別 max sort_order を計算
    let to_delete_set: std::collections::HashSet<u64> = to_delete.iter().copied().collect();
    let mut date_sort_map = {
        let table = db.table("tasks")?;
        let mut map: HashMap<String, i32> = HashMap::new();
        for r in table.list() {
            if to_delete_set.contains(&r.id) {
                continue;
            }
            let date = match r.get("date") {
                Some(ybasey::schema::Value::Str(s)) => s.clone(),
                _ => continue,
            };
            if matches!(r.get("is_backlog"), Some(ybasey::schema::Value::Int(v)) if *v != 0) {
                continue;
            }
            let sort = match r.get("sort_order") {
                Some(ybasey::schema::Value::Int(v)) => *v as i32,
                _ => 0,
            };
            let entry = map.entry(date).or_insert(-1);
            if sort > *entry {
                *entry = sort;
            }
        }
        for v in map.values_mut() {
            *v += 1;
        }
        map
    };

    let inserted_count = filtered.len();
    let insert_records: Vec<NewRecord> = filtered
        .iter()
        .map(|date| {
            let ds = date.format("%Y-%m-%d").to_string();
            let sort = *date_sort_map.entry(ds.clone()).or_insert(0);
            *date_sort_map.get_mut(&ds).unwrap() += 1;
            let mut fields = vec![
                ("date".into(), ds),
                ("title".into(), rec.title.clone()),
                ("category_id".into(), rec.category_id.clone()),
                ("duration_min".into(), rec.duration_min.to_string()),
                ("status".into(), "todo".into()),
                ("sort_order".into(), sort.to_string()),
                ("is_backlog".into(), "0".into()),
                ("recurrence_id".into(), rec.id.to_string()),
            ];
            if let Some(fs) = rec.fixed_start {
                fields.push(("fixed_start".into(), fs.to_string()));
            }
            NewRecord::from(fields)
        })
        .collect();

    let mut ops: Vec<ybasey::Op> = to_delete
        .iter()
        .map(|&id| ybasey::Op::Delete { id })
        .collect();
    ops.extend(
        insert_records
            .into_iter()
            .map(|r| ybasey::Op::Insert { record: r }),
    );
    if !ops.is_empty() {
        db.batch("tasks", ops)?;
    }

    Ok(ReplaceResult {
        deleted: deleted_count,
        inserted: inserted_count,
    })
}
