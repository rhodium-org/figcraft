//! A save handed to a host page as a download (`file.saveAs {data: true}`) counts once the host
//! confirms it stored the bytes ([`FileSnapshot::delivered`]): as of the snapshot taken when it
//! was handed over, so edits made meanwhile stay unsaved.

use serde_json::json;

use crate::Session;
use crate::cmd::file::FileSnapshot;

fn rect(s: &mut Session, x: f64) {
    s.execute("shape.rectangle", &json!({"x": x, "y": 0, "width": 10, "height": 10})).unwrap();
}

/// What the browser shell does on Save: the bytes as a download, and the snapshot they were made from.
fn hand_over(s: &mut Session) -> FileSnapshot {
    let snapshot = FileSnapshot::of(s.ds().unwrap());
    let r = s.execute("file.saveAs", &json!({"data": true})).unwrap();
    assert!(r["data"].as_str().is_some_and(|d| !d.is_empty()));
    snapshot
}

#[test]
fn a_handed_over_save_counts_only_once_the_host_confirms_it() {
    let mut s = Session::new();
    rect(&mut s, 0.0);
    let save = hand_over(&mut s);
    assert!(s.ds().unwrap().is_dirty(), "handed over, not yet stored");
    assert!(save.delivered(&mut s));
    assert!(!s.ds().unwrap().is_dirty());
}

#[test]
fn an_edit_made_while_the_save_was_on_its_way_stays_unsaved() {
    let mut s = Session::new();
    rect(&mut s, 0.0);
    let save = hand_over(&mut s);
    rect(&mut s, 50.0);
    assert!(save.delivered(&mut s));
    assert!(s.ds().unwrap().is_dirty(), "the later edit was not in the save");
}

#[test]
fn a_refused_save_then_a_stored_one_marks_the_file_saved() {
    let mut s = Session::new();
    rect(&mut s, 0.0);
    let refused = hand_over(&mut s);
    drop(refused);
    assert!(s.ds().unwrap().is_dirty(), "a refused save changes nothing");
    let stored = hand_over(&mut s);
    assert!(stored.delivered(&mut s));
    assert!(!s.ds().unwrap().is_dirty());
}

#[test]
fn two_saves_on_their_way_both_count_in_turn() {
    let mut s = Session::new();
    rect(&mut s, 0.0);
    let first = hand_over(&mut s);
    rect(&mut s, 50.0);
    let second = hand_over(&mut s);
    assert!(first.delivered(&mut s));
    assert!(s.ds().unwrap().is_dirty(), "the second save's edit is not stored yet");
    assert!(second.delivered(&mut s), "the first save landing doesn't make the second stale");
    assert!(!s.ds().unwrap().is_dirty());
}

#[test]
fn a_confirmation_for_a_closed_file_changes_nothing() {
    let mut s = Session::new();
    rect(&mut s, 0.0);
    let save = hand_over(&mut s);
    s.execute("file.close", &json!({"force": true})).unwrap();
    assert!(!save.delivered(&mut s));
}
