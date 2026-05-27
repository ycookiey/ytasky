# GitHub Issue Import

GitHub の issue を ytasky の backlog タスクへ取り込む機能の設計書。

GCal import（`docs/gcal-import.md`）で確立したパターン（`external_id` による重複防止、CLI/MCP/TUI/lazy sync の4経路、`Option<feature>` オプトイン）を踏襲する。OAuth を使わず `gh` CLI のトークンを流用するため、GCal より実装は軽い。

## 1. ゴール

- GitHub の検索クエリ（`is:open assignee:@me` 等）に一致する open issue を backlog タスクとして取り込む
- import のみ（双方向同期・状態同期はしない）
- 2回目以降の import で、ユーザーが手で設定した列（category 等）を壊さず、issue 由来の列（title 等）だけ最新化する
- 認証は `gh` CLI に委譲し、ytasky 側でトークンを保管しない

非ゴール:
- task → issue の起票（export）
- issue close → task 完了 等の状態同期
- issue body の取り込み（Task に格納列がない）

## 2. アーキテクチャ

```
src/github/
  mod.rs     GithubConfig, load_config（config.json 相乗り）, external_id ヘルパ
  auth.rs    gh auth token 取得（サブプロセス）+ gh 未導入/未ログインのエラー
  api.rs     GET /search/issues 呼び出し + Link ヘッダ pagination + 429 retry
  types.rs   SearchResult / Issue / Milestone の serde 型
  import.rs  import_queries, ImportSummary, 列所有権 upsert
```

- feature flag `github`、`default = ["gcal", "github"]`
- 依存は reqwest（gcal と共有）のみ。OAuth 系ライブラリ不要

## 3. スキーマ変更

なし。GCal import で追加済みの `tasks.external_id` 列を共用する（`recurrences.external_id` も存在するが GitHub import は tasks のみ使う）。

`external_id` は連携元が混在するため prefix で名前空間を分ける:

| 連携元 | external_id 形式 |
|---|---|
| GCal | `gcal:{calendarId}:{eventId}` |
| GitHub | `github:{owner}/{repo}#{number}` |

prefix（`gcal:` / `github:`）により両者が衝突しない。

## 4. 認証（`github/auth.rs`）

`gh` CLI のトークンを流用する。ytasky 自身はトークンを保管・更新しない。

```
fn get_token() -> Result<GithubToken>   // GithubToken は Debug を手動実装し [REDACTED] で隠す
```

1. `Command::new("gh").args(["auth", "token"])` をサブプロセス実行（引数は固定文字列のみ。ユーザー入力を引数に渡す経路はなくコマンドインジェクションの余地はない）
2. exit code と stdout を検査し、トークン文字列を `GithubToken` で包んで返す（呼び出し側が API の Bearer に使う）

セキュリティ:
- `GithubToken` は GCal の `Credential`/`Token` と同様に `Debug` を手動実装し、ログ・エラーチェーンに平文が出ないよう `[REDACTED]` でマスクする
- エラーメッセージにトークンを含めない。stderr はそのままログに出してよい（トークンは stdout 側）

エラーハンドリング:
- `gh` が PATH に無い（spawn 失敗）→ 「`gh` をインストールして `gh auth login` してください」
- `gh auth token` が非ゼロ終了（未ログイン）→ 「`gh auth login` で GitHub にログインしてください」

トークン管理（保管・更新・スコープ）は `gh` に委譲するため、自前 token.json を持たない＝GCal の Windows ACL リスクも無い。

### 認証方式の選定理由

| 方式 | 判定 | 理由 |
|---|---|---|
| gh CLI 流用 | ★採用 | 実装最小、既にログイン済みが多い、トークン管理を委譲 |
| PAT（env/config）| 不採用 | PAT 発行・失効管理の手間、平文保存リスク |
| OAuth Device Flow | 不採用 | OAuth App 登録＋secret 配布が必要で過剰（import は読み取りのみ）|

gh 未導入環境では動かないが、ytasky は個人の開発マシンで使う前提で、gh は導入済みのことが多い。

## 5. import 対象の指定（config.json）

`~/.config/ytasky/config.json` に GitHub 検索構文の配列で指定する（GCal の設定と同じファイルに相乗り）。

```json
{
  "github_import_queries": [
    "is:open assignee:@me",
    "repo:ycookiey/ytasky is:open label:bug"
  ],
  "github_auto_sync": true
}
```

- `GET /search/issues?q=<query>` にそのまま渡す。GitHub Web UI / `gh issue list` と同じ構文が使える
- `is:open` を含めることで「open のみ取り込み」を満たす（ユーザー責任。含めない場合は closed も入りうる旨を README に明記する）
- 複数クエリは順に実行し、結果を external_id で重複排除してマージ

