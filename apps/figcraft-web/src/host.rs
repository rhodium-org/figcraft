//! Host modes: a page that runs FigCraft (as an `<iframe>`, or in its own document) opens
//! documents in it and stores what it saves instead of the browser downloading. The messages
//! and the two transports are `craft-web-host`'s (storytold/craft-libs): the same contract as
//! the other Craft apps' bridges. This module is FigCraft's part of it.
//!
//! - `?host=parent` (an `<iframe>`, such as a Nextcloud app) talks to the same-origin parent over
//!   `postMessage`; `data-host="page"` on the canvas (the page's own document) uses DOM
//!   `CustomEvent`s on the canvas instead. `?host=parent` wins when both are present.
//! - `figcraft:ready` (to the host) once the app is running and listening; send a file now.
//! - `figcraft:open` (from the host): `{ name, bytes }`. It opens exactly as a file picked with
//!   File › Open or dropped on the window would (the app's own open queue, with its "Opening as
//!   a new file" card). A payload over the browser's own open limit
//!   ([`figcraft_ui_egui::browser_can_read`]'s 512 MB) is refused before its bytes are copied,
//!   and anything malformed is ignored. FigCraft reads whole files, so it says `file: false` in
//!   `ready` and a host sends `bytes`.
//! - `figcraft:save` (to the host): `{ name, saveAs, bytes }` for every file the app hands its
//!   download service; in host mode nothing reaches the browser's downloads. `saveAs` says whether
//!   the file becomes the document's file ([`crate::hostmode::save_as`]):
//!   - A Save of the file the host opened, under its own name: `false`.
//!   - A document file (`.figcraft` or `.json`, the formats `file.save` and `file.saveAs` write)
//!     under a name the host didn't open — Save As, or a Save of a document the host never sent
//!     (an untitled one): `true`. It becomes the document's file; the next Save sends its name
//!     with `false`. Names compare without case.
//!   - Everything else is a copy or export that never becomes the document: `false` — rendered
//!     images, SVG and PDF, `.fig` copies, and the variables exports (a `.variables.json`
//!     sidecar's `.json` extension doesn't make it a document).
//! - `figcraft:dirty` whenever unsaved changes appear or go away, `figcraft:failed` when the app
//!   couldn't start.
//!
//! - Beyond the crate's contract ([`crate::host_ext`], rhodium-org fork): `figcraft:places` with
//!   any place makes File ▸ Open (Ctrl+O, Open on the home screen) send `figcraft:open-request`
//!   instead of opening the browser's picker, and the host sends the file; after
//!   `figcraft:confirm-saves {value: true}` each document Save carries an `id` and counts once the
//!   host answers `figcraft:saved {id}`, while `figcraft:save-failed {id, message}` leaves the
//!   document unsaved and shows why. Copies and exports are never waited for.
//!   `figcraft:close-all` closes every open document, unsaved changes and all, before the next
//!   `figcraft:open`: a host that keeps one document at a time sends it once the person agreed to
//!   lose what was not saved, so the app's unsaved state is the new document's alone.
//! - Tabs ([`crate::host_ext`]): every `figcraft:open` counts (1 for the host's first) and,
//!   once it has opened, `figcraft:opened {open, doc, name}` says which document it became
//!   (`doc: null` when it didn't open). `figcraft:documents {front, docs: [{doc, name, dirty}]}`
//!   follows the tabs, their names, their unsaved state and the tab in front, and every
//!   `figcraft:save` carries `doc`. The file the host opened, which decides `saveAs`, is kept
//!   per document, so two documents of one name never save as each other.
//!
//! In host mode the app starts with no document (the home screen) and the host sends the file to
//! edit; `?author=` names the user for comments and board notes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use craft_web_host::web::{FileData, Host, Message};
use figcraft_ui_egui::{Delivery, FigApp};

use crate::host_ext::Ext;
use crate::hostmode::DocEntry;

/// The app id in the host messages (`figcraft:ready`).
const APP: &str = "figcraft";

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

/// Files the host sent, on their way to the app's own open queue ([`figcraft_ui_egui::pending_open`]):
/// `(the host's number for the open, name, bytes)`.
pub type OpenQueue = Rc<RefCell<Vec<(u64, String, Vec<u8>)>>>;

