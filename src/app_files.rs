//! `app_files` — a file crossing an app sandbox, host side, as a **local
//! extension**.
//!
//! An entity-app runs in `sandbox="allow-scripts"`: an opaque origin whose only
//! channel is `postMessage`. The entity-apps contract gives it `state` (persist
//! this) and nothing for a *file*, so an app's output could only leave through
//! a browser download — a destination the entity tree cannot read and a peer
//! cannot serve (see `app_sandbox`'s `allow-downloads` note). This module is
//! the host half of a verb pair that returns the bytes to the HOST instead.
//!
//! ## Why every name here starts with `x-`
//!
//! The message contract belongs to entity-apps, and it is fixed on purpose.
//! We asked for this pair in `ROUTING-2026-09-11-q-entity-apps-…` and said, in
//! as many words, that we would not mint it unilaterally and that our prototype
//! would stay an `x-`-prefixed local extension their bridge ignores. So:
//!
//! | direction  | message              | payload                              |
//! |------------|----------------------|--------------------------------------|
//! | app → host | [`MSG_FILE`]         | `{name, media_type?, data}`          |
//! | host → app | [`MSG_FILE_RESULT`]  | `{ok, name, reason?, id?}`           |
//! | app → host | [`MSG_REQUEST_FILE`] | `{accept?}`                          |
//! | host → app | [`MSG_FILE`]         | `{name, media_type, data}`           |
//!
//! and the opt-in is the catalog key [`MANIFEST_KEY`]. If entity-apps rules on
//! a shape, the rename is this module's constants and one catalog field.
//!
//! ## Opt-in, and what "not declared" does
//!
//! **An app that does not declare [`MANIFEST_KEY`] sees exactly today's host.**
//! No button is drawn, and an `x-file` from it is dropped without a reply —
//! a reply is a message the app never asked to receive, and the manifest is
//! where it would have asked. The host logs the drop, so an app author who
//! forgot the key finds out from the console rather than from silence.
//!
//! An app that *does* declare it always gets an [`MSG_FILE_RESULT`]: the
//! host may refuse, and **refusal is a reported outcome, never silence** — the
//! failure `Entity.Export` has today, which its own documentation apologises
//! for.
//!
//! ## Where a file lands
//!
//! In [`crate::kept_files`]: our own `system/content` plus a PRIVATE manifest
//! under the app's own directory (`apps/{set}/files/{app}/`). **Not an offer.**
//! Until 2026-09-14 it was one — reuse that looked free and shared every file a
//! person pulled out of an app with every peer they were connected to. Sharing is
//! now a separate, deliberate step (File Transfer's "Offer to peers"). The size
//! limit is still [`crate::file_offer::MAX_OFFER_BYTES`], one number.
//!
//! ## Why the host cannot open a file picker on request
//!
//! A browser opens a file chooser only inside a user gesture, and a
//! `postMessage` from a frame is not one. So [`MSG_REQUEST_FILE`] cannot
//! *open* anything: it raises the player's "send a file" control, and the
//! person's tap on that control is the gesture.

use crate::file_offer::MAX_OFFER_BYTES;

/// The catalog (`index.json` / `app/app-catalog` entry) key an app sets to
/// `true` to receive the file verbs.
pub const MANIFEST_KEY: &str = "x-files";

/// app → host: "keep this file"; host → app: "here is a file".
pub const MSG_FILE: &str = "x-file";
/// app → host: "I would like a file" — raises the host's control, see module doc.
pub const MSG_REQUEST_FILE: &str = "x-request-file";
/// host → app: what happened to an [`MSG_FILE`] the app sent.
pub const MSG_FILE_RESULT: &str = "x-file-result";

/// Longest file name accepted, in bytes. The common filesystem limit, so a name
/// that fits here fits wherever the file goes next (a download, a guest's 9p).
pub const MAX_NAME_BYTES: usize = 255;

/// Why the host did not keep a file. Each variant names **whose** problem it is,
/// because an app author and a person reading the player bar go to different
/// places for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The app's catalog entry does not carry [`MANIFEST_KEY`]. The host drops
    /// the message rather than replying (module doc).
    NotDeclared,
    /// No `name`, or not a string. The app's defect.
    NoName,
    /// A name that is empty after taking its last path component, is `.`/`..`,
    /// holds a control character, or is longer than [`MAX_NAME_BYTES`].
    BadName,
    /// No `data`, or not an `ArrayBuffer`/typed array. The app's defect.
    NoData,
    /// Over [`MAX_OFFER_BYTES`]. A limit of this host, not a defect of the app.
    TooLarge { size: u64, limit: u64 },
    /// The bytes were admitted and the store refused them. The host's problem.
    StoreFailed(String),
}

impl Refusal {
    /// The stable machine code sent as `reason` in [`MSG_FILE_RESULT`]. An app
    /// branches on this, never on the human sentence, so these strings are
    /// wire and do not change with a translation.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NotDeclared => "not-declared",
            Refusal::NoName => "no-name",
            Refusal::BadName => "bad-name",
            Refusal::NoData => "no-data",
            Refusal::TooLarge { .. } => "too-large",
            Refusal::StoreFailed(_) => "store-failed",
        }
    }

    /// The human reason, for the player's status line. **English inside a
    /// localized frame** (`apps.file.refused` carries it as `{why}`) — the same
    /// recorded shape as `file_offer::too_large_message`: a translated frame
    /// with the specific reason dropped would be worse than one English clause.
    pub fn describe(&self) -> String {
        match self {
            Refusal::NotDeclared => format!("this app does not declare {MANIFEST_KEY}"),
            Refusal::NoName => "the app sent no file name".to_string(),
            Refusal::BadName => "the app sent a file name that cannot be used".to_string(),
            Refusal::NoData => "the app sent no file contents".to_string(),
            Refusal::TooLarge { size, limit } => format!(
                "it is {} and this browser keeps files up to {}",
                crate::file_offer::human_bytes(*size),
                crate::file_offer::human_bytes(*limit),
            ),
            Refusal::StoreFailed(why) => why.clone(),
        }
    }
}

