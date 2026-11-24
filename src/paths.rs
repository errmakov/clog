//! Locating session files and naming projects.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

pub const PROJECTS_ENV: &str = "CLOG_PROJECTS_DIR";
pub const DB_ENV: &str = "CLOG_DB";

/// `$CLOG_PROJECTS_DIR`, else `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
pub fn default_projects_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(PROJECTS_ENV) {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(p).join("projects"));
    }
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".claude").join("projects"))
}

/// `$CLOG_DB`, else `<cache dir>/clog/index.db`.
pub fn default_db_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(DB_ENV) {
        return Ok(PathBuf::from(p));
    }
    let cache = dirs::cache_dir().context("cannot determine cache directory")?;
    Ok(cache.join("clog").join("index.db"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    pub path: PathBuf,
    /// Name of the encoded project directory, e.g. `-home-alice-www-demo`.
    pub project_dir: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub subagents: Vec<PathBuf>,
}

/// Finds `<projects>/<project>/<session>.jsonl` files and their subagent
/// transcripts at `<project>/<session>/subagents/*.jsonl`.
pub fn discover(projects_dir: &Path) -> Result<Vec<SessionFile>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(projects_dir)
        .with_context(|| format!("read sessions directory {}", projects_dir.display()))?;
    for project in entries.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let project_dir = project.file_name().to_string_lossy().into_owned();
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let Ok(md) = f.metadata() else { continue };
            if !md.is_file() {
                continue;
            }
            let subagents = jsonl_files(&path.with_extension("").join("subagents"));
            out.push(SessionFile {
                path,
                project_dir: project_dir.clone(),
                size: md.len(),
                mtime_ms: mtime_ms(&md),
                subagents,
            });
        }
    }
    Ok(out)
}

/// True for `<session>/subagents/<agent>.jsonl` transcripts.
pub fn is_subagent_file(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "subagents")
}

pub fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    v.sort();
    v
}

fn mtime_ms(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Human project label: the session's cwd relative to `home`, or the encoded
/// directory name when no cwd was recorded.
pub fn project_label(cwd: Option<&str>, project_dir: &str, home: Option<&Path>) -> String {
    let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
        return project_dir.trim_start_matches('-').to_string();
    };
    let cwd_path = Path::new(cwd);
    match home.and_then(|h| cwd_path.strip_prefix(h).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Some(rel) => rel.to_string_lossy().into_owned(),
        None => cwd.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_label_strips_home() {
        let home = Path::new("/home/alice");
        assert_eq!(
            project_label(Some("/home/alice/www/demo"), "x", Some(home)),
            "www/demo"
        );
        assert_eq!(project_label(Some("/home/alice"), "x", Some(home)), "~");
        assert_eq!(project_label(Some("/srv/app"), "x", Some(home)), "/srv/app");
        assert_eq!(
            project_label(None, "-home-alice-www", Some(home)),
            "home-alice-www"
        );
    }

    #[test]
    fn detects_subagent_files_by_parent_dir_only() {
        assert!(is_subagent_file(Path::new(
            "/p/proj/abc/subagents/agent-1.jsonl"
        )));
        assert!(!is_subagent_file(Path::new("/subagents/proj/abc.jsonl")));
    }

    #[test]
    fn discovers_sessions_and_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-home-alice-demo");
        fs::create_dir_all(proj.join("abc").join("subagents")).unwrap();
        fs::write(proj.join("abc.jsonl"), "{}\n").unwrap();
        fs::write(
            proj.join("abc").join("subagents").join("agent-1.jsonl"),
            "{}\n",
        )
        .unwrap();
        fs::write(proj.join("notes.txt"), "x").unwrap();
        fs::write(dir.path().join("stray.jsonl"), "x").unwrap();

        let found = discover(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project_dir, "-home-alice-demo");
        assert_eq!(found[0].size, 3);
        assert_eq!(found[0].subagents.len(), 1);
    }
}
