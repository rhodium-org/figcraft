//! A host page that confirms saves ([`Services::deliver`]): a Save counts once the host answers
//! that it stored the file, a refusal leaves the document unsaved and says why, and an edit made
//! while a save was on its way stays unsaved. A host with places of its own takes File ▸ Open.

use std::cell::RefCell;
use std::rc::Rc;

use figcraft_engine::Session;
use serde_json::json;

use crate::{Delivery, FigApp, Services};

#[derive(Default)]
struct Shared {
    /// Every file handed over: (ticket, name).
    handed: Vec<(u64, String)>,
    /// The document each was handed over for.
    docs: Vec<u64>,
    /// Answers the host has given and the app has not yet taken.
    answers: Vec<(u64, Result<(), String>)>,
    /// Open requests the host took.
    opens: usize,
    /// Whether the host offers places to open from.
    places: bool,
}

/// A page host that numbers every save and answers when the test says.
struct ConfirmingHost(Rc<RefCell<Shared>>);

impl Services for ConfirmingHost {
    fn downloads(&self) -> bool {
        true
    }
    fn download(&mut self, _name: &str, _bytes: &[u8]) -> bool {
        panic!("a confirming host is handed document files through deliver");
    }
    fn deliver(&mut self, doc: u64, name: &str, _bytes: &[u8]) -> Delivery {
        let mut s = self.0.borrow_mut();
        s.docs.push(doc);
        let ticket = s.handed.len() as u64 + 1;
        s.handed.push((ticket, name.to_string()));
        Delivery::Waiting(ticket)
    }
    fn settled_saves(&mut self) -> Vec<(u64, Result<(), String>)> {
        std::mem::take(&mut self.0.borrow_mut().answers)
    }
    fn host_file(&self, _doc: u64) -> Option<String> {
        Some("design.figcraft".into())
    }
    fn host_open(&mut self) -> bool {
        let mut s = self.0.borrow_mut();
        if s.places {
            s.opens += 1;
        }
        s.places
    }
}

fn app() -> (FigApp, Rc<RefCell<Shared>>) {
    let shared = Rc::new(RefCell::new(Shared::default()));
    let mut app = FigApp::new(Session::empty(), Box::new(ConfirmingHost(shared.clone())));
    app.session.execute("file.new", &json!({"name": "design"})).unwrap();
    edit(&mut app, 0.0);
    (app, shared)
}

fn edit(app: &mut FigApp, x: f64) {
    app.session.execute("shape.rectangle", &json!({"x": x, "y": 0, "width": 10, "height": 10})).unwrap();
}

fn answer(app: &mut FigApp, shared: &Rc<RefCell<Shared>>, ticket: u64, outcome: Result<(), String>) {
    shared.borrow_mut().answers.push((ticket, outcome));
    app.host_saves_tick();
}

fn dirty(app: &FigApp) -> bool {
    app.session.ds().unwrap().is_dirty()
}

#[test]
fn a_save_counts_once_the_host_answers_that_it_is_stored() {
    let (mut app, shared) = app();
    app.save();
    assert_eq!(shared.borrow().handed, [(1, "design.figcraft".to_string())]);
    assert_eq!(shared.borrow().docs, [app.session.ds().unwrap().uid], "the save names its document");
    assert!(dirty(&app), "waiting for the host's answer");
    answer(&mut app, &shared, 1, Ok(()));
    assert!(!dirty(&app));
}

#[test]
fn a_refused_save_leaves_the_document_unsaved_and_says_why() {
    let (mut app, shared) = app();
    app.save();
    answer(&mut app, &shared, 1, Err("GitHub holds a newer version, so nothing was written.".into()));
    assert!(dirty(&app));
    assert!(app.ui.toast.as_ref().is_some_and(|t| t.0.contains("newer version")), "the host's reason is shown");
    app.save();
    answer(&mut app, &shared, 2, Ok(()));
    assert!(!dirty(&app), "a save that lands after one that didn't marks it saved");
}

#[test]
fn an_edit_made_while_a_save_is_on_its_way_stays_unsaved() {
    let (mut app, shared) = app();
    app.save();
    edit(&mut app, 50.0);
    answer(&mut app, &shared, 1, Ok(()));
    assert!(dirty(&app), "the edit was not in the save");
}

#[test]
fn an_answer_for_no_save_changes_nothing() {
    let (mut app, shared) = app();
    answer(&mut app, &shared, 7, Ok(()));
    assert!(dirty(&app));
}

#[test]
fn a_host_with_places_takes_file_open() {
    let (mut app, shared) = app();
    app.open_dialog();
    assert_eq!(shared.borrow().opens, 0, "no places declared: the app's own picker");
    assert!(app.pending_dialog.is_some(), "the platform's picker was started");
    app.pending_dialog = None;
    shared.borrow_mut().places = true;
    app.open_dialog();
    assert_eq!(shared.borrow().opens, 1);
    assert!(app.pending_dialog.is_none(), "no file picker was started");
}
