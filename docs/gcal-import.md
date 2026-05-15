# Google Calendar Import 設計

ytasky に Google Calendar からのイベント import を組み込むための設計ドキュメント。

## 1. ゴール

- Google Calendar のイベントを ytasky の `tasks` / `recurrences` に取り込む
- 単発イベントは `tasks` に `fixed_start` 付きで挿入
- 繰り返しイベントは `recurrences` に変換し、ytasky の horizon 展開で日々のタスクへ反映
- 同じイベントを再度 import しても重複しない（external_id ベースの upsert）
- 操作面は CLI / MCP / TUI キー / 起動時 1 回の lazy sync の 4 経路で対応

ノンゴール（現フェーズで扱わない）:

- GCal への書き込み（Export / 双方向同期）
- **終日イベントの取り込み**（複日跨ぎや 1440 分占有問題を回避するため当面サポート外）
- 複数カレンダーの同時 import（`--calendar` で 1 つずつ）
- カテゴリ自動推定（一律デフォルトカテゴリに入れる）
- GCal 完全削除イベントの ytasky 側削除伝播（`events.list` に出現しないため検知不可。link 解除 or 手動削除で対応）

## 2. アーキテクチャ

```
src/
├── gcal/
│   ├── mod.rs        # 公開 API: import_range(), login(), logout()
│   ├── auth.rs       # OAuth + PKCE + Loopback サーバー + token 保管/refresh
│   ├── api.rs        # events.list / events.instances / calendarList.list の薄いクライアント
│   ├── rrule.rs      # RRULE 文字列 → ytasky pattern/pattern_data 変換
│   ├── tz.rs         # RFC3339 ↔ 「日付 + 分」変換、_meta.tz 解決
│   ├── import.rs     # event → task/recurrence の upsert オーケストレータ
│   └── types.rs      # API レスポンス用 serde 型
```

呼び出し階層:

```
cli.rs / mcp.rs / app.rs (TUI) / main.rs (lazy)
        └─ gcal::import_range(db, from, to, options)
                 ├─ auth::get_valid_token()           (必要なら refresh / login)
                 ├─ api::list_events(token, …)
                 ├─ rrule::parse_to_pattern(rrule)    (recurrence 振り分け)
                 ├─ api::list_instances(eventId)     (フォールバック時)
                 ├─ tz::rfc3339_to_local_min(...)    (タイムゾーン変換)
                 └─ db.upsert(table, "external_id", ext_id, fields)
```

`gcal` モジュール全体は **`gcal` feature flag** 配下に置き、デフォルト features に含める一方、`--no-default-features` でビルドする人は外せるようにする（重い依存を持ち込むため。詳細は §8）。

## 3. スキーマ変更

### 3.1 tasks に追加

| field | type | nullable | 用途 |
|---|---|---|---|
| `external_id` | `str` | true | `gcal:<calendarId>:<eventId>` 形式。同じ event の再 import 時に行を更新するキー |

### 3.2 recurrences に追加

| field | type | nullable | 用途 |
|---|---|---|---|
| `external_id` | `str` | true | `gcal:<calendarId>:<eventId>` 形式（繰り返しイベントは主 event UID を保持） |

### 3.3 pattern_data の拡張

現状の `PatternData` は `{ "days": [..] }` のみ。これに以下を追加。

```rust
pub struct PatternData {
    pub days: Option<Vec<i8>>,        // 既存。pattern により解釈が変わる（後述）
    pub interval: Option<u8>,         // 新規 (INTERVAL=N。未指定=1)
    pub setpos: Option<i8>,           // 新規 (BYSETPOS。1=第1, 2=第2, -1=最終 …)
}
```

`days` の解釈（pattern と `setpos` の組み合わせで変わる）:

| pattern | setpos | days の意味 | 例 |
|---|---|---|---|
| `daily` | — | 未使用 | — |
| `weekly` | — | 曜日番号 (1=MO, 2=TU, … 7=SU) | `[1,3,5]` = 月水金 |
| `monthly` | None | BYMONTHDAY (1-31) | `[15]` = 毎月15日 |
| `monthly` | Some(N) | BYDAY 曜日番号 (1-7) | `days=[2], setpos=2` = 第2火曜 |

