# Google Calendar Import

ytasky の Google Calendar import 機能のドキュメント。**M1〜M7 実装済み**（`gcal` feature）。
本書は設計意図と実装結果の両方を反映する。設計段階からの主な変更点は各節と §11 に記す。

## 1. ゴール

- Google Calendar のイベントを ytasky の `tasks` / `recurrences` に取り込む
- 単発イベントは `tasks` に `fixed_start` 付きで挿入
- 繰り返しイベントは `recurrences` に変換し、ytasky の horizon 展開で日々のタスクへ反映
- 同じイベントを再 import しても重複しない（external_id ベースの upsert）
- 操作面は CLI / MCP / TUI キー / 起動時 1 回の lazy sync の 4 経路で対応

ノンゴール（現フェーズで扱わない、§12 で将来拡張）:

- GCal への書き込み（Export / 双方向同期）
- 終日イベントの取り込み（複日跨ぎ・1440 分占有問題のため当面スキップ）
- 複数カレンダーの同時 import（`--calendar` で 1 つずつ）
- カテゴリ自動推定（一律デフォルトカテゴリ）
- GCal 完全削除イベントの削除伝播（`events.list` に出現しないため検知不可）
- EXDATE / RDATE の反映（パースして summary に件数集計するのみ。recurrence_exceptions への登録は未実装）

## 2. アーキテクチャ

```
src/
├── gcal/
│   ├── mod.rs        # GcalConfig / load_config / truncate_for_log
│   ├── auth.rs       # OAuth + PKCE + Loopback + token 保管/refresh
│   ├── api.rs        # events.list / events.instances の薄いクライアント
│   ├── rrule.rs      # GCal recurrence 配列 → ytasky pattern/pattern_data
│   ├── tz.rs         # RFC3339 ↔ (NaiveDate, 分)、_meta.tz 解決
│   ├── import.rs     # event → task/recurrence の upsert オーケストレータ
│   └── types.rs      # API レスポンス用 serde 型
build.rs              # gcal_client.json を読み credential をビルド時埋め込み
```

呼び出し階層:

```
cli.rs / mcp.rs / app.rs (TUI Shift+G・lazy sync)
        └─ gcal::import::import_range(db, from, to, &ImportOptions)
                 ├─ auth::get_valid_token()            (必要なら refresh)
                 ├─ api::list_events(token, …, singleEvents=false)
                 ├─ rrule::parse_recurrence_rules(...) (recurrence 振り分け)
                 ├─ api::list_event_instances(eventId) (Unsupported のフォールバック)
                 ├─ tz::rfc3339_to_local_minute(...)
                 └─ db.upsert(table, "external_id", ext_id, fields)
```

`gcal` モジュールは **`gcal` feature flag** 配下。default features に含めるが
`--no-default-features` で外せる（§8）。

## 3. スキーマ変更

### 3.1 / 3.2 external_id 列（tasks / recurrences）

| field | type | nullable | 用途 |
|---|---|---|---|
| `external_id` | `str` | true | `gcal:<calendarId>:<eventId>` 形式。再 import 時の upsert キー |

### 3.3 pattern_data の拡張

```rust
#[derive(Default, Serialize, Deserialize)]
pub struct PatternData {
    pub days: Option<Vec<u8>>,   // 既存。pattern により解釈が変わる（下表）
    pub interval: Option<u8>,    // INTERVAL=N（未指定/1 は None）
    pub setpos: Option<i8>,      // BYSETPOS（1=第1 … -1=最終）
}
```

`days` の解釈:

| pattern | setpos | days の意味 | 例 |
|---|---|---|---|
| `daily` | — | 未使用 | — |
| `weekly` | — | 曜日番号 (1=MO … 7=SU) | `[1,3,5]` = 月水金 |
| `monthly` | None | BYMONTHDAY (1-31) | `[15]` = 毎月15日 |
| `monthly` | Some(N) | BYDAY 曜日番号 (1-7) | `days=[2], setpos=2` = 第2火曜 |

**設計変更**: 当初 `days: Vec<i8>` を検討したが、`days` に負値が来るケース（BYDAY=-1MO 等）は
`setpos` 側に逃がせるため `Vec<u8>` を維持。app.rs / ui.rs のフォーム実装への波及も避けた。
全フィールドに `#[serde(default)]` + `Option` を付けており、既存 `{"days":[1,3]}` JSON は
そのまま読める（後方互換）。`month_days` 等は YAGNI で追加しない。

### 3.4 migration

