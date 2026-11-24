# clog

A terminal UI for browsing and searching your local Claude Code session
transcripts — including the part of a session that was compacted away and is
no longer visible in the terminal.

```
clog                     open the session list
clog retry webhook       open with the filter pre-filled
clog index               update the index and print stats
clog export <id|path>    write a session as Markdown (stdout or -o FILE)
clog --reindex           rebuild the index from scratch
```

## How it works

- Reads `~/.claude/projects/<project>/<session>.jsonl` (and subagent
  transcripts in `<session>/subagents/`). Source files are **never written**.
- Keeps a cache at `~/.cache/clog/index.db` (SQLite + FTS5): session metadata
  and dialog text only — no tool output, no thinking. Startup re-parses only
  files whose size, mtime or subagent count changed.
- Compaction boundaries (`compact_boundary`) are shown as banners; the summary
  Claude received afterwards is collapsible beneath each banner.

Overrides: `--projects-dir` / `CLOG_PROJECTS_DIR` (also honours
`CLAUDE_CONFIG_DIR`), `--db` / `CLOG_DB`, `PAGER` (default `less`).

## Keys

**Session list**

| Key | Action |
| --- | --- |
| `j`/`k`, `↑`/`↓` | move |
| `Enter` | open session (in full-text mode: at the matching message) |
| `/` | edit the filter (`Esc` leaves the input, `Ctrl-u` clears) |
| `Tab` | switch fuzzy filter ↔ full-text search |
| `s` | cycle sort: recent / size / messages |
| `p` | toggle filter to the selected session's project |
| `y` | copy `cd <cwd> && claude --resume <id>` (OSC 52) |
| `q`, `Ctrl-c` | quit |

Full-text terms are ANDed and prefix-matched; `"quoted words"` match as a phrase.

**Viewer**

| Key | Action |
| --- | --- |
| `j`/`k`, `Space`/`b`, `Ctrl-d`/`Ctrl-u`, `g`/`G` | scroll |
| `]` / `[` | next / previous prompt of yours |
| `}` / `{` | next / previous compaction |
| `Enter` | expand / collapse the tool call or summary under the cursor |
| `t` | expand / collapse all tool calls |
| `T` | show / hide thinking |
| `/`, `n`, `N` | search within the session (visible text) |
| `\|` | toggle the timeline |
| `e` | open the whole session as Markdown in `$PAGER` |
| `s` | pick a subagent transcript |
| `y` | copy the resume command |
| `q`, `Esc` | back |

## Build

```
cargo build --release
cp target/release/clog ~/.local/bin/
```

Copying uses OSC 52, which most modern terminals (and tmux with
`set -g set-clipboard on`) support.