互換維持: 既存の `monthly` で `setpos=None` なら従来通り `days` を BYMONTHDAY として扱う。**`month_days` 等の追加フィールドは YAGNI のため当面追加しない。** 将来「BYDAY と BYMONTHDAY を併用したい RRULE」が来たら拡張検討。

`days` の型を `Vec<u8>` → `Vec<i8>` に変更（`setpos` の負値統一とは独立だが、将来 BYDAY の負値（例: `-1MO`=最終月曜）にも対応しやすくする）。既存データへの互換性は serde が JSON 整数を i8 として読めるので問題なし。

### 3.4 init.rs / db.rs / migration

`apply_schema` は `create_table` + `add_field` の組み合わせだが、いずれも **冪等ではない**（既存テーブル/フィールドで `Error::TableExists` / `Error::FieldExists` を返す）。既存ユーザーの DB に対しては、新規スキーマ追加分だけを差分適用する **migration 関数** を別途用意する。

```rust
// src/init.rs に追加
pub fn migrate_schema(db: &mut ybasey::Database) -> Result<()> {
    add_field_if_absent(db, "tasks", "external_id", "str", true)?;
    add_field_if_absent(db, "recurrences", "external_id", "str", true)?;
    Ok(())
}

fn add_field_if_absent(
    db: &mut ybasey::Database,
    table: &str,
    name: &str,
    type_spec: &str,
    nullable: bool,
) -> Result<()> {
    if db.table(table)?.schema.has_field(name) {
        return Ok(());
    }
    db.add_field(table, name, type_spec, nullable)?;
    Ok(())
}
```

`Database::table(name)?.schema` は pub、`Schema::has_field(name)` も pub であることを確認済み（`ybasey/src/schema/mod.rs:56`）。ybasey 側の変更は不要。

`main.rs` の起動経路で `migrate_schema(&mut db)` を一度実行する。`apply_schema`（init 時）にも `external_id` 追加を含めるので、新規 init と既存 DB の両方で整合する。

`record_to_task` / `record_to_recurrence` も `external_id` を `Option<String>` で読むよう更新する。

## 4. RRULE → ytasky pattern マッピング

GCal が返す `recurrence` フィールドは `["RRULE:...", "EXDATE:...", "RDATE:..."]` の配列。RRULE 1 本を主とし、それ以外は `recurrence_exceptions` または個別タスクで補う。

### 4.1 サポートマトリクス

| RRULE | ytasky 表現 | 備考 |
|---|---|---|
| `FREQ=DAILY` | `pattern=daily` | |
| `FREQ=DAILY;INTERVAL=N` | `pattern=daily, interval=N` | |
| `FREQ=WEEKLY;BYDAY=MO,WE,FR` | `pattern=weekly, days=[1,3,5]` | |
| `FREQ=WEEKLY;INTERVAL=N;BYDAY=…` | `pattern=weekly, interval=N, days=…` | |
| `FREQ=MONTHLY;BYMONTHDAY=15` | `pattern=monthly, days=[15]` | `setpos=None` ⇒ days を BYMONTHDAY として保存 |
| `FREQ=MONTHLY;BYDAY=2TU` | `pattern=monthly, days=[2], setpos=2` | "第 2 火曜" |
| `FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2` | `pattern=monthly, days=[2], setpos=2` | 同上 |
| `FREQ=MONTHLY;BYDAY=-1FR` | `pattern=monthly, days=[5], setpos=-1` | "最終金曜" |
| `UNTIL=YYYYMMDD…` | `end_date` | RRULE 側 UNTIL を recurrence.end_date に反映 |
| `COUNT=N` | `end_date` に換算 | start_date から N 回展開した最終日を end_date とする |
| `EXDATE:YYYYMMDD` | `recurrence_exceptions` 1 行 | |
| `RDATE` | 個別 task として追加 | recurrence と無関係に独立 task |
| `FREQ=YEARLY` | **未対応** → instances 展開 | §4.2 参照 |
| `FREQ=WEEKLY;BYWEEKNO=…` | **未対応** → instances 展開 | |
| `BYMONTH` 併用 / `BYDAY` + `BYMONTHDAY` 同時 等 | **未対応** → instances 展開 | |

### 4.2 未対応 RRULE のフォールバック

未対応 RRULE のイベントは `events.instances` エンドポイント（`GET /calendars/{calendarId}/events/{eventId}/instances?timeMin=…&timeMax=…`）で当該イベントの instance 群のみを取得し、それぞれを `tasks` に `fixed_start` 付きで挿入する。external_id は instance 単位の `gcal:<calendarId>:<eventId>_<originalStartTime>` を使う（GCal が返す `id` を流用）。

