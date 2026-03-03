# Architecture

## Tech Stack

- **言語**: Rust
- **TUIフレームワーク**: Ratatui
- **データベース**: SQLite（rusqlite）

高速動作を最優先に選定。将来のWeb版展開時にはlibSQL/Tursoへの移行を視野に入れる（SQLite互換のためスキーマ・クエリはそのまま流用可能）。

## Core Mechanics

### 動的連鎖計算

タスクを並び替え（`J`/`K`）ると、開始時刻が自動で再計算される。各タスクは見積もり時間のみ必須で、開始時刻は前タスクの終了から自動決定。明示的に時刻を固定することも可能。

### 予定 vs 実績

各タスクは予定時刻（連鎖計算）と実績時刻（`space`で記録）の両方を持つ。差分をレポートで可視化。

### Undo/Redo

`u` / `Ctrl+r`。コマンドパターンでセッション中の操作履歴を管理。

### 繰り返しタスク

Googleカレンダー風。パターン（毎日/毎週/毎月）+ 例外日リストで管理。個別編集した日は通常タスクに変換。

### レポート

カテゴリ別・タイトル別の時間集計。予定/実績の比較。範囲は日〜全期間。

### 日跨ぎ

深夜作業等で日付を跨ぐ場合を自動補正。

## Data Format

SQLiteに永続化。

### スキーマ

```sql
CREATE TABLE categories (
    id    TEXT PRIMARY KEY,   -- 'sleep', 'meal', 'work', etc.
    name  TEXT NOT NULL,
    icon  TEXT NOT NULL,
    color TEXT NOT NULL
);

CREATE TABLE tasks (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    date          TEXT NOT NULL,        -- 'YYYY-MM-DD'
    sort_order    INTEGER NOT NULL,     -- 連鎖計算の順序
    title         TEXT NOT NULL,
    category_id   TEXT NOT NULL REFERENCES categories(id),
    duration_min  INTEGER NOT NULL,     -- 見積もり（分）
    fixed_start   INTEGER,             -- NULL=自動計算, 値=固定開始時刻（分, 0-1439）
    actual_start  INTEGER,             -- 実績開始（分）
    actual_end    INTEGER,             -- 実績終了（分）
    recurrence_id INTEGER REFERENCES recurrences(id),
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(date, sort_order)
);

CREATE TABLE recurrences (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    category_id  TEXT NOT NULL REFERENCES categories(id),
    duration_min INTEGER NOT NULL,
    fixed_start  INTEGER,
    pattern      TEXT NOT NULL,  -- 'daily' | 'weekly' | 'monthly'
    pattern_data TEXT,           -- JSON: {"days":[1,3,5]} etc.
    start_date   TEXT NOT NULL,
    end_date     TEXT,           -- NULL=無期限
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE recurrence_exceptions (
    recurrence_id INTEGER NOT NULL REFERENCES recurrences(id),
    date          TEXT NOT NULL,
    PRIMARY KEY (recurrence_id, date)
);

CREATE INDEX idx_tasks_date ON tasks(date);
```

### 設計方針

- 時刻は分単位の整数（480 = 08:00）。15分ブロックとの計算が楽
- `fixed_start`がNULL → 前タスク終了から自動計算、値あり → 固定
- 繰り返しタスクはルール定義のみ保存。日を開いた時に`tasks`行を遅延生成
- 個別編集時は`recurrence_id`をNULLにして通常タスクに変換

## Future Ideas

- [ ] テンプレート: 平日/休日/試験期間など、パターン化したスケジュールを即座に呼び出し
- [ ] ポモドーロ統合: ブロック内でポモドーロタイマーを起動
- [ ] カレンダー連携: Google Calendar等からインポート/エクスポート
- [ ] 通知: 次のタスク開始時にデスクトップ通知
- [ ] GitHub Issues連携: lifeリポジトリのIssueをタスクとして取り込み
- [ ] Web版: libSQL/Tursoでデータ同期し、同等機能をブラウザで提供
