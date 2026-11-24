use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;

const FIXTURE: &str = include_str!("../../../tests/fixtures/compacted.jsonl");

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

#[test]
fn resize_keeps_cursor_inside_the_same_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.jsonl");
    let long = "word ".repeat(400);
    let jsonl = format!(
        "{{\"type\":\"user\",\"message\":{{\"content\":\"hi\"}}}}\n\
         {{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}\n"
    );
    std::fs::write(&path, jsonl).unwrap();
    let mut v = Viewer::open(path, "p".into(), None).unwrap();
    let mut wide = Terminal::new(TestBackend::new(90, 30)).unwrap();
    screen(&mut v, &mut wide);
    let first = v.first_line_of(2).unwrap();
    v.jump(first + 10);
    let mut narrow = Terminal::new(TestBackend::new(60, 30)).unwrap();
    screen(&mut v, &mut narrow);
    assert_eq!(v.lines[v.cursor].msg_no, Some(2));
    assert_eq!(
        v.cursor,
        v.first_line_of(2).unwrap() + 10,
        "offset within message kept"
    );
}

#[test]
fn search_ignores_ctrl_keys_and_folds_unicode_case() {
    let (_d, mut v, mut t) = setup();
    screen(&mut v, &mut t);
    v.on_key(key(KeyCode::Char('/')));
    v.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert_eq!(v.search.as_ref().unwrap().query, "");
    for c in "NONRETRYABLE".chars() {
        v.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(v.search.as_ref().unwrap().matches.len(), 1);
}

fn setup() -> (tempfile::TempDir, Viewer, Terminal<TestBackend>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s-1.jsonl");
    std::fs::write(&path, FIXTURE).unwrap();
    let sub = dir.path().join("s-1").join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("agent-a.jsonl"),
        r#"{"type":"user","isSidechain":true,"message":{"content":"Explore the retry code"}}"#,
    )
    .unwrap();
    let viewer = Viewer::open(path, "www/demo".into(), None).unwrap();
    (
        dir,
        viewer,
        Terminal::new(TestBackend::new(90, 30)).unwrap(),
    )
}

fn screen(v: &mut Viewer, t: &mut Terminal<TestBackend>) -> String {
    t.draw(|f| v.draw(f, f.area())).unwrap();
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

#[test]
fn draws_header_dialog_banner_and_footer() {
    let (_d, mut v, mut t) = setup();
    let s = screen(&mut v, &mut t);
    assert!(s.contains("www/demo · Fix webhook retry on timeout"));
    assert!(s.contains("4 msgs · 1 compaction · 1 subagent"));
    assert!(s.contains("┃ you"));
    assert!(s.contains("▸ Bash cargo test retry"));
    assert!(s.contains("⧉ COMPACTED"));
    assert!(s.contains("]/[ prompt"));
}

#[test]
fn enter_on_tool_expands_output() {
    let (_d, mut v, mut t) = setup();
    screen(&mut v, &mut t);
    let tool_line = v
        .lines
        .iter()
        .position(|l| l.text().starts_with("▸ Bash"))
        .unwrap();
    v.jump(tool_line);
    v.on_key(key(KeyCode::Enter));
    let s = screen(&mut v, &mut t);
    assert!(s.contains("test retry ... FAILED"));
    assert!(
        v.lines[v.cursor].text().starts_with("▾ Bash"),
        "cursor stays on fold header"
    );
    v.on_key(key(KeyCode::Char('t')));
    assert!(
        screen(&mut v, &mut t).contains("FAILED"),
        "t expands all tools"
    );
    v.on_key(key(KeyCode::Char('t')));
    assert!(
        !screen(&mut v, &mut t).contains("FAILED"),
        "second t collapses all, resetting toggles"
    );
}

#[test]
fn jumps_between_prompts_and_compactions() {
    let (_d, mut v, mut t) = setup();
    screen(&mut v, &mut t);
    v.on_key(key(KeyCode::Char(']')));
    assert_eq!(v.lines[v.cursor].kind, Kind::Prompt);
    assert_eq!(
        v.lines[v.cursor].msg_no,
        Some(3),
        "first ] skips the prompt under the cursor"
    );
    v.on_key(key(KeyCode::Char('{')));
    assert_eq!(v.lines[v.cursor].kind, Kind::Boundary);
    v.on_key(key(KeyCode::Char('[')));
    assert_eq!(v.lines[v.cursor].msg_no, Some(1));
    v.on_key(key(KeyCode::Char('G')));
    assert_eq!(v.cursor, v.lines.len() - 1);
    v.on_key(key(KeyCode::Char('g')));
    assert_eq!(v.cursor, 0);
}

#[test]
fn search_finds_and_cycles_matches() {
    let (_d, mut v, mut t) = setup();
    screen(&mut v, &mut t);
    v.on_key(key(KeyCode::Char('/')));
    for c in "retry".chars() {
        v.on_key(key(KeyCode::Char(c)));
    }
    v.on_key(key(KeyCode::Enter));
    let first = v.cursor;
    assert!(v.lines[first].text().to_lowercase().contains("retry"));
    v.on_key(key(KeyCode::Char('n')));
    assert!(v.cursor > first);
    v.on_key(key(KeyCode::Char('N')));
    assert_eq!(v.cursor, first);
    let s = screen(&mut v, &mut t);
    assert!(s.contains("/retry  match"));
    v.on_key(key(KeyCode::Esc));
    assert!(v.search.is_none());
    assert_eq!(v.on_key(key(KeyCode::Esc)), Action::Back);
}

#[test]
fn opens_at_requested_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s-1.jsonl");
    std::fs::write(&path, FIXTURE).unwrap();
    let mut v = Viewer::open(path, "p".into(), Some(3)).unwrap();
    let mut t = Terminal::new(TestBackend::new(90, 30)).unwrap();
    screen(&mut v, &mut t);
    assert_eq!(v.lines[v.cursor].msg_no, Some(3));
}

#[test]
fn copy_resume_and_subagent_picker() {
    let (_d, mut v, mut t) = setup();
    screen(&mut v, &mut t);
    assert_eq!(
        v.on_key(key(KeyCode::Char('y'))),
        Action::Copy("cd -- /home/alice/www/demo && claude --resume s-1".into())
    );
    assert_eq!(v.on_key(key(KeyCode::Char('s'))), Action::None);
    let s = screen(&mut v, &mut t);
    assert!(s.contains("Explore the retry code"));
    match v.on_key(key(KeyCode::Enter)) {
        Action::OpenSubagent(p, project) => {
            assert!(p.ends_with("agent-a.jsonl"));
            assert_eq!(project, "www/demo");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn subagent_viewer_includes_sidechain_and_cannot_resume() {
    let (d, _, mut t) = setup();
    let mut v = Viewer::open(
        d.path().join("s-1/subagents/agent-a.jsonl"),
        "p".into(),
        None,
    )
    .unwrap();
    let s = screen(&mut v, &mut t);
    assert!(s.contains("Explore the retry code"));
    assert!(matches!(
        v.on_key(key(KeyCode::Char('y'))),
        Action::Status(_)
    ));
    assert!(matches!(
        v.on_key(key(KeyCode::Char('s'))),
        Action::Status(_)
    ));
}