/// Reduce what an app called its file to a single safe name.
///
/// **The last path component, because a name is not a path.** Apps pass what
/// they have — the run-environment's files are `mnt/report.txt` — and the
/// directory is a fact about the app's world that means nothing in ours. It is
/// also where a hostile bundle would put `../`. Both separators are split on:
/// a name built on Windows still carries `\`.
pub fn clean_name(raw: &str) -> Result<String, Refusal> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if last.is_empty() || last == "." || last == ".." {
        return Err(Refusal::BadName);
    }
    if last.chars().any(char::is_control) {
        return Err(Refusal::BadName);
    }
    if last.len() > MAX_NAME_BYTES {
        return Err(Refusal::BadName);
    }
    Ok(last.to_string())
}

/// Decide whether the host keeps a file, **before** reading any bytes into the
/// store. `name` and `size` are `None` when the message did not carry a usable
/// value of that kind.
///
/// The order is the order of whose problem it is: an undeclared app first (it
/// gets no reply at all), then the app's own malformations, then this host's
/// limit. A too-large file with no name reports the missing name, because that
/// is the thing its author can fix.
pub fn admit(declared: bool, name: Option<&str>, size: Option<u64>) -> Result<String, Refusal> {
    if !declared {
        return Err(Refusal::NotDeclared);
    }
    let name = clean_name(name.ok_or(Refusal::NoName)?)?;
    let size = size.ok_or(Refusal::NoData)?;
    if size > MAX_OFFER_BYTES {
        return Err(Refusal::TooLarge { size, limit: MAX_OFFER_BYTES });
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_app_is_refused_before_anything_about_its_message_is_read() {
        // Even a perfectly formed file: the manifest is the opt-in.
        assert_eq!(admit(false, Some("a.txt"), Some(3)), Err(Refusal::NotDeclared));
        // And a malformed one reports NotDeclared, not its malformation -- the
        // host does not reply to an undeclared app, so nothing else is owed.
        assert_eq!(admit(false, None, None), Err(Refusal::NotDeclared));
    }

    #[test]
    fn a_name_is_its_last_path_component() {
        assert_eq!(clean_name("mnt/report.txt").unwrap(), "report.txt");
        assert_eq!(clean_name("C:\\Users\\x\\pic.png").unwrap(), "pic.png");
        assert_eq!(clean_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(clean_name("  spaced.txt  ").unwrap(), "spaced.txt");
    }

    #[test]
    fn names_that_reduce_to_nothing_or_hold_controls_are_refused() {
        for bad in ["", "/", "dir/", "..", "a/..", ".", "   ", "evil\nname", "tab\there", "nul\0"] {
            assert_eq!(clean_name(bad), Err(Refusal::BadName), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_name_limit_is_asserted_from_both_sides() {
        assert!(clean_name(&"a".repeat(MAX_NAME_BYTES)).is_ok());
        assert_eq!(clean_name(&"a".repeat(MAX_NAME_BYTES + 1)), Err(Refusal::BadName));
        // BYTES, not chars: 128 two-byte chars is 256 bytes.
        assert_eq!(clean_name(&"é".repeat(128)), Err(Refusal::BadName));
    }

    #[test]
    fn the_size_limit_is_the_offer_limit_asserted_from_both_sides() {
        assert!(admit(true, Some("f"), Some(MAX_OFFER_BYTES)).is_ok());
        assert_eq!(
            admit(true, Some("f"), Some(MAX_OFFER_BYTES + 1)),
            Err(Refusal::TooLarge { size: MAX_OFFER_BYTES + 1, limit: MAX_OFFER_BYTES })
        );
        // An empty file is a real file (a placeholder, a truncated export).
        assert_eq!(admit(true, Some("empty"), Some(0)).unwrap(), "empty");
    }

    #[test]
    fn the_app_defect_is_reported_before_the_host_limit() {
        assert_eq!(admit(true, None, Some(MAX_OFFER_BYTES + 1)), Err(Refusal::NoName));
        assert_eq!(admit(true, Some("f"), None), Err(Refusal::NoData));
    }

    #[test]
    fn every_refusal_has_its_own_wire_code() {
        let all = [
            Refusal::NotDeclared,
            Refusal::NoName,
            Refusal::BadName,
            Refusal::NoData,
            Refusal::TooLarge { size: 1, limit: 0 },
            Refusal::StoreFailed(String::new()),
        ];
        let codes: std::collections::BTreeSet<_> = all.iter().map(Refusal::code).collect();
        assert_eq!(codes.len(), all.len(), "two refusals share a code: {codes:?}");
        // Pinned by literal: these are wire, and a test spelled in terms of the
        // enum would follow a rename it exists to catch.
        assert_eq!(Refusal::TooLarge { size: 1, limit: 0 }.code(), "too-large");
        assert_eq!(Refusal::StoreFailed(String::new()).code(), "store-failed");
    }

    #[test]
    fn the_extension_names_are_x_prefixed() {
        // The promise in ROUTING-2026-09-11-q: a local extension, never a name
        // in entity-apps' contract. Unprefixing one of these is a contract change
        // and belongs to them.
        for name in [MANIFEST_KEY, MSG_FILE, MSG_REQUEST_FILE, MSG_FILE_RESULT] {
            assert!(name.starts_with("x-"), "{name} must stay x-prefixed until entity-apps rules");
        }
    }
}