`apply_schema`（`create_table` + `add_field`）は冪等でない（既存で `TableExists`/`FieldExists`）。
既存 DB 向けに差分適用する `migrate_schema` を新設し、`db::open()` 内で毎回呼ぶ。

```rust
pub fn migrate_schema(db: &mut ybasey::Database) -> Result<()> {
    add_field_if_absent(db, "tasks", "external_id", "str", true)?;
    add_field_if_absent(db, "recurrences", "external_id", "str", true)?;
    Ok(())
}
// add_field_if_absent は db.table(t)?.schema.has_field(name) で存在確認してから add_field
```

`Database::table()?.schema` / `Schema::has_field()` はいずれも pub のため ybasey 変更は不要。
`record_to_task` / `record_to_recurrence` も `external_id` を `Option<String>` で読む。

## 4. RRULE → ytasky pattern マッピング

GCal の `recurrence` は `["RRULE:...", "EXDATE:...", "RDATE:..."]` 配列。RRULE 1 本を主とする
（複数 RRULE 行は Unsupported）。手書きパーサ（`gcal/rrule.rs`）。

### 4.1 サポートマトリクス

| RRULE | ytasky 表現 | 備考 |
|---|---|---|
| `FREQ=DAILY[;INTERVAL=N]` | `pattern=daily[, interval=N]` | |
| `FREQ=WEEKLY;BYDAY=MO,WE,FR` | `pattern=weekly, days=[1,3,5]` | BYDAY 省略時は start_date の曜日 |
| `FREQ=WEEKLY;INTERVAL=N;BYDAY=…` | `pattern=weekly, interval=N, days=…` | |
| `FREQ=MONTHLY;BYMONTHDAY=15` | `pattern=monthly, days=[15]` | setpos=None ⇒ days を BYMONTHDAY |
| `FREQ=MONTHLY;BYDAY=2TU` | `pattern=monthly, days=[2], setpos=2` | 第2火曜。BYDAY 省略時は start_date の日 |
| `FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2` | 同上 | |
| `FREQ=MONTHLY;BYDAY=-1FR` | `pattern=monthly, days=[5], setpos=-1` | 最終金曜 |
| `UNTIL=…` | `end_date` | |
| `COUNT=N` | `end_date` に換算 | daily/weekly/monthly 別に近似計算 |
| `FREQ=YEARLY` / `BYWEEKNO` / `BYDAY+BYMONTHDAY 併用` 等 | **Unsupported** → instances 展開（§4.2） | |

**入力防御**（敵対カレンダー対策、§実装 R3）:

- `INTERVAL=0` → `Invalid`。`INTERVAL` は u32 でパースし、ytasky の `u8` 範囲（1-255）に
  収まらない値（256 以上）は `Unsupported`（instances 展開へ）
- `COUNT=0` → `Invalid`、`COUNT > 10000` → `Invalid`（巨大日付計算の暴走防止）
- `EXDATE` / `RDATE` のリスト長が 1000 件超 → `Invalid`（DoS 防止）

### 4.2 Unsupported RRULE のフォールバック

`events.instances`（`GET /calendars/{calendarId}/events/{eventId}/instances`）で当該イベントの
instance 群のみを取得し、各 instance を `tasks` に upsert。external_id は **GCal が返す
instance の `id`**（`parent_20260518T010000Z` のように originalStartTime を内包しユニーク）を
そのまま `gcal:<calendarId>:<id>` に使う。`events.list?singleEvents=true` 全件再取得は単発イベントと
重複するため不採用。

### 4.3 RRULE パーサ

クレート非依存の手書き。`KEY=VALUE` を `;` で分解し、マトリクス外の要素が混じれば
`RruleError::Unsupported`、構文・値が不正なら `RruleError::Invalid` を返す。import 側は
Unsupported → instances 展開、Invalid → スキップ + summary.errors に記録。

### 4.4 matches_recurrence_pattern

`(rec: &Recurrence, target_date) -> Result<bool>` シグネチャ。start_date からの経過で
INTERVAL を判定、monthly + setpos で「第N曜日 / 最終曜日（月末から逆算）」を判定。
`generate_dates_for_recurrence` は `from..=to` を 1 日ずつ走査。

## 5. OAuth 2.0 Loopback Redirect + PKCE

### 5.1 フロー（`gcal/auth.rs`）

1. credential 取得（§5.4）
2. 32B random → `code_verifier`、SHA256 → base64url で `code_challenge`（S256）
3. `tiny_http` で 127.0.0.1 の OS 割当ポートに bind
4. 認可 URL をブラウザで開く（`webbrowser`）。scope は `calendar.readonly`、`access_type=offline`、
   `prompt=consent`、32B random の `state`
