# Architecture

## Tech Stack

- **言語**: Rust
- **TUIフレームワーク**: Ratatui
- **データベース**: [ybasey](../../ybasey)（file-based DB、AI Read最適化）

データは `<config>/ytasky-ybasey/<table>/` 配下にテーブル単位のテキストファイルとして保存される。Web版展開時はファイルをそのままR2等にミラーする方式（`ybasey-mirror` を経由するか、ybaseyの将来の同期機能を使う）を想定。

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

ybaseyに永続化。各テーブルが `<table>/_all` `<table>/_schema` などのファイルに展開され、daemonがwriteごとに更新する。スキーマ定義は `src/init.rs` を起点とする。

### テーブル

| Table | 主な役割 |
|---|---|
| `categories` | カテゴリマスタ（`name`/`color`/`icon`） |
| `tasks` | 1日単位のタスク。`date`/`sort_order`で連鎖、`fixed_start`で時刻固定 |
| `recurrences` | 繰り返しタスクのルール（`pattern` + `pattern_data`） |
| `recurrence_exceptions` | 個別の繰り返し例外日 |

スキーマの実体は `ybasey schema <table>` で取得できる。代表的な`tasks`は `id auto / date / title / category_id (ref) / duration_min / fixed_start? / actual_start? / actual_end? / status / sort_order / recurrence_id (ref)? / note? / is_backlog / deadline?`。

### 設計方針

- 時刻は分単位の整数（480 = 08:00）。15分ブロックとの計算が楽
- `fixed_start`がNULL → 前タスク終了から自動計算、値あり → 固定
- 繰り返しタスクはルール定義のみ保存。日を開いた時に`tasks`行を遅延生成
- 個別編集時は`recurrence_id`をNULLにして通常タスクに変換
- ybaseyは `_all` の他に `_f.<field>_eq_<value>` フィルタファイルや `_v.<view>` を自動生成するため、外部からのRead-onlyな解析は付随ファイルを直接参照しても良い

## Future Ideas

- [ ] テンプレート: 平日/休日/試験期間など、パターン化したスケジュールを即座に呼び出し
- [ ] ポモドーロ統合: ブロック内でポモドーロタイマーを起動
- [ ] カレンダー連携: Google Calendar等からインポート/エクスポート
- [x] 通知: 次のタスク開始前のリマインド（ybasey → R2 → Cloudflare Workers → Discord 経路で実装。詳細は [life-notifier](../../life-notifier)）
- [ ] Web版: ybasey-mirror がR2に上げているスナップショットを Cloudflare Pages から読む方式を想定
