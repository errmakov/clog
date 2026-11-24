use std::fs;
use std::path::Path;

use super::*;
use crate::paths::discover;

const FIXTURE: &str = include_str!("../../tests/fixtures/compacted.jsonl");

fn setup() -> (tempfile::TempDir, Index) {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("projects").join("-home-alice-www-demo");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("s-1.jsonl"), FIXTURE).unwrap();
    (dir, Index::open_in_memory().unwrap())
}

fn sync(dir: &Path, index: &mut Index) -> SyncStats {
    let files = discover(&dir.join("projects")).unwrap();
    index
        .sync(&files, Some(Path::new("/home/alice")), |_, _| {})
        .unwrap()
}

#[test]
fn indexes_session_metadata() {
    let (dir, mut index) = setup();
    let stats = sync(dir.path(), &mut index);
    assert_eq!(
        stats,
        SyncStats {
            total: 1,
            parsed: 1,
            removed: 0,
            failed: 0
        }
    );

    let rows = index.sessions().unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.session_id, "s-1");
    assert_eq!(r.project, "www/demo");
    assert_eq!(r.title, "Fix webhook retry on timeout");
    assert_eq!(r.messages, 4);
    assert_eq!(r.compactions, 1);
}

#[test]
fn second_sync_skips_unchanged_files() {
    let (dir, mut index) = setup();
    sync(dir.path(), &mut index);
    let again = sync(dir.path(), &mut index);
    assert_eq!(again.parsed, 0);
}

#[test]
fn reindexes_changed_and_removes_deleted_files() {
    let (dir, mut index) = setup();
    sync(dir.path(), &mut index);
    let file = dir.path().join("projects/-home-alice-www-demo/s-1.jsonl");
    let extra = r#"{"type":"user","timestamp":"2026-11-21T09:00:00Z","message":{"content":"zebra crossing"}}"#;
    fs::write(&file, format!("{FIXTURE}\n{extra}\n")).unwrap();

    assert_eq!(sync(dir.path(), &mut index).parsed, 1);
    let id = index.sessions().unwrap()[0].id;
    let hits = index.search("zebra", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session, id);
    assert_eq!(
        index
            .search("retry", 50)
            .unwrap()
            .iter()
            .filter(|h| h.msg_no == 1)
            .count(),
        1,
        "old rows replaced, not duplicated"
    );

    fs::remove_file(&file).unwrap();
    assert_eq!(sync(dir.path(), &mut index).removed, 1);
    assert!(index.sessions().unwrap().is_empty());
    assert!(index.search("zebra", 10).unwrap().is_empty());
}

#[test]
fn search_returns_message_numbers_and_highlights() {
    let (dir, mut index) = setup();
    sync(dir.path(), &mut index);
    let hits = index.search("NonRetryable", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].msg_no, 2);
    assert_eq!(hits[0].role, "assistant");
    assert!(hits[0].snippet.contains("\u{1}NonRetryable\u{2}"));
    assert!(
        index
            .search("Look at the classifier", 10)
            .unwrap()
            .is_empty(),
        "thinking is not indexed"
    );
    assert!(
        index.search("FAILED", 10).unwrap().is_empty(),
        "tool output is not indexed"
    );
}

#[test]
fn find_session_by_prefix_or_path() {
    let (dir, mut index) = setup();
    sync(dir.path(), &mut index);
    assert!(index.find_session("s-").unwrap().is_some());
    assert!(index.find_session("nope").unwrap().is_none());
    let path = index.sessions().unwrap()[0].path.clone();
    assert!(
        index
            .find_session(path.to_str().unwrap())
            .unwrap()
            .is_some()
    );
    let id = index.sessions().unwrap()[0].id;
    assert_eq!(index.session_by_id(id).unwrap().unwrap().session_id, "s-1");
}

#[test]
fn reopening_file_index_keeps_data() {
    let (dir, _) = setup();
    let db = dir.path().join("cache/index.db");
    {
        let mut index = Index::open(&db).unwrap();
        sync(dir.path(), &mut index);
    }
    let index = Index::open(&db).unwrap();
    assert_eq!(index.sessions().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn new_index_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache/clog/index.db");
    let _index = Index::open(&db).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(db.parent().unwrap()), 0o700);
    assert_eq!(mode(&db), 0o600);
}

#[test]
fn control_characters_are_not_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("projects").join("p");
    fs::create_dir_all(&proj).unwrap();
    let line = r#"{"type":"user","message":{"content":"mark\u0001er \u001b[31mred"}}"#;
    fs::write(proj.join("s.jsonl"), line).unwrap();
    let mut index = Index::open_in_memory().unwrap();
    sync(dir.path(), &mut index);
    let hits = index.search("red", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].snippet.contains('\u{1b}'));
    assert_eq!(
        index.search("marker", 10).unwrap().len(),
        1,
        "\\x01 stripped, not a separator"
    );
}