5. `/cb?code=…&state=…` を受信。**state を定数時間比較で検証してから**レスポンスを返す
   （CSRF 検知時はブラウザに「認証完了」を出さない）。120 秒タイムアウト
6. code を token endpoint に POST → `access_token` / `refresh_token` / `expires_in`

### 5.2 同時実行ロック

`~/.config/ytasky/gcal_login.lock` を `fs2::try_lock_exclusive`。取得失敗時は
「別の ytasky プロセスが認証中」でフロー中止。ポートは OS 任せで競合なし。

### 5.3 credential / token の保管とセキュリティ

`~/.config/ytasky/gcal.json`（任意。あれば同梱より優先）:

```json
{ "client_id": "...", "client_secret": "...", "auth_uri": "...", "token_uri": "..." }
```

- Google Cloud Console の `{"installed":{…}}` / `{"web":{…}}` ラッパーにも対応
- **`auth_uri` / `token_uri` は Google 公式 prefix（`https://accounts.google.com/`,
  `https://oauth2.googleapis.com/`）必須**。悪意ある gcal.json で token endpoint を攻撃者 URL に
  差し替える攻撃を `validate_oauth_endpoints` で遮断

`~/.config/ytasky/gcal_token.json`（access/refresh token, expires_at, scope）:

- **Unix**: `chmod 600`（失敗時は警告出力）
- **Windows**: ユーザープロファイル ACL 依存（追加保護なし。README に明記）
- `Credential` / `Token` / `TokenResponse` は `Debug` を手動実装し secret を `[REDACTED]` に
  マスク。token endpoint のエラー body は `truncate_for_log`（先頭 200 文字）で切り詰め、
  200 OK のパース失敗時は body を含めない（token のログ漏洩防止）

### 5.4 同梱 client_id/secret（ビルド時埋め込み）

**設計変更**: 当初「ソースにコンパイル時定数で埋め込む」としたが、secret を git 管理しない形に変更:

- `build.rs` がリポジトリ直下の `gcal_client.json`（Cloud Console の Desktop app credential JSON、
  **`.gitignore` 済み**）を読み、`YTASKY_GCAL_CLIENT_ID` / `YTASKY_GCAL_CLIENT_SECRET` を
  `cargo:rustc-env` で設定
- `auth.rs` は `option_env!("YTASKY_GCAL_CLIENT_ID")` で受ける。ファイルが無ければ `None` となり、
  `~/.config/ytasky/gcal.json` の配置が必須になる
- これにより、配布バイナリには credential が埋め込まれる一方、リポジトリには secret が入らない
  （rclone 型の「同梱 + 上書き可」）

### 5.5 Refresh

`expires_at`（30 秒スキュー込みの `is_expired`）を確認し、過ぎていれば `refresh_token` で更新。
refresh レスポンスに refresh_token が無い場合は既存値を維持。API が 401 を返した場合は
明示メッセージで `ytasky gcal-login` を案内（自動 refresh リトライは未実装、§12）。

### 5.6 Scope

`https://www.googleapis.com/auth/calendar.readonly` のみ。将来 Export 時に `calendar.events` へ。

## 6. import 実行フロー（`gcal/import.rs`）

### 6.1 手順

1. `auth::get_valid_token()`（必要なら refresh）
2. `_meta` から tz を解決、timeMin/timeMax を midnight RFC3339 に
3. `api::list_events(calendarId, timeMin, timeMax, single_events=false)` を nextPageToken で完走
   （`MAX_PAGES=200` + 同一 token 検知で暴走防止、429 は Retry-After 尊重で 1 回 retry）
4. イベントごとに振り分け（`handle_event`）:
   - `status=cancelled` / 終日 / 親あり instance（`recurring_event_id` あり）→ スキップ
   - `recurrence` あり: parse 成功 → `recurrences` upsert / Unsupported → instances 展開で `tasks` upsert /
     Invalid → スキップ + errors 記録
   - `recurrence` 無し → `tasks` upsert
5. `ImportSummary { created, updated, skipped, skipped_exdates, skipped_rdates, errors }` を返す
   （個別イベントのエラーで全体停止せず errors に積む）

### 6.2 upsert ロジック

`db.upsert(table, "external_id", ext_id, fields)` を利用（WAL ベースで原子的）。

- **tasks**: insert 時のみ `sort_order=0` を渡す（update 時は触らず既存順序維持。表示時に
  `normalize_fixed_tasks_by_time` で時刻順に整列）。title は `sanitize_text` で制御文字 /
  ANSI escape / Unicode BIDI override・不可視文字（Cf/Zl/Zp）を除去（TUI 偽装防止）