`GithubConfig`（`github/mod.rs`）:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct GithubConfig {
    #[serde(default)]
    pub github_import_queries: Vec<String>,
    #[serde(default = "default_true")]
    pub github_auto_sync: bool,
}
```

- `github::load_config()` を独立に実装し、GCal と同じ `config.json` を `GithubConfig` へ部分デシリアライズする（`gcal::load_config()` は `GcalConfig` へデシリアライズ。両者が同一ファイルを別構造体で読む）
- **制約**: `GcalConfig` / `GithubConfig` のいずれにも `#[serde(deny_unknown_fields)]` を付けないこと。付けると相手のフィールドで失敗する

## 6. import 実行フロー（`github/import.rs`）

### 6.1 手順

1. `auth::get_token()` でトークン取得
2. クエリごとに `api::search_issues(token, query)` を呼び pagination 完走
3. 全 issue を external_id で重複排除（複数クエリで同一 issue がヒットしても1件に）
4. 各 issue を `handle_issue` で backlog タスクへ upsert
5. `ImportSummary { created, updated, skipped, errors }` を返す

### 6.2 フィールドマッピングと列所有権

| task 列 | 由来 | 既存行での挙動 |
|---|---|---|
| title | `#{number} {sanitize_text(issue.title)}` | 毎回更新（issue 所有）|
| deadline | `milestone.due_on` を `YYYY-MM-DD` へ変換（無ければ null）| 毎回更新（issue 所有）|
| external_id | `github:{owner}/{repo}#{number}` | 不変キー |
| category_id | デフォルト `"3"`（開発・仕事）| 初回 insert のみ。以降保持（ユーザー所有）|
| duration_min | デフォルト `30` | 同上（ユーザー所有）|
| is_backlog | 初回 `true` | import は触らない（schedule へ移動しても戻さない）|
| fixed_start | null | import は触らない |

補足:
- **title の sanitize**: GCal import と同じ `sanitize_text()`（ANSI escape / BIDI override / 制御文字を除去）を適用する。`sanitize_text` は現状 `gcal/import.rs` にあるため、共通モジュール（例 `src/text.rs` か `src/util.rs`）へ移動して両者で共用する
- **deadline 変換**: `milestone.due_on` は ISO 8601（例 `2026-06-15T07:00:00Z`）。milestone は「日」粒度のため日付のみを採り `YYYY-MM-DD` にする（時刻は切り捨て）
- **title 重複 prefix**: issue.title が既に `#123` で始まる場合 `#123 #123 ...` になりうる。実害は小さいため許容（将来 prefix 重複を検出してスキップする余地あり）

### 6.3 upsert ロジック（列所有権分割）

GCal の全列 upsert（`db.upsert("tasks", "external_id", ...)`）と異なり、**既存行は issue 所有列だけ UPDATE する**。ybasey の既存 API で2段階に実現する:

```
ext = "github:{owner}/{repo}#{number}"
既存 = table.find_by_field("external_id", ext)   // GCal の task_exists_by_external_id と同手法
  既存なし（新規）:
    backlog タスクとして insert → created++
    （既存の db::insert_backlog_task と同じ必須列: date=BACKLOG_DATE("9999-12-31"),
      sort_order=次の backlog 順, status="todo", is_backlog=true。これに
      title, category="3", duration=30, deadline, external_id を加える。
      実装は insert_backlog_task に deadline+external_id を足した関数を新設する）
  既存あり（id 取得）:
    db.update("tasks", id, [("title", ...), ("deadline", ...)])  // 部分更新。他列は SET しない
    → updated++
```

- ybasey の `Database::update(table, id, fields)` は渡したフィールドだけ更新する（`db.rs::update_task_fields_only` が同パターンで実在）。これにより category/duration/配置を保持できる
- 編集追跡のような仕組みは不要。「既存なら title/deadline だけ書く」だけで列所有権が成立する

### 6.4 カテゴリ

import 時のデフォルトは id=`"3"`（開発・仕事）。`--category` で上書き可能。初回 insert のみ適用し、以降ユーザーが変更した値を保持する。

## 7. 操作インタフェース

### 7.1 CLI

```
ytasky import-github [--query <検索構文>]... [--category <id>]
```

- `--query` 省略時は config の `github_import_queries` を使う
- `--query` 指定時はそれを優先（複数指定可）
- 結果は GCal import と同じく JSON（created/updated/skipped/errors）で出力

### 7.2 MCP

`import_github` tool を追加（`#[cfg(feature = "github")]`）。引数は queries（省略可）と category（省略可）。

### 7.3 TUI + background sync の汎用化

既存の `gcal_sync_rx`（`App` の `Option<Receiver<GcalSyncMessage>>` 単一フィールド）は GCal 専用で、GitHub と同時に走ると衝突する。これを**汎用 `SyncMessage` に統一**し、GCal 側も新型へ移行する:

