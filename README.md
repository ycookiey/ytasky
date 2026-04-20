# ytasky

lazygit風の操作感を持つ、フレキシブルなタイムブロッキングTUIスケジューラ。

「今何をすればいいか」が一目でわかるように、1日を15分ブロック単位で綿密にスケジュールする。

## Concept

- **タスク = 生活すべて**: 睡眠・食事・通学・勉強・開発・休憩など、実施することを網羅的に管理
- **15分ブロック**: すべてのタスクは15分単位で管理。視覚的にタイムラインバーで表示
- **余り時間の自動計算**: 24h - 割当済み時間 = 残り時間をリアルタイム表示
- **キーボード駆動**: マウス不要。Vim風キーバインドで高速操作

## Key Bindings

| Key | Action |
|-----|--------|
| `j` / `k` | カーソル移動 |
| `J` / `K` (Shift) | タスク並び替え |
| `h` | ズームアウト (日→週→月) |
| `l` | ズームイン (月→週→日) |
| `t` | タイムラインビュー切替 |
| `space` | 開始/完了トグル |
| `a` | タスク追加 |
| `d` | タスク削除 |
| `e` | タスク編集 |
| `u` / `Ctrl+z` | Undo |
| `Ctrl+r` | Redo |
| `q` | 終了 |

## Categories

| ID | Icon | Color | 用途 |
|----|------|-------|------|
| `sleep` | 󰒲 | blue-grey | 睡眠 |
| `meal` | 󰩃 | yellow | 食事 |
| `work` | 󰈙 | pink | 開発・仕事 |
| `study` | 󰑴 | purple | 勉強・講義 |
| `exercise` | 󰖏 | green | 運動 |
| `personal` |  | orange | 身支度・自由時間 |
| `break` | 󰾴 | cyan | 休憩 |
| `commute` | 󰄋 | red | 移動 |
| `errand` | 󰒓 | teal | 雑用・用事 |

## MCP Server

Claude Code など AI エージェントから操作するための MCP server モードを内蔵。

```
ytasky mcp     # stdio で MCP server 起動 (要 --features mcp ビルド)
```

provide tool: `list_tasks` / `add_task` / `edit_task` / `delete_task` / `start_task` / `done_task` / `move_task` / `list_backlog` / `schedule_backlog` / `add_recurrence` / `report` / `history` ほか。

## Storage

データは [ybasey](../ybasey) (file-based DB) に保存される。

- Windows: `%APPDATA%\ytasky\data`
- Linux/macOS: `~/.config/ytasky/data`
- `YTASKY_DATA_DIR` 環境変数で上書き可