- **recurrences**: `upsert` の `inserted` フラグで summary の created/updated を区別。end_date は
  `None` のとき ybasey の Null sentinel `"_"` を渡して明示クリア（GCal で UNTIL 削除時に追従）

ytasky 側で手動編集したいタスクは `external_id` を NULL にして link 解除（detach コマンドは §12）。

### 6.3 GCal 側削除

- cancelled instance はスキップ。完全削除イベントは `events.list` に出ないため検知不可（§12）

### 6.4 カテゴリ

`--category <id>`（デフォルト `"6"` = 身支度・自由時間 ≒ personal）。colorId 推定はしない。

### 6.5 タイムゾーン処理（`gcal/tz.rs`）

`start.dateTime`（RFC3339）→ `DateTime<FixedOffset>` → `_meta.tz`（`chrono_tz::Tz`、未設定/不正は
UTC フォールバック）→ ローカル `(NaiveDate, hour*60+minute)`。duration は end-start の分。
timeMin/timeMax 生成（`date_to_rfc3339_at_midnight`）は DST 境界に対応:
Ambiguous → 早い方、None（gap）→ 1〜4 時間進めて最初の有効時刻。複日跨ぎはノンゴール。

## 7. 操作インタフェース

### 7.1 CLI

```
ytasky import-gcal [--from YYYY-MM-DD] [--to YYYY-MM-DD] [--calendar <id>] [--category <id>]
ytasky gcal-login    # OAuth フローのみ
ytasky gcal-logout   # token 削除
```

- `--from` 省略=今日、`--to` 省略=from+30 日
- **`gcal-login` / `gcal-logout` は DB を必要としない**ため、`main.rs` で `db::open()` を
  経由せず直接 `auth::login()/logout()` を呼ぶ（未 init 環境でも認証可能）

### 7.2 MCP

`mcp__ytasky__import_gcal`（from_date / to_date / calendar_id / category）→
`{ created, updated, skipped, skipped_exdates, skipped_rdates, errors }`。
`#[tool]` は cfg 外に置き、handler 内で `#[cfg(feature="gcal")]` と `#[cfg(not)]` を分岐
（rmcp の tool_router が gcal 無効ビルドでも展開できるようにするため）。gcal 無効時は
`invalid_params` で「機能無効」を返す。

### 7.3 TUI

**設計変更**: 当初の「from/to/calendar/category 入力ダイアログ」は簡素化。

- `Shift+G` で確認モーダル（「今日〜+30日 / primary / 既定カテゴリで import」）
- `Enter` で `kick_gcal_import_background` がバックグラウンドスレッドで import（UI を
  ブロックしない）。完了は `poll_background_sync` がトースト化
- 詳細指定（範囲・カレンダー・カテゴリ）は CLU を使う
- キーバインドは **dispatch テーブル**（`COMMON_BINDINGS` / `TABLE_BINDINGS` /
  `TIMELINE_BINDINGS`）で一元管理。`handle_normal_key` の処理と `draw_keybindings_bar` の
  下部バー表示が同じテーブルから生成され、ショートカット追加時の表示漏れが起きない

### 7.4 起動時 lazy sync

- `App::new` で `std::thread::spawn`（token 無し / config OFF ならスキップ。spawn 失敗は警告）
- スレッド内で新規 `Database::open` + `migrate_schema` + `import_range`（`reqwest::blocking`）
- 結果は `std::sync::mpsc` でメインへ。`main.rs` の event loop が **500ms poll** ごとに
  `poll_background_sync` で受信 → `db.refresh()`（別スレッドの書き込みを in-memory に反映）→
  `refresh_tasks` → トースト表示
- 期間: 今日〜+`gcal_auto_sync_days`（既定 7）。`~/.config/ytasky/config.json` の
  `gcal_auto_sync: bool` で OFF 可

#### DB 排他

ybasey は `Database::open` 時に exclusive lock を取り初期化後 release、書き込み毎にも
acquire/release する（`storage/lock.rs`）。別プロセス・別スレッドの複数ハンドル並行で安全。

## 8. 依存 / feature flag

```toml
[features]
default = ["gcal"]
mcp = ["dep:rmcp", "dep:tokio", "dep:schemars"]
gcal = ["dep:reqwest", "dep:tiny_http", "dep:base64", "dep:sha2", "dep:rand",
        "dep:url", "dep:percent-encoding", "dep:webbrowser", "dep:fs2", "dep:chrono-tz"]

[build-dependencies]
serde_json = "1"   # build.rs で gcal_client.json をパース
```