`events.list?singleEvents=true` を全件再取得する案は、既に取得済みの単発イベントも展開されて重複するため採用しない。

### 4.3 RRULE パーサ

GCal の RRULE は RFC 5545 のサブセット。クレートを使わず手書き（200 行程度）にする。`KEY=VALUE` を `;` 区切りで分解し、上記マトリクスに該当しない要素が混じったら `Err(Unsupported)` を返してフォールバックさせる。

### 4.4 既存 `matches_recurrence_pattern` の改修

現状 `matches_recurrence_pattern(pattern, pattern_data, target_date)` は `interval` と `setpos` を扱えない。シグネチャを **`Recurrence` 全体を受け取る形** に変更する:

```rust
pub fn matches_recurrence_pattern(
    rec: &Recurrence,
    target_date: NaiveDate,
) -> Result<bool>;
```

変更後のロジック（擬似コード）:

```text
start = parse(rec.start_date)
interval = rec.pattern_data.interval.unwrap_or(1)
match rec.pattern:
  "daily":
    delta_days = target - start
    return delta_days >= 0 && delta_days % interval == 0

  "weekly":
    delta_days = target - start
    if delta_days < 0: return false
    delta_weeks = delta_days / 7
    if delta_weeks % interval != 0: return false
    return days.contains(target.weekday_1to7())

  "monthly":
    delta_months = (target.year - start.year) * 12 + (target.month - start.month)
    if delta_months < 0 || delta_months % interval != 0: return false

    match setpos:
      None:
        return days.contains(target.day())           # BYMONTHDAY
      Some(n):
        target_weekday = target.weekday_1to7()
        if !days.contains(target_weekday): return false
        # target が「月内で n 番目の当該曜日」か判定
        if n > 0:
          nth_occurrence = (target.day() - 1) / 7 + 1
          return nth_occurrence == n
        else:  # n < 0: 月末から逆算
          last_day_of_month = days_in_month(target)
          remaining = last_day_of_month - target.day()
          nth_from_end = remaining / 7 + 1
          return nth_from_end == -n
```

`generate_dates_for_recurrence` は引き続き `from..=to` を 1 日ずつ回す方式で OK（範囲が広くないため）。呼び出し側は `matches_recurrence_pattern(rec, d)` に変更。

## 5. OAuth 2.0 Loopback Redirect + PKCE

### 5.1 フロー

1. `~/.config/ytasky/gcal.json` を読込（無ければ同梱 client_id/secret を使用）
2. 32 byte random で `code_verifier` 生成 → SHA256 → base64url で `code_challenge`
3. ローカルの空きポートを OS に割り当てさせて tiny_http サーバー起動（127.0.0.1:N）
4. ブラウザで認可 URL を開く:
   ```
   https://accounts.google.com/o/oauth2/v2/auth
     ?client_id=…
     &redirect_uri=http://127.0.0.1:N/cb
     &response_type=code
     &scope=https://www.googleapis.com/auth/calendar.readonly
     &code_challenge=…
     &code_challenge_method=S256
     &access_type=offline
     &prompt=consent
     &state=<32byte random>
   ```
5. `http://127.0.0.1:N/cb?code=…&state=…` を受信
   - **state 検証**: ステップ4で生成した値と完全一致しなければ 400 を返してエラー
   - **タイムアウト**: 120 秒以内に受信しなければサーバー停止し「タイムアウト」エラー
6. code を `https://oauth2.googleapis.com/token` に POST
7. response の `access_token` / `refresh_token` / `expires_in` を保存

### 5.2 同時実行 / ロック

OAuth フローの並行起動を防ぐため、`~/.config/ytasky/gcal_login.lock` を `fs2::FileExt::try_lock_exclusive` で取得する。取得失敗時は「別の ytasky プロセスが認証中」と表示してフロー中止。`tiny_http` のポートは OS 任せ（ポート競合は発生しない）。

token が既に有効ならフロー全体をスキップ（lazy sync の重複ログインを抑制）。

### 5.3 Credential / Token の保管

`~/.config/ytasky/gcal.json` (rclone 型 — 同梱をデフォルト、ユーザー上書き可):

