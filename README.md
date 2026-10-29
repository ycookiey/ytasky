[日本語版 README](README.ja.md)

# ytasky

A time-blocking scheduler for the terminal. You list the day's tasks in order with a duration for each, and ytasky chains them: each task starts when the previous one ends, unless you pin it to a fixed start time. Use it from a TUI, from the command line, or from an AI agent through its built-in MCP server.

## Features

- Day columns with planned start times, plus a timeline view (`t`) that draws the day as blocks on a time axis
- The minutes left in the day (24 hours minus the planned durations) in the title bar
- Planned and actual times: `Space` records the actual start, then the actual end
- A backlog of unscheduled tasks with optional deadlines, to move into any day
- Recurring tasks: daily, weekly or monthly, every N days/weeks/months, or the Nth (or last) weekday of the month
- A report of planned and actual minutes by category and by title
- CLI subcommands that print JSON, and an MCP server (`ytasky mcp`) with tools for tasks, the backlog, recurrences, reports and imports
- Import from Google Calendar (read-only) and from GitHub issues

## Install

Windows (x64). With [Scoop](https://scoop.sh/):

```powershell
scoop bucket add yscoopy https://github.com/ycookiey/yscoopy
scoop install yscoopy/ytasky
```

Or download `ytasky.exe` from [Releases](https://github.com/ycookiey/ytasky/releases) and put it on your `PATH`.

Then create the database once:

```powershell
ytasky init
```

`init` prints the folder it created. To keep the data in another folder, set `YTASKY_DATA_DIR` to it, both for `init` and for every later run.

## Usage

Run `ytasky` to open the TUI. The bar at the bottom lists the keys of the current view. The main keys:

| Key | Action |
|-----|--------|
| `j` / `k` | Select the next / previous task |
| `h` / `l` | Move to the previous / next day |
| `1`-`9` | Show that many days side by side |
| `a` / `e` / `d` | Add / edit / delete a task |
| `J` / `K` | Move the task down / up |
| `Space` | Record the actual start, then the actual end |
| `R` | Add a recurring task |
| `Tab` | Switch focus to the backlog panel and back |
| `B` / `p` | Send the task to the backlog / pick a backlog task to insert into the day |
| `T` | In the backlog panel, switch between the backlog and the recurrences |
| `t` | Switch between the table and the timeline |
| `r` | Open the report |
| `G` / `H` | Import from Google Calendar / GitHub, after a confirmation |
| `u` / `Ctrl+r` | Undo / redo |
| `q` | Quit |

The CLI covers the same operations; see `ytasky --help`. For example:

```powershell
ytasky add --title "Write report" --category 3 --duration 90 --fixed-start 09:00
ytasky list --date 2026-01-15
```

Categories are fixed IDs. `ytasky categories` lists them:

| ID | Use |
|----|-----|
| 1 | Sleep |
| 2 | Meals |
| 3 | Work and development |
| 4 | Study and classes |
| 5 | Exercise |
| 6 | Getting ready and free time |
| 7 | Breaks |
| 8 | Commuting |
| 9 | Errands |

### MCP server

`ytasky mcp` runs an MCP server over stdio. Register it in your MCP client, for example:

```json
{ "mcpServers": { "ytasky": { "command": "ytasky", "args": ["mcp"] } } }
```

### Google Calendar

`ytasky gcal-login` opens the browser to sign in with Google and asks for read-only access to your calendars. After that, `ytasky import-gcal` (or `G` in the TUI) imports the events from today to 30 days ahead as tasks with fixed start times; `--from`, `--to`, `--calendar` and `--category` change the range, calendar and category. A recurring event becomes a recurring task when ytasky supports its rule, and one task per occurrence otherwise. All-day events are skipped. Importing again updates the tasks instead of adding duplicates.

When a token is saved, the TUI imports the next 7 days in the background at startup.

### GitHub issues

Requires the [GitHub CLI](https://cli.github.com/), signed in with `gh auth login`. ytasky uses its token. List search queries in `config.json`, then run `ytasky import-github` (or press `H`). Issues that match the queries become backlog tasks. When the issue of a task still in the backlog is closed, the next import marks the task done. `ytasky import-github --query "is:open assignee:@me"` imports without the config.

### Configuration

`config.json` goes in `%APPDATA%\ytasky` (or in the folder set by `YTASKY_CONFIG_DIR`). Every key is optional:

```json
{
  "gcal_auto_sync": true,
  "gcal_auto_sync_days": 7,
  "github_import_queries": ["is:open assignee:@me"],
  "github_auto_sync": true
}
```

- `gcal_auto_sync`, `github_auto_sync`: import in the background when the TUI starts
- `gcal_auto_sync_days`: how many days ahead the startup import of Google Calendar covers

To sign in with your own Google OAuth client, put its JSON (a Desktop app client from Google Cloud Console) in the same folder as `gcal.json`.

## Known limitations

- The interface is in Japanese: TUI labels, `--help`, messages and category names
- Category icons are [Nerd Font](https://www.nerdfonts.com/) glyphs; other fonts show them as boxes
- `ytasky init` sets the time zone of the data to Asia/Tokyo. Imported Google Calendar times and GitHub milestone dates are converted to that zone. To use another zone, change the `tz:` line of the `_meta` file in the data folder to an IANA name such as `Europe/Berlin`
- On Windows, the Google token is saved as plain text in `gcal_token.json` in the config folder. Any program running as your user can read it
- `ytasky update` updates through Scoop only

## Uninstall

1. Quit the MCP client that runs `ytasky mcp` and close the TUI; Scoop cannot replace or remove a running `ytasky.exe`
2. Run `ytasky gcal-logout` to delete the Google token. To revoke the access too, remove ytasky from the third-party connections of your Google Account
3. Run `scoop uninstall ytasky`, or delete `ytasky.exe`
4. Delete the data folder that `ytasky init` printed and the config folder `%APPDATA%\ytasky`

## License

[MIT License](LICENSE)
