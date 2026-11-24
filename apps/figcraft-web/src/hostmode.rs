//! Pure host-mode rules, shared by the bridge ([`crate::host`]) and testable without a browser
//! (like [`crate::query`]): the name the bridge fills in when an open request has none, and whether
//! a handed-over file becomes the document's file. Which pages host the app is `craft-web-host`'s.

/// The file name used when an open request carries no usable one.
pub const DEFAULT_OPEN_NAME: &str = "Untitled.figcraft";

/// The extensions of files that can be a document's own file: the formats `file.save` and
/// `file.saveAs` write (`file.open` opens both as native documents). A `.fig` file is an export
/// copy instead, and so is everything else the app hands over (rendered images, SVG, PDF,
/// variables).
const DOC_EXTS: &[&str] = &["figcraft", "json"];

/// The suffix `variable.export`'s downloads carry: a `.json` that is a variables sidecar, not a
/// document, so its extension alone can't tell the `saveAs` decision.
const VARIABLES_SIDECAR: &str = ".variables.json";

/// The name `craft-web-host` gives an open request that carries no usable one (it cleans a host's
/// name and falls back to this).
const NAMELESS: &str = "document";

/// A usable file name for an open request: the cleaned name the crate hands over, or
/// [`DEFAULT_OPEN_NAME`] when the host sent none.
pub fn open_name(name: &str) -> String {
    if name == NAMELESS { DEFAULT_OPEN_NAME.to_string() } else { name.to_string() }
}

/// Whether the file `name` becomes the document's own file when handed over (`saveAs: true`),
/// given the file the host opened (`None`: the host never sent one). The rule:
///
/// - A document file (`.figcraft` or `.json`, [`DOC_EXTS`]'s formats) under a name the host didn't
///   open — Save As, or a Save of a document the host never sent — is the document's new file:
///   `true`. Names compare without case.
/// - A Save of the host's own file under its name is not: `false`.
/// - Everything else is a copy or export that never becomes the document — rendered images, SVG,
///   PDF, `.fig` copies, variables — and the variables sidecar counts as one whatever its `.json`
///   extension says: `false`.
pub fn save_as(name: &str, host_file: Option<&str>) -> bool {
    if name.to_ascii_lowercase().ends_with(VARIABLES_SIDECAR) {
        return false;
    }
    let ext = std::path::Path::new(name).extension().and_then(std::ffi::OsStr::to_str);
    if !ext.is_some_and(|e| DOC_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e))) {
        return false;
    }
    match host_file {
        Some(host) => !host.eq_ignore_ascii_case(name),
        None => true,
    }
}

/// One open document as the host hears of it in `documents`: its tab's stable id, its name and
/// whether it has unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEntry {
    pub doc: u64,
    pub name: String,
    pub dirty: bool,
}

/// The `documents` message's fields: the tab in front (`null` on the home screen) and every open
/// document in tab order.
pub fn documents_fields(front: Option<u64>, docs: &[DocEntry]) -> serde_json::Value {
    serde_json::json!({
        "front": front,
        "docs": docs.iter().map(|d| serde_json::json!({"doc": d.doc, "name": d.name, "dirty": d.dirty})).collect::<Vec<_>>(),
    })
}

/// The `opened` message's fields: the host's number for its open (1 for the first file it sent),
/// the document the file became (`null` when it didn't open) and the file's name.
pub fn opened_fields(open: u64, doc: Option<u64>, name: &str) -> serde_json::Value {
    serde_json::json!({"open": open, "doc": doc, "name": name})
}

/// How many documents the host's Recents list keeps.
pub const MAX_RECENTS: usize = 12;

/// The longest place id, key, label or name taken from a host's `document-place`.
const MAX_FIELD_CHARS: usize = 500;

fn field(message: &serde_json::Value, key: &str) -> Option<String> {
    let v: String = message[key].as_str()?.trim().chars().take(MAX_FIELD_CHARS).collect();
    (!v.is_empty()).then_some(v)
}

/// A host's `document-place`: the document it is about (`doc`, if given) and the recent entry
/// it makes, `{place, key}` required, `label` defaulting to the place and `name` to `fallback`
/// (the file the host opened as that document), then to the key. `None` without a place or a key.
pub fn document_place(message: &serde_json::Value, fallback: Option<&str>) -> Option<(Option<u64>, figcraft_ui_egui::HostRecent)> {
    let place = field(message, "place")?;
    let key = field(message, "key")?;
    let label = field(message, "label").unwrap_or_else(|| place.clone());
    let name = field(message, "name").or_else(|| fallback.map(str::to_string)).unwrap_or_else(|| key.clone());
    Some((message["doc"].as_u64(), figcraft_ui_egui::HostRecent { name, place, key, label }))
}