/// Saves a host confirms ([`crate::host_ext`]): numbered once the host asks, and answered later.
#[derive(Default)]
struct Confirms {
    /// The host asked for numbered saves (`confirm-saves {value: true}`).
    on: bool,
    /// The last ticket given.
    last: u64,
    /// Saves handed over and not yet answered: the ticket, the document, and the name the
    /// document's file becomes once it is stored (a Save As), if any.
    waiting: Vec<(u64, u64, Option<String>)>,
    /// Answers heard and not yet taken by the app.
    settled: Vec<(u64, Result<(), String>)>,
}

#[derive(Clone)]
pub struct Bridge {
    host: Rc<Host>,
    ext: Ext,
    /// The host declared places of its own: it opens files ([`crate::host_ext`]).
    places: Rc<Cell<bool>>,
    confirms: Rc<RefCell<Confirms>>,
    /// The host asked to close every open document (`close-all`), not yet done.
    close_all: Rc<Cell<bool>>,
    /// Per document (its tab's uid), the file the host opened, and after a `saveAs` hand-over
    /// the document's file: the name a Save under (`saveAs: false`) is decided against.
    host_files: HostFiles,
    /// The host's opens counted so far, and the names of those not yet opened, by number.
    opens: Rc<RefCell<Opens>>,
    /// What the host last heard in `documents`: the tab in front and the open documents.
    told: Rc<RefCell<Option<Told>>>,
    /// The host writes its files back in place (`autosave {available}`), so AutoSave may run.
    auto_saves: Rc<Cell<bool>>,
    /// The documents the host said where it keeps (`document-place`), newest first: the home
    /// screen's Recent files, kept in the browser's storage between visits.
    recents: Rc<RefCell<Vec<figcraft_ui_egui::HostRecent>>>,
}

/// The browser storage key the host's Recent files are kept under.
const RECENTS_KEY: &str = "figcraft.hostRecents";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

type HostFiles = Rc<RefCell<HashMap<u64, String>>>;

/// The tab in front and the open documents, as `documents` last told them.
type Told = (Option<u64>, Vec<DocEntry>);

#[derive(Default)]
struct Opens {
    last: u64,
    names: HashMap<u64, String>,
}

impl Bridge {
    /// The bridge for this page, or `None` when FigCraft isn't hosted: no `?host=parent` and no
    /// `data-host="page"` on the canvas, an unframed `?host=parent` page (it stays the normal app
    /// with downloads), or a parent on another origin.
    pub fn connect(canvas: &web_sys::HtmlCanvasElement) -> Option<Self> {
        let host = Host::connect(APP, canvas)?;
        let ext = Ext::new(APP, host.mode(), canvas)?;
        Some(Self {
            host: Rc::new(host),
            ext,
            places: Rc::default(),
            confirms: Rc::default(),
            close_all: Rc::default(),
            host_files: Rc::default(),
            opens: Rc::default(),
            told: Rc::default(),
            auto_saves: Rc::default(),
            recents: Rc::new(RefCell::new(
                storage().and_then(|s| s.get_item(RECENTS_KEY).ok().flatten()).map(|t| crate::hostmode::recents_from_json(&t)).unwrap_or_default(),
            )),
        })
    }

    /// Hear the files the host sends, and queue each one on `opens` for the app's own open path
    /// (the shell drains it into [`figcraft_ui_egui::pending_open`] every frame, so a host file
    /// opens like a dropped one).
    pub fn listen(&self, opens: OpenQueue, ctx: egui::Context) {
        let (host_files, confirms, counted, ext, repaint) =
            (self.host_files.clone(), self.confirms.clone(), self.opens.clone(), self.ext.clone(), ctx.clone());
        self.host.listen(move |message| match message {
            Message::Open { name, data } => deliver_open(&opens, &counted, &ext, &ctx, &name, &data),
            Message::Saved { id } => {
                if let Some(ticket) = crate::hostmode::ticket(&serde_json::Value::String(id)) {
                    settle(&confirms, &host_files, ticket, Ok(()));
                    ctx.request_repaint();
                }
            }
            _ => {}
        });
        // The extension messages (crate::host_ext).
        let (places, confirms, host_files, close_all) = (self.places.clone(), self.confirms.clone(), self.host_files.clone(), self.close_all.clone());
        let (auto_saves, recents) = (self.auto_saves.clone(), self.recents.clone());
        self.ext.listen(move |kind, message| {
            match kind {
                "autosave" => auto_saves.set(crate::hostmode::flag(&message, "available")),
                "document-place" => {
                    let fallback = message["doc"].as_u64().and_then(|d| host_files.borrow().get(&d).cloned());
                    if let Some((_, recent)) = crate::hostmode::document_place(&message, fallback.as_deref()) {
                        let mut list = recents.borrow_mut();
                        crate::hostmode::remember(&mut list, recent);
                        if let Some(s) = storage() {
                            let _ = s.set_item(RECENTS_KEY, &crate::hostmode::recents_to_json(&list));
                        }
                    }
                }
                "places" => places.set(crate::hostmode::offers_places(&message)),
                "confirm-saves" => confirms.borrow_mut().on = crate::hostmode::flag(&message, "value"),
                "close-all" => close_all.set(true),
                "save-failed" => {
                    if let Some((ticket, why)) = crate::hostmode::save_failed(&message) {
                        settle(&confirms, &host_files, ticket, Err(why));
                    }
                }
                _ => return,
            }
            repaint.request_repaint();
        });
    }

