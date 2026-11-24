//! The host messages beyond `craft-web-host`'s contract (rhodium-org fork, offered upstream): a
//! host's own places to open from, and saves a host confirms or refuses. They travel the same way
//! as the crate's (`postMessage` to and from the same-origin parent, or `CustomEvent`s on the
//! canvas), with the same `figcraft:` prefix, so they belong in the crate once upstream takes them;
//! until then this module carries them for FigCraft alone. The same messages as the
//! rhodium-org/wordcraft fork's, less those FigCraft has no place for.
//!
//! | Kind | Direction | Fields |
//! |---|---|---|
//! | `places` | host → app | `places: [{ id, label }]`, replacing the list; any place means the host opens files |
//! | `open-request` | app → host | none: File ▸ Open was asked for; the host shows its own way to open and sends `open` |
//! | `confirm-saves` | host → app | `value`: from now on each document Save carries an `id` and counts once the host answers |
//! | `save-failed` | host → app | `id`, `message?`: the save was not stored; the document stays unsaved and the app shows why |
//! | `close-all` | host → app | none: close every open document, unsaved changes and all (the host asked the person), before the next `open` |
//!
//! The crate's `saved {id}` confirms a save. A host that never sends `confirm-saves` sees saves
//! without an `id`, as before, and one that declares no places leaves File ▸ Open to the browser's
//! picker.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};

use craft_web_host::{Mode, message_type};

/// The kinds this module hears from the host.
pub const INCOMING: [&str; 4] = ["places", "confirm-saves", "save-failed", "close-all"];

#[derive(Clone)]
enum Target {
    /// The same-origin parent window; only it is heard and answered.
    Parent { window: web_sys::Window, parent: web_sys::Window, origin: String },
    /// The app's canvas; messages are `CustomEvent`s on it.
    Page { canvas: web_sys::EventTarget },
}

/// The extension messages' side of the connection to the hosting page.
#[derive(Clone)]
pub struct Ext {
    app: &'static str,
    target: Target,
}

impl Ext {
    /// The same transport `craft_web_host::web::Host::connect` chose (`mode`): it has already
    /// checked that a parent is framing the app from the page's own origin.
    pub fn new(app: &'static str, mode: Mode, canvas: &web_sys::Element) -> Option<Self> {
        let target = match mode {
            Mode::Parent => {
                let window = web_sys::window()?;
                let parent = window.parent().ok().flatten()?;
                let origin = window.location().origin().ok()?;
                Target::Parent { window, parent, origin }
            }
            Mode::Page => Target::Page { canvas: canvas.clone().into() },
        };
        Some(Self { app, target })
    }

    /// Hear the host's extension messages: `(kind, fields as JSON)`. A message that isn't plain
    /// data (no JSON form) arrives as `null`.
    pub fn listen(&self, handler: impl Fn(&str, serde_json::Value) + 'static) {
        let app = self.app;
        let handler = std::rc::Rc::new(handler);
        match &self.target {
            Target::Parent { window, parent, origin } => {
                let (parent, origin) = (parent.clone(), origin.clone());
                let on_message = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
                    if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                        return;
                    }
                    let data = event.data();
                    let Some(kind) = kind_of(app, &data) else { return };
                    handler(kind, json(&data));
                });
                if window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
                    on_message.forget();
                }
            }
            Target::Page { canvas } => {
                for kind in INCOMING {
                    let handler = handler.clone();
                    let on_event = Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |event: web_sys::CustomEvent| {
                        handler(kind, json(&event.detail()));
                    });
                    let name = message_type(app, kind);
                    if canvas.add_event_listener_with_callback(&name, on_event.as_ref().unchecked_ref()).is_ok() {
                        on_event.forget();
                    }
                }
            }
        }
    }

    /// `open-request`: File ▸ Open was asked for, and the host opens files.
    pub fn post_open_request(&self) -> Result<(), String> {
        let message_type = message_type(self.app, "open-request");
        let error = |e: JsValue| e.as_string().unwrap_or_else(|| format!("{e:?}"));
        match &self.target {
            Target::Parent { parent, origin, .. } => {
                let message = js_sys::Object::new();
                js_sys::Reflect::set(&message, &JsValue::from_str("type"), &JsValue::from_str(&message_type))
                    .map_err(|_| "couldn't build the message".to_string())?;
                parent.post_message(&message, origin).map_err(error)
            }
            Target::Page { canvas } => {
                let init = web_sys::CustomEventInit::new();
                init.set_detail(&js_sys::Object::new().into());
                let event = web_sys::CustomEvent::new_with_event_init_dict(&message_type, &init).map_err(error)?;
                canvas.dispatch_event(&event).map(|_| ()).map_err(error)
            }
        }
    }
}

/// The extension kind a parent's message names for `app`, or `None` for anything else (the
/// crate's own kinds, another app's messages).
fn kind_of(app: &str, data: &JsValue) -> Option<&'static str> {
    let message_type = js_sys::Reflect::get(data, &JsValue::from_str("type")).ok()?.as_string()?;
    let kind = message_type.strip_prefix(app)?.strip_prefix(':')?;
    INCOMING.into_iter().find(|k| *k == kind)
}

/// A host's message as JSON, read through the browser's own `JSON.stringify`; `null` when it has
/// no JSON form (a cycle, a function).
fn json(data: &JsValue) -> serde_json::Value {
    js_sys::JSON::stringify(data).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::Value::Null)
}