/// `recents` with `recent` in front: one entry per place and key, newest first, at most
/// [`MAX_RECENTS`].
pub fn remember(recents: &mut Vec<figcraft_ui_egui::HostRecent>, recent: figcraft_ui_egui::HostRecent) {
    recents.retain(|r| !(r.place == recent.place && r.key == recent.key));
    recents.insert(0, recent);
    recents.truncate(MAX_RECENTS);
}

/// The Recents list as kept in the browser's storage.
pub fn recents_to_json(recents: &[figcraft_ui_egui::HostRecent]) -> String {
    serde_json::Value::Array(recents.iter().map(|r| serde_json::json!({"name": r.name, "place": r.place, "key": r.key, "label": r.label})).collect())
        .to_string()
}

/// The Recents list read back from the browser's storage; anything unreadable is an empty list.
pub fn recents_from_json(text: &str) -> Vec<figcraft_ui_egui::HostRecent> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let mut recents = Vec::new();
    for item in items.iter().rev() {
        if let Some((_, r)) = document_place(item, None) {
            remember(&mut recents, r);
        }
    }
    recents
}

/// The longest reason taken from a host's `save-failed`.
const MAX_REASON_CHARS: usize = 500;

/// Whether a host's `places` message offers any place to open from: a `places` list holding at
/// least one entry with a non-empty string `id`. Anything else (no list, an empty one, junk)
/// leaves File ▸ Open to the browser's picker.
pub fn offers_places(message: &serde_json::Value) -> bool {
    message["places"].as_array().is_some_and(|places| places.iter().any(|p| p["id"].as_str().is_some_and(|id| !id.trim().is_empty())))
}

/// A host message's boolean `key`: true only for JSON `true`.
pub fn flag(message: &serde_json::Value, key: &str) -> bool {
    message[key].as_bool() == Some(true)
}