    /// Hand a file of document `doc` the app saves to the host. Numbered, and answered later,
    /// once the host asked to confirm saves; otherwise done when handed over, as before.
    pub fn deliver(&self, doc: u64, path: &str, bytes: &[u8], auto: bool) -> Delivery {
        let name = file_name(path);
        let host_file = self.host_file(doc);
        let save_as = crate::hostmode::save_as(&name, host_file.as_deref());
        let mut confirms = self.confirms.borrow_mut();
        if !confirms.on {
            drop(confirms);
            return if self.post_save_of(doc, path, bytes, auto) { Delivery::Delivered } else { Delivery::Refused };
        }
        let ticket = confirms.last.wrapping_add(1);
        if self.ext.post_save(&name, bytes, Some(&ticket.to_string()), save_as, Some(doc), auto).is_err() {
            return Delivery::Refused;
        }
        confirms.last = ticket;
        // After a Save As is stored the host has the new file: the next Save of it is no Save As.
        confirms.waiting.push((ticket, doc, save_as.then_some(name)));
        Delivery::Waiting(ticket)
    }

    /// The host's open number `open` became document `doc` (`None`: it didn't open): the host
    /// hears it, and the file it sent is that document's.
    pub fn opened(&self, open: u64, doc: Option<u64>) {
        let name = self.opens.borrow_mut().names.remove(&open).unwrap_or_default();
        if let Some(doc) = doc {
            self.host_files.borrow_mut().insert(doc, name.clone());
        }
        let _ = self.ext.post_json("opened", &crate::hostmode::opened_fields(open, doc, &name));
    }

    /// The open documents and the tab in front, every frame: the host hears `documents` when
    /// they changed, and forgets the files of documents no longer open.
    pub fn set_documents(&self, front: Option<u64>, docs: Vec<DocEntry>) {
        let now = (front, docs);
        if self.told.borrow().as_ref() == Some(&now) {
            return;
        }
        self.host_files.borrow_mut().retain(|doc, _| now.1.iter().any(|d| d.doc == *doc));
        if self.ext.post_json("documents", &crate::hostmode::documents_fields(now.0, &now.1)).is_ok() {
            *self.told.borrow_mut() = Some(now);
        }
    }

    /// The host writes its files back in place, so AutoSave may run for them.
    pub fn auto_saves(&self) -> bool {
        self.auto_saves.get()
    }

    /// The host's documents for the home screen's Recent files.
    pub fn recents(&self) -> Vec<figcraft_ui_egui::HostRecent> {
        self.recents.borrow().clone()
    }

    /// Ask the host to open `recent` again from its place.
    pub fn open_recent(&self, recent: &figcraft_ui_egui::HostRecent) -> bool {
        self.ext.post_open_request(Some((&recent.place, &recent.key))).is_ok()
    }

    /// Whether the host asked to close every open document since the last call.
    pub fn take_close_all(&self) -> bool {
        self.close_all.replace(false)
    }

    /// The saves the host answered since the last call.
    pub fn settled_saves(&self) -> Vec<(u64, Result<(), String>)> {
        std::mem::take(&mut self.confirms.borrow_mut().settled)
    }

