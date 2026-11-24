//! SQLite cache of session metadata plus an FTS5 index of dialog text.
//!
//! Source transcripts are only ever read. FTS rowids encode
//! `(session id << ROWID_SHIFT) | message no`, so a session's rows can be
//! replaced with a cheap rowid range delete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::Transcript;
use crate::parse::{ParseOptions, parse_file};
use crate::paths::{SessionFile, project_label};
use crate::search::fts_query;

const SCHEMA_VERSION: i64 = 1;
const ROWID_SHIFT: u32 = 20;
const MSG_MASK: i64 = (1 << ROWID_SHIFT) - 1;
const PARSE_CHUNK: usize = 32;
const UNTITLED: &str = "(untitled)";

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: i64,
    pub path: PathBuf,
    pub session_id: String,
    pub project: String,
    pub cwd: Option<String>,
    pub title: String,
    pub first_prompt: String,
    /// Unix milliseconds.
    pub started: Option<i64>,
    pub ended: Option<i64>,
    pub messages: i64,
    pub compactions: i64,
    pub size: i64,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub subagents: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub session: i64,
    pub msg_no: usize,
    pub role: String,
    /// Snippet with matches wrapped in `\u{1}` … `\u{2}`.
    pub snippet: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub total: usize,
    pub parsed: usize,
    pub removed: usize,
    pub failed: usize,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    /// Opens (creating if needed) the index. The index holds dialog text from
    /// every session, so a new directory is 0700 and a new database 0600;
    /// SQLite gives its -wal/-shm files the database's permissions.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty() && !d.exists())
        {
            private_dir(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        if !path.exists() {
            private_file(path).with_context(|| format!("create {}", path.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open index {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let index = Self { conn };
        if version != SCHEMA_VERSION {
            index.recreate()?;
        }
        Ok(index)
    }

    /// Drops all cached data; the next [`Index::sync`] rebuilds it.
    pub fn recreate(&self) -> Result<()> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS sessions;
             DROP TABLE IF EXISTS fts;
             CREATE TABLE sessions (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 session_id TEXT NOT NULL,
                 project TEXT NOT NULL,
                 cwd TEXT,
                 title TEXT NOT NULL,
                 first_prompt TEXT NOT NULL,
                 started INTEGER,
                 ended INTEGER,
                 messages INTEGER NOT NULL,
                 compactions INTEGER NOT NULL,
                 size INTEGER NOT NULL,
                 mtime INTEGER NOT NULL,
                 model TEXT,
                 branch TEXT,
                 subagents INTEGER NOT NULL
             );
             CREATE VIRTUAL TABLE fts USING fts5(
                 text, role UNINDEXED, tokenize = 'unicode61 remove_diacritics 2'
             );",
        )?;
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// Brings the index in line with `files`: parses new or changed files,
    /// removes vanished ones. `progress(done, todo)` is called per chunk.
    pub fn sync(
        &mut self,
        files: &[SessionFile],
        home: Option<&Path>,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<SyncStats> {
        let existing = self.fingerprints()?;
        let mut stats = SyncStats {
            total: files.len(),
            ..Default::default()
        };

        let live: HashMap<String, &SessionFile> = files
            .iter()
            .map(|f| (f.path.to_string_lossy().into_owned(), f))
            .collect();
        let stale: Vec<&SessionFile> = files
            .iter()
            .filter(|f| {
                let key = f.path.to_string_lossy();
                existing
                    .get(key.as_ref())
                    .is_none_or(|fp| !fp.same_file_state(&Fingerprint::of(f)))
            })
            .collect();

        let tx = self.conn.transaction()?;
        for (path, fp) in &existing {
            if !live.contains_key(path) {
                delete_session(&tx, fp.id)?;
                stats.removed += 1;
            }
        }
        tx.commit()?;

        let opts = ParseOptions {
            include_sidechain: false,
            keep_tool_output: false,
        };
        for (n, chunk) in stale.chunks(PARSE_CHUNK).enumerate() {
            let parsed: Vec<(&SessionFile, Result<Transcript>)> = chunk
                .par_iter()
                .map(|f| (*f, parse_file(&f.path, opts)))
                .collect();
            let tx = self.conn.transaction()?;
            for (file, result) in parsed {
                match result {
                    Ok(t) => {
                        write_session(&tx, file, &t, home)?;
                        stats.parsed += 1;
                    }
                    Err(_) => stats.failed += 1,
                }
            }
            tx.commit()?;
            progress(((n + 1) * PARSE_CHUNK).min(stale.len()), stale.len());
        }
        Ok(stats)
    }

    fn fingerprints(&self) -> Result<HashMap<String, Fingerprint>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, id, size, mtime, subagents FROM sessions")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Fingerprint {
                    id: r.get(1)?,
                    size: r.get(2)?,
                    mtime: r.get(3)?,
                    subagents: r.get(4)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All sessions, most recently active first.
    pub fn sessions(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM sessions ORDER BY COALESCE(ended, started, mtime) DESC"
        ))?;
        let rows = stmt.query_map([], session_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Looks a session up by full path, exact session id, or unique id prefix.
    pub fn find_session(&self, needle: &str) -> Result<Option<SessionRow>> {
        let sql = format!(
            "SELECT {SESSION_COLS} FROM sessions
             WHERE path = ?1 OR session_id = ?1 OR session_id LIKE ?2 ESCAPE '\\'
             ORDER BY (path = ?1 OR session_id = ?1) DESC LIMIT 2"
        );
        let escaped = needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<SessionRow> = stmt
            .query_map(params![needle, format!("{escaped}%")], session_row)?
            .collect::<rusqlite::Result<_>>()?;
        let exact = rows
            .first()
            .is_some_and(|r| r.session_id == needle || r.path.as_os_str() == needle);
        Ok(match rows.len() {
            1 => rows.into_iter().next(),
            _ if exact => rows.into_iter().next(),
            _ => None,
        })
    }

    /// Full-text search over dialog text, best matches first.
    pub fn search(&self, input: &str, limit: usize) -> Result<Vec<Hit>> {
        let Some(q) = fts_query(input) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid, role, snippet(fts, 0, char(1), char(2), '…', 16)
             FROM fts WHERE fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![q, limit], |r| {
            let rowid: i64 = r.get(0)?;
            Ok(Hit {
                session: rowid >> ROWID_SHIFT,
                msg_no: usize::try_from(rowid & MSG_MASK).unwrap_or(0),
                role: r.get(1)?,
                snippet: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn session_by_id(&self, id: i64) -> Result<Option<SessionRow>> {
        let sql = format!("SELECT {SESSION_COLS} FROM sessions WHERE id = ?1");
        Ok(self.conn.query_row(&sql, [id], session_row).optional()?)
    }
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

fn private_file(path: &Path) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    match opts.open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Copy)]
struct Fingerprint {
    id: i64,
    size: i64,
    mtime: i64,
    subagents: i64,
}

impl Fingerprint {
    /// Equal on-disk state; the row id is ignored.
    fn same_file_state(&self, other: &Fingerprint) -> bool {
        self.size == other.size && self.mtime == other.mtime && self.subagents == other.subagents
    }

    fn of(f: &SessionFile) -> Self {
        Self {
            id: 0,
            size: to_i64(f.size),
            mtime: f.mtime_ms,
            subagents: to_i64(f.subagents.len() as u64),
        }
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

const SESSION_COLS: &str = "id, path, session_id, project, cwd, title, first_prompt, started, ended, \
     messages, compactions, size, model, branch, subagents";

fn session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        path: PathBuf::from(r.get::<_, String>(1)?),
        session_id: r.get(2)?,
        project: r.get(3)?,
        cwd: r.get(4)?,
        title: r.get(5)?,
        first_prompt: r.get(6)?,
        started: r.get(7)?,
        ended: r.get(8)?,
        messages: r.get(9)?,
        compactions: r.get(10)?,
        size: r.get(11)?,
        model: r.get(12)?,
        branch: r.get(13)?,
        subagents: r.get(14)?,
    })
}

fn delete_session(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
    delete_fts(conn, id)
}

fn delete_fts(conn: &Connection, id: i64) -> Result<()> {
    let lo = id << ROWID_SHIFT;
    conn.execute(
        "DELETE FROM fts WHERE rowid BETWEEN ?1 AND ?2",
        params![lo, lo | MSG_MASK],
    )?;
    Ok(())
}

fn write_session(
    conn: &Connection,
    file: &SessionFile,
    t: &Transcript,
    home: Option<&Path>,
) -> Result<()> {
    let m = &t.meta;
    let path = file.path.to_string_lossy().into_owned();
    let stem = file
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let session_id = m.session_id.clone().unwrap_or(stem);
    let project = project_label(m.cwd.as_deref(), &file.project_dir, home);
    let title = m.title.as_deref().unwrap_or(UNTITLED);
    let first_prompt = m.first_prompt.as_deref().unwrap_or("");
    let started = m.started.map(|d| d.timestamp_millis());
    let ended = m.ended.map(|d| d.timestamp_millis());
    let fp = Fingerprint::of(file);
    let id: i64 = conn.query_row(
        "INSERT INTO sessions (path, session_id, project, cwd, title, first_prompt, started,
             ended, messages, compactions, size, mtime, model, branch, subagents)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(path) DO UPDATE SET
             session_id=excluded.session_id, project=excluded.project, cwd=excluded.cwd,
             title=excluded.title, first_prompt=excluded.first_prompt, started=excluded.started,
             ended=excluded.ended, messages=excluded.messages, compactions=excluded.compactions,
             size=excluded.size, mtime=excluded.mtime, model=excluded.model,
             branch=excluded.branch, subagents=excluded.subagents
         RETURNING id",
        params![
            path,
            session_id,
            project,
            m.cwd,
            title,
            first_prompt,
            started,
            ended,
            to_i64(m.messages as u64),
            to_i64(m.compactions as u64),
            fp.size,
            fp.mtime,
            m.model,
            m.branch,
            fp.subagents,
        ],
        |r| r.get(0),
    )?;
    delete_fts(conn, id)?;
    let mut insert =
        conn.prepare_cached("INSERT INTO fts (rowid, text, role) VALUES (?1, ?2, ?3)")?;
    for msg in t.messages() {
        let no = to_i64(msg.no as u64);
        let text = crate::sanitize::sanitize(&msg.dialog_text());
        if no > MSG_MASK || text.trim().is_empty() {
            continue;
        }
        insert.execute(params![(id << ROWID_SHIFT) | no, text, msg.role.as_str()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