/// A save's ticket from the `id` the host echoes back: the decimal number the bridge sent, as a
/// string (the crate's `saved {id}`) or a number.
pub fn ticket(id: &serde_json::Value) -> Option<u64> {
    match id {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// A host's `save-failed`: the save's ticket and what to tell the person, the host's own reason
/// (trimmed and capped) or [`SAVE_FAILED`] when it gave none. `None` without a usable `id`.
pub fn save_failed(message: &serde_json::Value) -> Option<(u64, String)> {
    let ticket = ticket(&message["id"])?;
    let reason: String = message["message"].as_str().map(str::trim).unwrap_or_default().chars().take(MAX_REASON_CHARS).collect();
    Some((ticket, if reason.is_empty() { SAVE_FAILED.to_string() } else { reason }))
}

/// Said when the host refused a save without saying why.
pub const SAVE_FAILED: &str = "The page didn't store the file, so it stays unsaved.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents_name_the_front_tab_and_every_document_in_order() {
        let docs = [DocEntry { doc: 7, name: "Poster".into(), dirty: true }, DocEntry { doc: 9, name: "Poster".into(), dirty: false }];
        assert_eq!(
            documents_fields(Some(9), &docs),
            json!({"front": 9, "docs": [{"doc": 7, "name": "Poster", "dirty": true}, {"doc": 9, "name": "Poster", "dirty": false}]}),
            "two documents of one name stay apart by their ids"
        );
        assert_eq!(documents_fields(None, &[]), json!({"front": null, "docs": []}), "the home screen");
    }

    #[test]
    fn opened_names_the_host_s_open_and_the_document_or_none() {
        assert_eq!(opened_fields(2, Some(7), "Poster.figcraft"), json!({"open": 2, "doc": 7, "name": "Poster.figcraft"}));
        assert_eq!(opened_fields(3, None, "Broken.figcraft"), json!({"open": 3, "doc": null, "name": "Broken.figcraft"}));
    }

    fn recent(place: &str, key: &str) -> figcraft_ui_egui::HostRecent {
        figcraft_ui_egui::HostRecent { name: format!("{key}.figcraft"), place: place.into(), key: key.into(), label: place.into() }
    }

    #[test]
    fn a_document_place_needs_a_place_and_a_key_and_fills_in_the_rest() {
        let (doc, r) =
            document_place(&json!({"doc": 7, "place": "github", "key": "acme/x@main:a.figcraft", "label": "acme/x · main"}), Some("a.figcraft"))
                .unwrap();
        assert_eq!(doc, Some(7));
        assert_eq!(
            r,
            figcraft_ui_egui::HostRecent {
                name: "a.figcraft".into(),
                place: "github".into(),
                key: "acme/x@main:a.figcraft".into(),
                label: "acme/x · main".into()
            }
        );
        let (_, r) = document_place(&json!({"place": "computer", "key": "Wall.figcraft"}), None).unwrap();
        assert_eq!((r.name.as_str(), r.label.as_str()), ("Wall.figcraft", "computer"), "the key names it, the place labels it");
        for junk in [json!({"place": "github"}), json!({"key": "k"}), json!({"place": " ", "key": "k"}), json!({"place": 3, "key": "k"}), json!(null)]
        {
            assert_eq!(document_place(&junk, None), None, "{junk}");
        }
        let long = "x".repeat(2000);
        assert_eq!(document_place(&json!({"place": "p", "key": long}), None).map(|(_, r)| r.key.len()), Some(MAX_FIELD_CHARS));
    }

    #[test]
    fn recents_keep_one_entry_per_place_and_key_newest_first_and_a_dozen_at_most() {
        let mut recents = Vec::new();
        remember(&mut recents, recent("github", "a"));
        remember(&mut recents, recent("computer", "a"));
        remember(&mut recents, recent("github", "a"));
        assert_eq!(recents.iter().map(|r| (r.place.as_str(), r.key.as_str())).collect::<Vec<_>>(), [("github", "a"), ("computer", "a")]);
        for i in 0..30 {
            remember(&mut recents, recent("github", &i.to_string()));
        }
        assert_eq!(recents.len(), MAX_RECENTS);
        assert_eq!(recents[0].key, "29");
    }

    #[test]
    fn recents_survive_the_browser_s_storage_and_junk_reads_as_none() {
        let recents = vec![recent("github", "b"), recent("computer", "a")];
        assert_eq!(recents_from_json(&recents_to_json(&recents)), recents, "order kept");
        assert!(recents_from_json("not json").is_empty());
        assert_eq!(recents_from_json(r#"[{"place":"p"},{"place":"p","key":"k"}]"#).len(), 1, "an entry without a key is dropped");
    }

    #[test]
    fn places_are_offered_only_by_a_list_with_a_real_place() {
        assert!(offers_places(&json!({"places": [{"id": "computer", "label": "This computer"}, {"id": "github"}]})));
        for junk in [json!({}), json!({"places": []}), json!({"places": "computer"}), json!({"places": [{"id": ""}, {"id": 3}, "x"]}), json!(null)] {
            assert!(!offers_places(&junk), "{junk}");
        }
    }

    #[test]
    fn flags_are_true_only_for_true() {
        assert!(flag(&json!({"value": true}), "value"));
        for junk in [json!({"value": "true"}), json!({"value": 1}), json!({}), json!(null)] {
            assert!(!flag(&junk, "value"), "{junk}");
        }
    }

    #[test]
    fn a_refused_save_names_its_ticket_and_a_reason() {
        assert_eq!(
            save_failed(&json!({"id": "4", "message": "  GitHub holds a newer version.  "})),
            Some((4, "GitHub holds a newer version.".into()))
        );
        assert_eq!(save_failed(&json!({"id": 5})), Some((5, SAVE_FAILED.into())));
        assert_eq!(save_failed(&json!({"id": "6", "message": ""})), Some((6, SAVE_FAILED.into())));
        assert_eq!(save_failed(&json!({"id": "6", "message": "x".repeat(2000)})).map(|(_, m)| m.len()), Some(MAX_REASON_CHARS));
        for junk in [json!({}), json!({"id": "a"}), json!({"id": -1}), json!({"id": null}), json!(null)] {
            assert_eq!(save_failed(&junk), None, "{junk}");
        }
    }

    #[test]
    fn open_names_fall_back_to_the_default_when_the_host_sent_none() {
        assert_eq!(open_name("Poster.figcraft"), "Poster.figcraft");
        assert_eq!(open_name("Poster 2.json"), "Poster 2.json");
        assert_eq!(open_name("document"), DEFAULT_OPEN_NAME, "what the crate says for a missing or blank name");
    }

    #[test]
    fn a_save_of_the_host_s_file_is_not_save_as() {
        assert!(!save_as("Poster.figcraft", Some("Poster.figcraft")));
        assert!(!save_as("poster.FIGCRAFT", Some("Poster.figcraft")), "names compare without case");
        assert!(!save_as("design.json", Some("design.json")));
    }

    #[test]
    fn a_new_document_file_is_save_as() {
        assert!(save_as("Poster 2.figcraft", Some("Poster.figcraft")));
        assert!(save_as("Poster.json", Some("Poster.figcraft")), "the other format a document's file takes");
        assert!(save_as("Untitled.figcraft", None), "the host never sent a file");
        assert!(save_as("Poster.figcraft", Some("design.json")), "the opened .json saved as .figcraft is a new file");
    }

    #[test]
    fn copies_and_exports_never_become_the_document_s_file() {
        for name in ["Poster.png", "Poster@2x.jpg", "Poster.svg", "Poster.pdf", "Poster.fig", "Poster.csv", "Poster.variables.css"] {
            assert!(!save_as(name, Some("Poster.figcraft")), "{name} is a copy");
            assert!(!save_as(name, None), "{name} is a copy even without a host file");
        }
    }

    #[test]
    fn the_variables_sidecar_is_a_copy_whatever_its_extension() {
        assert!(!save_as("Poster.variables.json", Some("Poster.figcraft")));
        assert!(!save_as("Poster.variables.json", None));
        assert!(!save_as("design.variables.json", Some("design.variables.json")));
    }
}