    /// File ▸ Open, where the host declared places of its own: it is asked to open instead
    /// (`open-request`); true when it was asked.
    pub fn host_open(&self) -> bool {
        self.places.get() && self.ext.post_open_request(None).is_ok()
    }

    /// `?author=`: the host's name for the user signs comments and board notes.
    pub fn set_author(app: &mut FigApp, author: &str) {
        let author: String = author.trim().chars().take(MAX_AUTHOR_CHARS).collect();
        if !author.is_empty() {
            app.session.prefs.user_name = author;
        }
    }

    pub fn post_ready(&self) {
        let _ = self.host.post_ready(env!("CARGO_PKG_VERSION"), false);
    }

    /// The app couldn't start (no WebGPU or WebGL2, for example): let the host say so.
    pub fn post_failed(&self, error: &str) {
        let _ = self.host.post_failed(error, None);
    }

    /// The file the host opened as document `doc`, or the last Save As of it made, if any.
    pub fn host_file(&self, doc: u64) -> Option<String> {
        self.host_files.borrow().get(&doc).cloned()
    }

    /// Hand a file the app would otherwise download (a copy or an export) to the host, as a file
    /// of the document in front; `false` when nothing was delivered.
    pub fn post_save(&self, path: &str, bytes: &[u8]) -> bool {
        let front = self.told.borrow().as_ref().and_then(|t| t.0);
        match front {
            Some(doc) => self.post_save_of(doc, path, bytes, false),
            None => self.ext.post_save(&file_name(path), bytes, None, false, None, false).is_ok(),
        }
    }

    /// A file of document `doc`, unnumbered. The `saveAs` rule is this module's documentation's.
    fn post_save_of(&self, doc: u64, path: &str, bytes: &[u8], auto: bool) -> bool {
        let name = file_name(path);
        let save_as = crate::hostmode::save_as(&name, self.host_file(doc).as_deref());
        let sent = self.ext.post_save(&name, bytes, None, save_as, Some(doc), auto).is_ok();
        // After a Save As hand-over the host has the new file: the next Save of it sends its name
        // with `saveAs: false`. A failed send changes nothing, so a retry is still a Save As.
        if sent && save_as {
            self.host_files.borrow_mut().insert(doc, name);
        }
        sent
    }

    /// Report unsaved changes when they appear or go away, not every frame (the crate does not
    /// repeat a state it already reported).
    pub fn set_dirty(&self, dirty: bool) {
        self.host.post_dirty(dirty);
    }
}

/// The host answered save `ticket`; one the bridge isn't waiting for (answered twice, made up)
/// is ignored. A stored Save As makes its name the document's file.
fn settle(confirms: &RefCell<Confirms>, host_files: &RefCell<HashMap<u64, String>>, ticket: u64, outcome: Result<(), String>) {
    let mut c = confirms.borrow_mut();
    let Some(i) = c.waiting.iter().position(|(t, ..)| *t == ticket) else { return };
    let (_, doc, becomes) = c.waiting.remove(i);
    if outcome.is_ok()
        && let Some(name) = becomes
    {
        host_files.borrow_mut().insert(doc, name);
    }
    c.settled.push((ticket, outcome));
}

/// The file an open message carries, numbered and queued for the app's own open path. One the
/// app can't take (a `File`, or over the open limit) keeps its number and is answered at once:
/// `opened` with no document.
fn deliver_open(opens: &OpenQueue, counted: &RefCell<Opens>, ext: &Ext, ctx: &egui::Context, name: &str, data: &FileData) {
    let name = crate::hostmode::open_name(name);
    let n = {
        let mut c = counted.borrow_mut();
        c.last = c.last.wrapping_add(1);
        c.last
    };
    let refuse = |why: &str| {
        log::warn!("host file {name} {why}; not opened");
        let _ = ext.post_json("opened", &crate::hostmode::opened_fields(n, None, &name));
    };
    let FileData::Bytes(array) = data else {
        refuse("came as a File; FigCraft reads bytes only");
        return;
    };
    if figcraft_ui_egui::browser_can_read(&name, data.size()).is_err() {
        refuse("is too large to open in the browser");
        return;
    }
    // The number ties the document it becomes to this file (`opened`).
    counted.borrow_mut().names.insert(n, name.clone());
    opens.borrow_mut().push((n, name, array.to_vec()));
    ctx.request_repaint();
}

/// The last component of `path`, or the whole of it when there is none.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}