```json
{
  "client_id": "...",
  "client_secret": "...",
  "auth_uri": "https://accounts.google.com/o/oauth2/v2/auth",
  "token_uri": "https://oauth2.googleapis.com/token"
}
```

`~/.config/ytasky/gcal_token.json`:

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "expires_at": 1747400000,
  "scope": "https://www.googleapis.com/auth/calendar.readonly"
}
```

Unix では `chmod 600` を設定。Windows ではユーザープロファイル配下の ACL に依存する（追加保護なし）。

### 5.4 同梱 client_id/secret の方針

- Google Cloud Console で「ytasky」プロジェクトを作成し、Desktop app 種別の OAuth client を発行
- ソースに埋め込む（コンパイル時定数）
- OSS 公開済みでも Calendar API スコープに限られるため影響は限定的（Google もデスクトップアプリでは secret を機密扱いしていない）
- ユーザーが `~/.config/ytasky/gcal.json` を置けばそちらが優先

### 5.5 Refresh

`access_token` の `expires_at` を毎回確認し、過ぎていたら `refresh_token` で更新:

```
POST https://oauth2.googleapis.com/token
  client_id, client_secret, refresh_token, grant_type=refresh_token
```

refresh_token が失効した場合は完全に再ログイン。

### 5.6 Scope

| Scope | 用途 |
|---|---|
| `https://www.googleapis.com/auth/calendar.readonly` | 現フェーズ（import のみ） |
| `https://www.googleapis.com/auth/calendar.events` | 将来 Export 時に拡張 |

## 6. import 実行フロー

```
ytasky import-gcal --from 2026-05-16 --to 2026-06-16 [--calendar primary] [--category personal]
```

### 6.1 手順

1. token 取得（必要なら refresh、無ければ OAuth フロー起動）
2. `events.list(calendarId, timeMin=from, timeMax=to+1day, singleEvents=false, maxResults=2500)`
3. nextPageToken でページネーション完走
4. イベントごとに振り分け:
   - `status=cancelled` → スキップ
   - `start.date` あり（終日） → スキップ（§1 ノンゴール）
   - `recurrence` あり:
     - RRULE が `rrule::parse_to_pattern` でサポート範囲内 → `recurrences` を `external_id` で upsert
     - 範囲外 → `events.instances` で当該イベントの instance 群を取得し、各 instance を `tasks` upsert（§4.2）
   - `recurrence` 無し → `tasks` を `external_id` で upsert
5. summary 表示 (`created / updated / skipped` 件数)

### 6.2 Upsert ロジック

ybasey の `Database::upsert(table, key_field, key_value, fields)` を直接利用する:

```rust
let (id, was_inserted) = db.upsert(
    "tasks",
    "external_id",
    &format!("gcal:{}:{}", calendar_id, event_id),
    fields,
)?;
```

WAL ベースで原子性が保証される。find + insert/update の 2 ステップは使わない。

ytasky 側で手動編集したい場合は `external_id` を NULL にして link 解除する運用（将来 TUI に「detach from GCal」コマンドを用意）。

### 6.3 GCal 側削除の扱い

- 「`status=cancelled` → スキップ」は **繰り返しイベントの個別 instance キャンセル** に対してのみ有効（その instance を recurrence_exceptions に登録するかは将来検討）
- **完全削除されたイベント**は `events.list` に出現しないため検知不能。ytasky 側にデータが残り続ける
- 将来対応案: `events.list?showDeleted=true&updatedMin=…` で差分同期、または「最後の import 日時より古い external_id を持つ task」を孤児として一覧表示する機能を追加

### 6.4 カテゴリ

- `--category <id>` で指定（デフォルト: 設定ファイルの `gcal_default_category`、未設定なら "personal" 相当）
- GCal の colorId からの自動推定はしない
- 後でユーザーが TUI でカテゴリ変更する前提

### 6.5 タイムゾーン処理

GCal は `start.dateTime` を RFC3339 (`2026-05-16T10:00:00+09:00`) で返す。ytasky の `fixed_start` は「分」の整数（0 = 00:00、0..1440 を想定）。

変換手順:

1. `chrono::DateTime::parse_from_rfc3339(&s)` → `DateTime<FixedOffset>` （イベント側 tz）
2. `_meta.tz` を読み込み（例: `Asia/Tokyo`）。未設定ならシステム TZ（`chrono::Local`）にフォールバック
3. `chrono_tz::Tz` で解釈し、`DateTime<Tz>` に変換
4. ローカル日付 (`NaiveDate`) と分 (`hour*60 + minute`) を抽出
5. ytasky の `tasks.date` と `fixed_start` に投入

