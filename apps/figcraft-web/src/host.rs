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
//!
//! In host mode the app starts with no document (the home screen) and the host sends the file to
//! edit; `?author=` names the user for comments and board notes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use craft_web_host::web::{FileData, Host, Message};
use figcraft_ui_egui::{Delivery, FigApp};

use crate::host_ext::Ext;

/// The app id in the host messages (`figcraft:ready`).
const APP: &str = "figcraft";

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

/// Files the host sent, on their way to the app's own open queue ([`figcraft_ui_egui::pending_open`]):
/// `(name, bytes)`.
pub type OpenQueue = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

/// Saves a host confirms ([`crate::host_ext`]): numbered once the host asks, and answered later.
#[derive(Default)]
struct Confirms {
    /// The host asked for numbered saves (`confirm-saves {value: true}`).
    on: bool,
    /// The last ticket given.
    last: u64,
    /// Saves handed over and not yet answered: the ticket, and the name the document's file
    /// becomes once it is stored (a Save As), if any.
    waiting: Vec<(u64, Option<String>)>,
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
    /// The file the host opened, and after a `saveAs` hand-over the document's file: the name a
    /// Save under (`saveAs: false`) is decided against.
    host_file: Rc<RefCell<Option<String>>>,
}

impl Bridge {
    /// The bridge for this page, or `None` when FigCraft isn't hosted: no `?host=parent` and no
    /// `data-host="page"` on the canvas, an unframed `?host=parent` page (it stays the normal app
    /// with downloads), or a parent on another origin.
    pub fn connect(canvas: &web_sys::HtmlCanvasElement) -> Option<Self> {
        let host = Host::connect(APP, canvas)?;
        let ext = Ext::new(APP, host.mode(), canvas)?;
        Some(Self { host: Rc::new(host), ext, places: Rc::default(), confirms: Rc::default(), host_file: Rc::default() })
    }

    /// Hear the files the host sends, and queue each one on `opens` for the app's own open path
    /// (the shell drains it into [`figcraft_ui_egui::pending_open`] every frame, so a host file
    /// opens like a dropped one).
    pub fn listen(&self, opens: OpenQueue, ctx: egui::Context) {
        let (host_file, confirms, repaint) = (self.host_file.clone(), self.confirms.clone(), ctx.clone());
        self.host.listen(move |message| match message {
            Message::Open { name, data } => deliver_open(&opens, &ctx, &host_file, &name, &data),
            Message::Saved { id } => {
                if let Some(ticket) = crate::hostmode::ticket(&serde_json::Value::String(id)) {
                    settle(&confirms, &host_file, ticket, Ok(()));
                    ctx.request_repaint();
                }
            }
            _ => {}
        });
        // The extension messages (crate::host_ext).
        let (places, confirms, host_file) = (self.places.clone(), self.confirms.clone(), self.host_file.clone());
        self.ext.listen(move |kind, message| {
            match kind {
                "places" => places.set(crate::hostmode::offers_places(&message)),
                "confirm-saves" => confirms.borrow_mut().on = crate::hostmode::flag(&message, "value"),
                "save-failed" => {
                    if let Some((ticket, why)) = crate::hostmode::save_failed(&message) {
                        settle(&confirms, &host_file, ticket, Err(why));
                    }
                }
                _ => return,
            }
            repaint.request_repaint();
        });
    }

    /// Hand a document file the app saves to the host. Numbered, and answered later, once the
    /// host asked to confirm saves; otherwise done when handed over, as before.
    pub fn deliver(&self, path: &str, bytes: &[u8]) -> Delivery {
        let name = file_name(path);
        let host_file = self.host_file.borrow().clone();
        let save_as = crate::hostmode::save_as(&name, host_file.as_deref());
        let mut confirms = self.confirms.borrow_mut();
        if !confirms.on {
            drop(confirms);
            return if self.post_save(path, bytes) { Delivery::Delivered } else { Delivery::Refused };
        }
        let ticket = confirms.last.wrapping_add(1);
        if self.host.post_save(&name, bytes, Some(&ticket.to_string()), save_as).is_err() {
            return Delivery::Refused;
        }
        confirms.last = ticket;
        // After a Save As is stored the host has the new file: the next Save of it is no Save As.
        confirms.waiting.push((ticket, save_as.then_some(name)));
        Delivery::Waiting(ticket)
    }

    /// The saves the host answered since the last call.
    pub fn settled_saves(&self) -> Vec<(u64, Result<(), String>)> {
        std::mem::take(&mut self.confirms.borrow_mut().settled)
    }

    /// File ▸ Open, where the host declared places of its own: it is asked to open instead
    /// (`open-request`); true when it was asked.
    pub fn host_open(&self) -> bool {
        self.places.get() && self.ext.post_open_request().is_ok()
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

    /// The document file the host opened or the last Save As made, if any.
    pub fn host_file(&self) -> Option<String> {
        self.host_file.borrow().clone()
    }

    /// Hand a file the app would otherwise download to the host; `false` when nothing was
    /// delivered. The `saveAs` rule is this module's documentation's.
    pub fn post_save(&self, path: &str, bytes: &[u8]) -> bool {
        let name = file_name(path);
        let host_file = self.host_file.borrow().clone();
        let save_as = crate::hostmode::save_as(&name, host_file.as_deref());
        let sent = self.host.post_save(&name, bytes, None, save_as).is_ok();
        // After a Save As hand-over the host has the new file: the next Save of it sends its name
        // with `saveAs: false`. A failed send changes nothing, so a retry is still a Save As.
        if sent && save_as {
            *self.host_file.borrow_mut() = Some(name);
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
fn settle(confirms: &RefCell<Confirms>, host_file: &RefCell<Option<String>>, ticket: u64, outcome: Result<(), String>) {
    let mut c = confirms.borrow_mut();
    let Some(i) = c.waiting.iter().position(|(t, _)| *t == ticket) else { return };
    let (_, becomes) = c.waiting.remove(i);
    if outcome.is_ok()
        && let Some(name) = becomes
    {
        *host_file.borrow_mut() = Some(name);
    }
    c.settled.push((ticket, outcome));
}

/// The file an open message carries, queued for the app's own open path, or nothing when the
/// payload isn't one (a `File` the app doesn't read, or over the open limit).
fn deliver_open(opens: &OpenQueue, ctx: &egui::Context, host_file: &RefCell<Option<String>>, name: &str, data: &FileData) {
    let name = crate::hostmode::open_name(name);
    let FileData::Bytes(array) = data else {
        log::warn!("host file {name} came as a File; FigCraft reads bytes only; not opened");
        return;
    };
    if figcraft_ui_egui::browser_can_read(&name, data.size()).is_err() {
        log::warn!("host file {name} is too large to open in the browser; not opened");
        return;
    }
    // The document's file is now the one the host sent: a Save of it under this name is
    // `saveAs: false`.
    *host_file.borrow_mut() = Some(name.clone());
    opens.borrow_mut().push((name, array.to_vec()));
    ctx.request_repaint();
}

/// The last component of `path`, or the whole of it when there is none.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}
