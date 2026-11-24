use std::path::{Path, PathBuf};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::paths::discover;

const FIXTURE: &str = include_str!("../../../tests/fixtures/compacted.jsonl");

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn row(id: i64, project: &str, title: &str, size: i64) -> SessionRow {
    SessionRow {
        id,
        path: PathBuf::from(format!("/x/{id}.jsonl")),
        session_id: format!("sid-{id}"),
        project: project.into(),
        cwd: Some(format!("/home/a/{project}")),
        title: title.into(),
        first_prompt: format!("prompt for {title}"),
        started: Some(1_700_000_000_000 + id),
        ended: Some(1_700_000_100_000 + id),
        messages: id * 10,
        compactions: id % 2,
        size,
        model: Some("claude-opus-5".into()),
        branch: None,
        subagents: 0,
    }
}

fn screen(l: &mut ListScreen) -> String {
    let mut t = Terminal::new(TestBackend::new(110, 20)).unwrap();
    t.draw(|f| l.draw(f, f.area(), "")).unwrap();
    let buf = t.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sample() -> ListScreen {
    ListScreen::new(
        vec![
            row(1, "www/demo", "Fix webhook retry", 100),
            row(2, "home", "Configure statusline", 5000),
            row(3, "www/demo", "Refactor parser", 10),
        ],
        String::new(),
    )
}

fn type_str(l: &mut ListScreen, s: &str) {
    for c in s.chars() {
        l.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn draws_table_and_preview() {
    let mut l = sample();
    let s = screen(&mut l);
    assert!(s.contains("clog ─ 3 sessions"));
    assert!(s.contains("Fix webhook retry"));
    assert!(s.contains("⧉1"));
    assert!(s.contains("> prompt for Fix webhook retry"));
}

#[test]
fn long_titles_are_truncated_inside_their_column() {
    let mut l = ListScreen::new(
        vec![row(1, "www/demo", &"x".repeat(300), 100)],
        String::new(),
    );
    let s = screen(&mut l);
    let line = s.lines().find(|l| l.contains("xxx")).unwrap();
    let after_title = line
        .split('…')
        .nth(1)
        .expect("title truncated with ellipsis");
    assert!(after_title.starts_with(' '), "gap after title: {line:?}");
    assert!(
        after_title.contains("10"),
        "message count still visible: {line:?}"
    );
}

#[test]
fn fuzzy_filter_narrows_and_opens() {
    let mut l = sample();
    l.on_key(key(KeyCode::Char('/')));
    type_str(&mut l, "statusl");
    assert_eq!(l.order.len(), 1);
    match l.on_key(key(KeyCode::Enter)) {
        Action::Open { path, jump, .. } => {
            assert_eq!(path, Path::new("/x/2.jsonl"));
            assert_eq!(jump, None);
        }
        other => panic!("unexpected {other:?}"),
    }
    l.on_key(key(KeyCode::Esc));
    l.on_key(key(KeyCode::Esc));
    assert_eq!(l.order.len(), 3, "Esc outside the input clears the query");
}

#[test]
fn letters_are_commands_outside_the_input() {
    let mut l = sample();
    l.on_key(key(KeyCode::Char('s')));
    assert_eq!(l.order[0], 1, "sorted by size");
    l.on_key(key(KeyCode::Char('j')));
    l.on_key(key(KeyCode::Char('j')));
    l.on_key(key(KeyCode::Char('p')));
    assert_eq!(l.order.len(), 2, "project filter keeps www/demo");
    assert!(screen(&mut l).contains("project: www/demo"));
    assert_eq!(
        l.on_key(key(KeyCode::Char('y'))),
        Action::Copy("cd -- /home/a/www/demo && claude --resume sid-1".into())
    );
    assert_eq!(l.on_key(key(KeyCode::Char('q'))), Action::Quit);
}

#[test]
fn full_text_mode_groups_hits_and_jumps_to_message() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("-home-alice-www-demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("s-1.jsonl"), FIXTURE).unwrap();
    let mut index = Index::open_in_memory().unwrap();
    index
        .sync(&discover(dir.path()).unwrap(), None, |_, _| {})
        .unwrap();

    let mut l = ListScreen::new(index.sessions().unwrap(), String::new());
    l.on_key(key(KeyCode::Tab));
    assert_eq!(l.mode, Mode::FullText);
    type_str(&mut l, "regression");
    l.fts_pending = Some(Instant::now() - FTS_DEBOUNCE * 2);
    l.tick(&index);
    assert_eq!(l.fts_rows.len(), 2, "one session header + one hit");
    let s = screen(&mut l);
    assert!(s.contains("1 hits in 1 sessions"));
    assert!(s.contains("#3"));
    l.on_key(key(KeyCode::Down));
    match l.on_key(key(KeyCode::Enter)) {
        Action::Open { jump, .. } => assert_eq!(jump, Some(3)),
        other => panic!("unexpected {other:?}"),
    }
    l.on_key(key(KeyCode::Up));
    assert!(
        matches!(
            l.on_key(key(KeyCode::Enter)),
            Action::Open { jump: Some(3), .. }
        ),
        "header opens at first hit"
    );
}