`chrono_tz` クレートは依存追加が必要（§8）。`_meta` パースは ybasey API ではなく `~/.local/share/ytasky-ybasey/_meta` を直接読む（既存 `write_meta` と対称）。

複日跨ぎイベント（`start` と `end` の日付が異なる）はノンゴール（§1）。

## 7. 操作インタフェース

### 7.1 CLI

```
ytasky import-gcal [--from YYYY-MM-DD] [--to YYYY-MM-DD]
                   [--calendar <id>] [--category <id>]
ytasky gcal-login   # OAuth フローのみ実行（token 取得）
ytasky gcal-logout  # token 削除
```

- `--from` 省略時: 今日
- `--to` 省略時: 今日+30 日

### 7.2 MCP

```
mcp__ytasky__import_gcal
  from_date: str           (default: today)
  to_date: str             (default: today + 30d)
  calendar_id?: str        (default: "primary")
  category?: str           (default: 設定値)

  → { created: N, updated: N, skipped: N }
```

### 7.3 TUI

- キーバインド: `Shift+G` で「Import from Google Calendar」ダイアログを開く
- ダイアログ: from / to / calendar / category を入力
- 実行中はステータスバーに進捗、完了時にトースト「GCal: 12 created, 3 updated, 2 skipped」

### 7.4 起動時 lazy sync

- TUI は `crossterm::event::poll` ベースの同期イベントループ。tokio runtime を新規に作らず **`std::thread::spawn`** で別スレッド起動する
- スレッド内で **新規 `ybasey::Database::open(...)`** を行う（メインスレッドの DB ハンドルとは独立）
- HTTP クライアントは `reqwest::blocking::Client` を使う（tokio 不要）
- token が無い／無効ならスキップ（再ログインを促さない）
- 期間: 今日 〜 7 日後（デフォルト、設定で変更可）
- 結果は `mpsc::Sender<SyncResult>` 経由でメインスレッドへ送信し、TUI イベントループの定期 pull でトースト化
- エラー時もトーストのみ。TUI は止めない
- 設定: `~/.config/ytasky/config.json` の `gcal_auto_sync: bool` で OFF 可

#### DB 排他

ybasey が複数 `Database` インスタンスを同時に開いた際の排他は `ybasey/src/storage/lock.rs` の仕組みに従う（ファイルロック）。lazy sync 側でロック取得に失敗したら「次回起動時に再試行」として終了し、メイン側を阻害しない。

具体的なロック挙動の挙動確認は M7 着手前に ybasey 側コードを読んで確認する（必要なら ybasey に `try_open()` を追加）。

## 8. 依存追加 / feature flag

```toml
[features]
default = ["gcal"]
mcp = ["dep:rmcp", "dep:tokio", "dep:schemars"]
gcal = [
  "dep:reqwest", "dep:tiny_http", "dep:base64", "dep:sha2",
  "dep:rand", "dep:url", "dep:webbrowser", "dep:chrono-tz", "dep:fs2",
]

[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "blocking", "json"], optional = true }
tiny_http = { version = "0.12", optional = true }
base64 = { version = "0.22", optional = true }
sha2 = { version = "0.10", optional = true }
rand = { version = "0.9", optional = true }
url = { version = "2", optional = true }
webbrowser = { version = "1", optional = true }
chrono-tz = { version = "0.10", optional = true }
fs2 = { version = "0.4", optional = true }
```

**tokio は non-optional に昇格しない**。`reqwest::blocking` + `std::thread::spawn` で全てカバー可能。MCP feature 専用のままにすることで、MCP を使わないユーザーは tokio リンク無しで済む。

`gcal` feature は default に含めるが、`--no-default-features` でビルドする人は完全に外せる。これにより:

- 一般ユーザー（バイナリ配布）: gcal 込み、サイズ増を許容
- 軽量ビルド希望者: `cargo build --no-default-features --features mcp` 等で除外可

`src/gcal/` モジュールは `#[cfg(feature = "gcal")]` で囲み、CLI/MCP/TUI 側も該当コマンド/キー/MCP tool を feature gate する。