```rust
pub(crate) struct SyncMessage {
    pub source: &'static str,             // "gcal" | "github" — 二重起動ガードと表示ラベルに使う
    pub status: anyhow::Result<String>,   // 整形済み表示文字列
}

struct App {
    // gcal_sync_rx を廃し、複数ソースを1箇所で扱う
    sync_rx: Vec<std::sync::mpsc::Receiver<SyncMessage>>,
}
```

- **ImportSummary は共通化しない**。各 import 関数（`gcal::import::import_range` / `github::import::import_queries`）は従来どおり自モジュールの `ImportSummary` を返す（CLI/MCP はこれを JSON 出力に使う）。background sync では、各ソースが自分の `ImportSummary` を表示用文字列に整形してから `SyncMessage.status` に詰めて送る。これにより GCal の RRULE 固有フィールド（`skipped_exdates` / `skipped_rdates`）を共通型に押し込む歪みを避ける
- `poll_background_sync` は `sync_rx` の全 Receiver を走査し、`source` をラベルに `status` 文字列を表示する（"GCal auto-sync: ..." / "GitHub auto-sync: ..."）
- `kick_*_import_background` / `spawn_*_lazy_sync` は各ソースの Receiver を `sync_rx` に push する。同一ソースの二重起動ガードは「その source の rx が生存中か」で判定する
- GCal の `GcalSyncMessage`（`result` のみ）は `SyncMessage`（`source` + `status`）へ移行する
- TUI のキーバインドは dispatch table（`app.rs` の `Binding`）に GitHub import を追加（GCal import キーの隣）。下部キーバインドバー表示は dispatch table から自動生成される

### 7.4 起動時 lazy sync

`github_auto_sync`（default true）が有効なら、TUI 起動時に background で import を実行し、Receiver を `sync_rx` に push する。GCal lazy sync と同じ枠組み（`spawn_*_lazy_sync`）に載せる。GitHub は open issue 全件が対象のため「同期日数」の概念は持たない。

## 8. 依存 / feature flag

```toml
[features]
default = ["gcal", "github"]
github = ["dep:reqwest"]  # gcal と共有。既に optional dep として存在
```

- reqwest は gcal が既に要求済み（`rustls-tls, blocking, json`）。github も同じ blocking client を使う
- query パラメータは reqwest の `RequestBuilder::query()` で渡す（自動 URL エンコード。`percent-encoding` への直接依存は不要）
- 追加の新規 crate は不要

## 9. GitHub Search API の制約（`github/api.rs`）

`GET /search/issues` は GCal の events.list と pagination 方式が異なる。以下を実装に反映する:

| 項目 | 仕様 | 対応 |
|---|---|---|
| 1ページ件数 | デフォルト 30 / 最大 100 | `per_page=100` を指定 |
| pagination | `Link` ヘッダの `rel="next"` URL を辿る（`nextPageToken` 方式ではない）| Link ヘッダをパースして次ページ取得 |
| 結果上限 | **最大 1000 件**（API の硬制限）| 1000 件到達時はそれ以上取れない旨を警告し、クエリ分割を促す |
| rate limit | 認証済みで 30 req/min（secondary rate limit あり）| `MAX_PAGES=10`（100×10=1000）でガード |
| 429 | `Retry-After` ヘッダ | GCal と同じく Retry-After 尊重で1回 retry（上限秒数を設ける）|
| 401 | トークン失効/無効 | 「`gh auth login` で再ログイン」を案内 |

## 10. テスト

- external_id 生成（`github:{owner}/{repo}#{number}`）の単体テスト
- 列所有権 upsert: 新規=insert、既存=title/deadline のみ update で category/duration が保持されることの検証
- title prefix（`#123 ...`）整形 + sanitize_text 適用のテスト
- config 部分デシリアライズ（github_* のみ / 空オブジェクト / GcalConfig と同居）のテスト
- API 型の serde デシリアライズ（search レスポンスの代表 JSON、milestone あり/なし）
- milestone.due_on（ISO8601）→ `YYYY-MM-DD` 変換のテスト
- Link ヘッダ `rel="next"` パースのテスト

## 11. 実装計画（M 分割）

- **M1**: feature flag + モジュール骨格 + GithubConfig/load_config + external_id ヘルパ + types
- **M2**: auth（gh token 取得 + Debug マスク）+ api（search_issues + Link pagination + 429 retry）
- **M3**: import（列所有権 upsert）+ sanitize_text 共通化 + due_on 変換 + CLI `import-github`
- **M4**: MCP `import_github` tool
- **M5a**: background sync 汎用化（`SyncMessage` 統一）+ 既存 GCal の移行（既存動作を保持するテスト付き）
- **M5b**: GitHub の TUI キーバインド + background import + lazy sync
- 各 M 完了時にテスト通過を確認し commit

## 12. 将来拡張

- task → issue 起票（export）
- 状態同期（issue close → task 完了 / task 完了 → issue close）
- `--force-update`（列所有権を無視して全列上書き）
- issue body の取り込み（Task に description 列を追加する場合）
- title の重複 prefix（`#123 #123`）検出スキップ
