[English README](README.md)

# ytasky

ターミナルで使う時間ブロック型のスケジューラ。1日のタスクを順に並べて所要時間を入れると、各タスクの開始時刻を前のタスクの終了時刻から決める。開始時刻を固定したタスクは、固定した時刻に置く。TUI・コマンドライン・MCPサーバーの3つの方法で操作できる。MCPサーバーを使うと、AIエージェントからも操作できる。

## 特徴

- 日ごとの列にタスクを予定の開始時刻とともに表示する。`t`で、1日を時間軸上のブロックで描くタイムラインに切り替える
- 24時間から予定の所要時間の合計を引いた残りの時間を、タイトルバーに表示する
- 予定と実績の時刻を記録する。`Space`を押すたびに、実績の開始時刻、終了時刻の順に記録する
- 日付の決まっていないタスクをバックログに置き、任意の日に移す。バックログのタスクには期限を付けられる
- 繰り返しのタスク: 毎日・毎週・毎月、N日・N週・Nか月ごと、毎月の第N曜日か最終曜日
- カテゴリ別・タスク名別に、予定と実績の時間を集計するレポート
- 結果をJSONで出力するCLIのサブコマンドと、MCPサーバー`ytasky mcp`。MCPサーバーには、タスク・バックログ・繰り返し・レポート・取り込みのツールがある
- Google カレンダーとGitHubのissueからの取り込み。Google カレンダーには読み取りの権限だけを求める

## インストール