## 9. Google Cloud Console 準備手順（README 反映用）

ytasky 同梱の client_id/secret で使う場合、ユーザー側の準備は不要。自前で用意したい場合のみ:

1. https://console.cloud.google.com/ でプロジェクトを作成
2. **APIs & Services > Library** で **Google Calendar API** を有効化
3. **APIs & Services > OAuth consent screen** で External / Testing を選択
4. Test users に自分の Google アカウントを追加
5. **APIs & Services > Credentials** で **Create Credentials > OAuth client ID**
   - Application type: **Desktop app**
   - Name: `ytasky`
6. 表示された JSON をダウンロードし、`~/.config/ytasky/gcal.json` に配置

## 10. テスト戦略

- `tests/gcal_rrule.rs`: RRULE → pattern_data 変換の純粋関数テスト。次のパターンを最低限カバー:
  - `FREQ=DAILY`, `FREQ=DAILY;INTERVAL=3`
  - `FREQ=WEEKLY;BYDAY=MO,WE,FR`, `FREQ=WEEKLY;INTERVAL=2;BYDAY=TU`
  - `FREQ=MONTHLY;BYMONTHDAY=15`, `FREQ=MONTHLY;BYDAY=2TU`, `FREQ=MONTHLY;BYDAY=-1FR`, `FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2`
  - `UNTIL=20261231T000000Z`, `COUNT=10`, `EXDATE`, `RDATE`
  - 未対応パターンが `Err(Unsupported)` を返すこと（`FREQ=YEARLY`, `BYWEEKNO`, `BYMONTH` 併用）
- `tests/gcal_recurrence.rs`: 新 `matches_recurrence_pattern(rec, date)` の境界テスト
  - `interval=2` の隔日/隔週/隔月
  - `setpos=2`, `setpos=-1`（月末絡み: 31日月と30日月の両方）
  - start_date より前の date は false
- `tests/gcal_import.rs`: in-memory ybasey DB に固定 fixture JSON の events.list レスポンスを流し込んで upsert を検証
  - 新規 insert / 既存 update / cancelled スキップ / 終日スキップ / 未対応 RRULE のフォールバック
- OAuth フロー: 手動テスト（HTTP server のモック化は工数に見合わない）
- API クライアント: reqwest を `wiremock` でモック（または手書きの localhost HTTP server）

## 11. マイルストーン

| # | スコープ | 概算 | 依存 |
|---|---|---|---|
| **M1** | スキーマ拡張: `external_id` 列、`init.rs` の `apply_schema` 更新、`migrate_schema()` 関数、`db.rs` の `record_to_*` 反映、`main.rs` 起動経路に migration 呼び出し追加、テスト | 1〜2 日 | — |
| **M2** | recurrence pattern_data 拡張（`interval` / `setpos`）+ `days` を `Vec<i8>` 化 + `matches_recurrence_pattern` のシグネチャ変更 + 既存呼び出し全箇所更新 + UI / CLI / MCP / テスト | 3〜4 日 | M1 |
| **M3** | OAuth (`gcal/auth.rs`): PKCE + Loopback + token 保管/refresh + state 検証 + タイムアウト + ロックファイル + 同梱 credential 埋め込み | 2〜3 日 | — |
| **M4** | `gcal/api.rs`: events.list / events.instances / calendarList.list クライアント + `gcal/tz.rs` のタイムゾーン変換 | 2 日 | M3 |
| **M5** | `gcal/rrule.rs` + `gcal/import.rs`: 変換と upsert | 2〜3 日 | M2, M4 |
| **M6** | CLI / MCP / TUI 統合 | 1〜2 日 | M5 |
| **M7** | 起動時 lazy sync（std::thread + 別 DB ハンドル + mpsc + ロック確認） | 1〜2 日 | M6 |

合計 12〜18 日。各 M を 1〜2 コミット粒度で進める。

## 12. 将来拡張

- GCal への Export（書き込み）
- 完全削除イベントの検知（`updatedMin` 差分同期 or 孤児リスト）
- 複数カレンダーの並列 import
- 終日イベントの取り込み（複日跨ぎ分割を含む）
- `month_days`（BYDAY と BYMONTHDAY 併用 RRULE）対応
- 編集競合検出（etag ベース）
- ytasky 側変更を GCal に push back（双方向同期）
- Outlook / iCloud / CalDAV 連携（同じ抽象を再利用）
