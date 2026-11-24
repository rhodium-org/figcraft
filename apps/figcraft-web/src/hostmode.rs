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
