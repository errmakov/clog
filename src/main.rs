use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use clog::export::{ExportOptions, to_markdown};
use clog::index::Index;
use clog::parse::{ParseOptions, parse_file};
use clog::paths::{default_db_path, default_projects_dir, discover};

/// Browse and search local Claude Code session transcripts.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Pre-fill the session filter.
    query: Option<String>,
    /// Session transcripts directory [default: ~/.claude/projects, env CLOG_PROJECTS_DIR].
    #[arg(long, global = true)]
    projects_dir: Option<PathBuf>,
    /// Index database path [default: ~/.cache/clog/index.db, env CLOG_DB].
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Drop the index and rebuild it from scratch before starting.
    #[arg(long)]
    reindex: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Update the index and print statistics, without opening the TUI.
    Index,
    /// Write a session as Markdown.
    Export {
        /// Session id, unique id prefix, or path to a .jsonl transcript.
        session: String,
        /// Output file [default: stdout].
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Include thinking blocks.
        #[arg(long)]
        thinking: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let projects = match &cli.projects_dir {
        Some(p) => p.clone(),
        None => default_projects_dir()?,
    };
    let db = match &cli.db {
        Some(p) => p.clone(),
        None => default_db_path()?,
    };
    let mut index = Index::open(&db)?;
    if cli.reindex {
        index.recreate()?;
    }
    match cli.command {
        Some(Command::Index) => {
            sync(&mut index, &projects, true)?;
            Ok(())
        }
        Some(Command::Export {
            session,
            output,
            thinking,
        }) => export(&index, &session, output, thinking),
        None => {
            sync(&mut index, &projects, false)?;
            clog::tui::run(index, cli.query.unwrap_or_default())
        }
    }
}

fn sync(index: &mut Index, projects: &std::path::Path, verbose: bool) -> Result<()> {
    let started = Instant::now();
    let files = discover(projects)?;
    let home = dirs::home_dir();
    let mut stderr = std::io::stderr();
    let stats = index.sync(&files, home.as_deref(), |done, todo| {
        let _ = write!(stderr, "\rclog: indexing {done}/{todo} changed sessions…");
        let _ = stderr.flush();
    })?;
    if stats.parsed > 0 {
        eprintln!();
    }
    if verbose || stats.failed > 0 {
        eprintln!(
            "clog: {} sessions, {} (re)indexed, {} removed, {} unreadable · {:.1}s",
            stats.total,
            stats.parsed,
            stats.removed,
            stats.failed,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn export(index: &Index, session: &str, output: Option<PathBuf>, thinking: bool) -> Result<()> {
    let path = match index.find_session(session)? {
        Some(row) => row.path,
        None if std::path::Path::new(session).is_file() => PathBuf::from(session),
        None => bail!("no unique session matches {session:?} (run `clog index` first?)"),
    };
    let is_subagent = clog::paths::is_subagent_file(&path);
    let t = parse_file(
        &path,
        ParseOptions {
            include_sidechain: is_subagent,
            keep_tool_output: true,
        },
    )?;
    let md = to_markdown(
        &t,
        ExportOptions {
            include_thinking: thinking,
        },
    );
    match output {
        Some(out) => std::fs::write(&out, md).with_context(|| format!("write {}", out.display())),
        None => std::io::stdout()
            .write_all(md.as_bytes())
            .context("write stdout"),
    }
}