対応環境はWindowsのx64。[Scoop](https://scoop.sh/)でインストールする。

```powershell
scoop bucket add yscoopy https://github.com/ycookiey/yscoopy
scoop install yscoopy/ytasky
```

または、[Releases](https://github.com/ycookiey/ytasky/releases)から`ytasky.exe`をダウンロードし、`PATH`の通ったフォルダに置く。

最初に一度、データベースを作る。

```powershell
ytasky init
```

`init`は作ったフォルダのパスを表示する。データを別のフォルダに置くには、環境変数`YTASKY_DATA_DIR`にフォルダのパスを設定する。設定は`init`のときだけでなく、以後の実行でも必要。

## 使い方

`ytasky`を実行するとTUIが開く。画面の下端の行に、今の画面で使えるキーを表示する。主なキー:

| キー | 動作 |
|-----|--------|
| `j` / `k` | 次・前のタスクを選ぶ |
| `h` / `l` | 前・次の日に移る |
| `1`-`9` | 押した数の日数を横に並べて表示する |
| `a` / `e` / `d` | タスクを追加・編集・削除する |
| `J` / `K` | タスクを下・上に移す |
| `Space` | 実績の開始時刻、終了時刻の順に記録する |
| `R` | 繰り返しのタスクを追加する |
| `Tab` | バックログの欄と日の欄の間でフォーカスを移す |
| `B` / `p` | タスクをバックログに送る・バックログから選んだタスクを日に入れる |
| `T` | バックログの欄で、バックログと繰り返しの一覧を切り替える |
| `t` | 表とタイムラインを切り替える |
| `r` | レポートを開く |
| `G` / `H` | 確認の後、Google カレンダー・GitHubから取り込む |
| `u` / `Ctrl+r` | 元に戻す・やり直す |
| `q` | 終了する |

CLIでも同じ操作ができる。詳細は`ytasky --help`を参照。例:

```powershell
ytasky add --title "Write report" --category 3 --duration 90 --fixed-start 09:00
ytasky list --date 2026-01-15
```

カテゴリは固定のIDで指定する。一覧は`ytasky categories`で表示する。

| ID | 用途 |
|----|-----|
| 1 | 睡眠 |
| 2 | 食事 |
| 3 | 開発・仕事 |
| 4 | 勉強・講義 |
| 5 | 運動 |
| 6 | 身支度・自由時間 |
| 7 | 休憩 |
| 8 | 移動 |
| 9 | 用事・雑務 |

### MCPサーバー

`ytasky mcp`は標準入出力でMCPサーバーを動かす。MCPクライアントの設定の例:

```json
{ "mcpServers": { "ytasky": { "command": "ytasky", "args": ["mcp"] } } }
```

### Google カレンダー

`ytasky gcal-login`を実行するとブラウザが開き、Googleアカウントでログインする。求める権限はカレンダーの読み取りだけ。ログインの後、`ytasky import-gcal`を実行するか、TUIで`G`を押すと、今日から30日後までの予定を、開始時刻を固定したタスクとして取り込む。範囲・カレンダー・カテゴリは`--from`・`--to`・`--calendar`・`--category`で変える。繰り返しの予定は、繰り返しの規則をytaskyで表せるときは繰り返しのタスクとして、表せないときは1回ごとのタスクとして取り込む。終日の予定は取り込まない。同じ予定を再び取り込むと、タスクを増やさずに更新する。

ログイン済みなら、TUIの起動時に、今日から7日後までの予定をバックグラウンドで取り込む。

### GitHubのissue

[GitHub CLI](https://cli.github.com/)をインストールし、`gh auth login`でログインしておく。ytaskyはGitHub CLIのトークンを使う。`config.json`に検索のクエリを書き、`ytasky import-github`を実行するか、TUIで`H`を押す。クエリに一致するissueを、バックログのタスクとして取り込む。バックログにあるタスクのissueがcloseされると、次の取り込みでタスクを完了にする。`ytasky import-github --query "is:open assignee:@me"`のようにクエリを指定すると、`config.json`なしで取り込める。

### 設定

`config.json`は`%APPDATA%\ytasky`に置く。環境変数`YTASKY_CONFIG_DIR`を設定した場合は、`YTASKY_CONFIG_DIR`のフォルダに置く。どの項目も省略できる。

```json
{
  "gcal_auto_sync": true,
  "gcal_auto_sync_days": 7,
  "github_import_queries": ["is:open assignee:@me"],
  "github_auto_sync": true
}
```

- `gcal_auto_sync`・`github_auto_sync`: TUIの起動時にバックグラウンドで取り込むかどうか
- `gcal_auto_sync_days`: 起動時にGoogle カレンダーから取り込む日数

自分で作ったGoogleのOAuthクライアントでログインするには、Google Cloud ConsoleでデスクトップアプリのOAuthクライアントを作り、ダウンロードしたJSONを`config.json`と同じフォルダに`gcal.json`の名前で置く。

## 既知の制限

- 画面は日本語のみ。TUIの表示・`--help`・メッセージ・カテゴリ名が日本語
- カテゴリのアイコンは[Nerd Font](https://www.nerdfonts.com/)の文字。Nerd Font以外のフォントでは四角で表示される
- `ytasky init`はデータのタイムゾーンをAsia/Tokyoにする。Google カレンダーから取り込む時刻と、GitHubのマイルストーンの期日は、データのタイムゾーンに変換する。別のタイムゾーンを使うには、データのフォルダにある`_meta`ファイルの`tz:`の行を、`Europe/Berlin`のようなIANAのタイムゾーン名に書き換える
- Windowsでは、Googleのトークンを設定のフォルダの`gcal_token.json`に平文で保存する。同じユーザーの権限で動くプログラムは、`gcal_token.json`を読める
- `ytasky update`は、Scoopでインストールした場合だけ使える

## アンインストール

1. `ytasky mcp`を動かしているMCPクライアントを終了し、TUIも閉じる。動いている`ytasky.exe`は、Scoopで置き換えも削除もできない
2. `ytasky gcal-logout`でGoogleのトークンを削除する。権限も取り消すには、Googleアカウントのサードパーティとの接続の一覧からytaskyを削除する
3. `scoop uninstall ytasky`を実行する。Releasesからダウンロードした場合は`ytasky.exe`を削除する
4. `ytasky init`が表示したデータのフォルダと、設定のフォルダ`%APPDATA%\ytasky`を削除する

## ライセンス

[MIT License](LICENSE)