reqwest は `default-features=false` + `rustls-tls` + `blocking` + `json`。
**tokio は non-optional に昇格せず**（`reqwest::blocking` + `std::thread` でカバー）、mcp feature 限定のまま。
`gcal` は default に含むが `--no-default-features --features mcp` 等で除外可。
`src/gcal/` および CLI/MCP/TUI の該当箇所は `#[cfg(feature = "gcal")]` で gate。

## 9. Google Cloud Console 準備（自前 credential を使う場合）

同梱版（`gcal_client.json` をビルドに埋め込んだバイナリ）を使う場合は不要。自前で用意するなら:

1. プロジェクト作成（MFA 必須化されているアカウントは事前に 2 段階認証を設定）
2. **Google Calendar API** を有効化
3. **Google Auth Platform**（旧 OAuth consent screen）を構成: アプリ名 / サポートメール /
   対象=**External** / 連絡先
4. **公開モードの選択**:
   - **Testing**: テストユーザー登録が必要。refresh_token が **7 日で失効**
   - **本番公開（未審査）**: 「アプリを公開」を押すとテストユーザー登録不要・refresh_token **無期限**。
     「確認されていません」警告は出るが「詳細→移動」で進める。**個人利用はこちらを推奨**
5. **Credentials > OAuth client ID > Desktop app** を作成し JSON をダウンロード
6. 配置先:
   - 自前ビルドに埋め込む: リポジトリ直下に `gcal_client.json`（gitignore 済み）として置き
     `cargo build --release` → `build.rs` が埋め込む
   - 配置で使う: `~/.config/ytasky/gcal.json` に置く

## 10. テスト

実装では各モジュール内の `#[cfg(test)]` unit テストが中心:

- `rrule.rs`: DAILY/WEEKLY/MONTHLY、INTERVAL、BYDAY/BYMONTHDAY/BYSETPOS、UNTIL/COUNT、
  EXDATE/RDATE、Unsupported 検出、INTERVAL=0/256・COUNT=0/上限・EXDATE 上限の Invalid
- `recurrence.rs`（tests/recurrence.rs）: interval=2 隔日/隔週/隔月、setpos=2/-1、月末境界、
  start_date 以前 false
- `tz.rs`: JST/UTC/日跨ぎ/DST/不正 tz/欠落
- `types.rs`: events.list / 終日 / cancelled / instance のパース
- `import.rs`: in-memory ybasey で upsert の insert→update / 冪等性 / sanitize_text
- `auth.rs`: PKCE 仕様、state 定数時間、URL state redact、token JSON、Debug マスク、
  installed ラッパー、endpoint 検証
- `mod.rs`: GcalConfig のデフォルト / 部分 / 空
- OAuth 実フロー: 手動（HTTP モックは工数に見合わず）

## 11. 実装履歴

| # | 内容 | 状態 |
|---|---|---|
| M1 | external_id 列 + migrate_schema | ✅ |
| M2 | pattern_data の interval/setpos 拡張 + matches_recurrence_pattern + CLI/MCP/TUI | ✅ |
| M3 | OAuth PKCE + Loopback + token 保管/refresh | ✅ |
| M4 | events.list / events.instances + tz 変換 | ✅ |
| M5 | RRULE パーサ + import オーケストレータ | ✅ |
| M6 | CLI / MCP / TUI 統合 | ✅ |
| M7 | 起動時 lazy sync | ✅ |
| R1〜R6 | 2 次にわたる多角レビュー（OAuth/データ整合/API/統合/攻撃者視点）の指摘対応 | ✅ |
| 追加 | gcal-login の DB 非依存化 / build.rs 埋め込み / keybinding dispatch テーブル化 / 未使用コード削除 | ✅ |

## 12. 将来拡張

- GCal への Export（書き込み、`calendar.events` scope）
- 401 自動 refresh リトライ
- 完全削除イベントの検知（`updatedMin` 差分 or 孤児リスト）
- 複数カレンダー対応（`list_calendars` の再追加 + 一覧 UI）
- 終日イベントの取り込み（複日跨ぎ分割）
- EXDATE / RDATE の反映（`recurrence_exceptions` 登録 / RDATE 個別 task 化）
- TUI の詳細 import ダイアログ（from/to/calendar/category）
- ytasky → GCal の detach コマンド（external_id クリア）
- 編集競合検出（etag）、Outlook / iCloud / CalDAV 連携
