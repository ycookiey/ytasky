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

## Google Calendar 連携 (gcal feature)

`ytasky import-gcal` / `gcal-login` / TUI 上の `Shift+G` で Google Calendar の
イベントを取り込める (`--features gcal` でビルド、default features に含まれる)。

### Credential

OAuth client は `~/.config/ytasky/gcal.json` で上書き可能。`gcal.json` の
`auth_uri` / `token_uri` は **Google 公式エンドポイント** (`https://accounts.google.com/`,
`https://oauth2.googleapis.com/`) で始まる必要があり、それ以外は拒否される
(悪意ある設定ファイルによる token 漏洩を防ぐため)。

### Token 保管とセキュリティ

`~/.config/ytasky/gcal_token.json` に access_token / refresh_token を保存する。

- **Unix**: ファイル権限 `0600` を自動設定 (所有者のみ読書き可)
- **Windows**: OS のユーザープロファイル ACL に依存する。**共有 PC や別ユーザー
  からアクセス可能な環境では、token が同マシン上の他ユーザー / 管理者 /
  マルウェアから平文で読み取られるリスクがある**。重要な運用では
  `icacls` でアクセスを当該ユーザーのみに絞るか、専用マシンでのみ使うこと

### スコープ

`https://www.googleapis.com/auth/calendar.readonly` のみ要求する (読取専用)。
