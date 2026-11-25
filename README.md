# clog

[![CI](https://github.com/errmakov/clog/actions/workflows/ci.yml/badge.svg)](https://github.com/errmakov/clog/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A terminal UI for browsing and searching your local Claude Code session
transcripts — including the part of a session that was compacted away and is
no longer visible in the terminal.

> clog is an independent community tool. It is not affiliated with or
> endorsed by Anthropic. It only reads files Claude Code already stores on
> your machine and sends nothing anywhere.

## Install

**macOS / Linux**

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/errmakov/clog/releases/latest/download/clog-installer.sh | sh
```

**Windows (PowerShell)**

```
powershell -ExecutionPolicy Bypass -c "irm https://github.com/errmakov/clog/releases/latest/download/clog-installer.ps1 | iex"
```

**Prebuilt archives** for Linux (x86_64, arm64), macOS (Apple Silicon, Intel)
and Windows (x86_64) are on the
[Releases page](https://github.com/errmakov/clog/releases), with SHA-256
checksums. The binaries are not code-signed: on macOS, a binary downloaded
through a browser needs `xattr -d com.apple.quarantine ./clog` (the install
script avoids this); on Windows, SmartScreen may ask for confirmation.

**From source** (Rust 1.88+):

```
cargo install --git https://github.com/errmakov/clog --locked
```

**Managed Windows (WSL)**

On company-managed Windows, an Application Control (WDAC) policy may refuse
to run `clog.exe` ("An Application Control policy has blocked this file").
Building from source doesn't help, because a locally built binary is
unsigned too. If WSL is available, install the Linux build inside it with
the macOS / Linux command above. Then point it at your Windows sessions,
replacing `<you>` with your Windows user name:

```
echo 'export CLOG_PROJECTS_DIR=/mnt/c/Users/<you>/.claude/projects' >> ~/.bashrc
```

If you run Claude Code inside WSL itself, skip this; plain `clog` finds
`~/.claude/projects`. Reading Windows files from WSL is slower, so the first
indexing run takes longer. The resume command that `y` copies is in bash
syntax but holds the session's Windows path (`C:\…`), so adjust it before
running it.

## Usage

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

Copying uses OSC 52, which most modern terminals (and tmux with
`set -g set-clipboard on`) support.

## Platforms

Linux, macOS and Windows (64-bit) are built and tested in CI.

| | Sessions | Index | `y` copies | Default pager |
| --- | --- | --- | --- | --- |
| Linux | `~/.claude/projects` | `~/.cache/clog/index.db` | bash/zsh command | `less` |
| macOS | `~/.claude/projects` | `~/Library/Caches/clog/index.db` | bash/zsh command | `less` |
| Windows | `%USERPROFILE%\.claude\projects` | `%LOCALAPPDATA%\clog\index.db` | PowerShell command | `more` |

On Windows, copying needs a terminal with OSC 52 support (Windows Terminal);
the index file relies on your profile folder's default permissions rather
than Unix 0600.

## Development

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Releases: bump `version` in `Cargo.toml`, commit, then push a tag
`vX.Y.Z`; the Release workflow builds the archives and installers and
publishes the GitHub Release.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
