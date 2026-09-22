//! Which build is this, read from the shell that loaded it.
//!
//! # Why this is worth its own module
//!
//! **Two usable identities were sitting in the deployed bytes and nothing read
//! either.** `tools/build-stamp.sh` has stamped the commit into
//! `<meta name="entity-build">` for some time, and trunk puts a content hash in
//! the main bundle's *filename*, which `assets/sw.js` already parses as
//! `BUNDLE_HASH`. Measured before this module existed: `grep -rn "entity-build"
//! src/` → **0 hits**. The application could not say what it was.
//!
//! Four separate features read this one value — the version display, the
//! "a new version is available" banner (currently keyed to `sw.js` bytes
//! changing, which is a different artifact on a different lifecycle and so
//! fires almost never), the boot-slot model's notion of slot identity, and a
//! bug report that can name a build. It is the cheapest item in the code-axis
//! design and the widest.
//!
//! # The two identities are not interchangeable
//!
//! - **`commit`** — from `<meta name="entity-build">`. Names the *source* the
//!   shell was cut from, and is the right thing to quote in a bug report or to
//!   resolve against `git log`. It can be `-dirty`, and it can be `unknown`
//!   (the stamp never fails a build; an unresolvable commit stamps `unknown`
//!   rather than stopping a release).
//! - **`bundle`** — the content hash in the main bundle's filename. Names the
//!   *bytes*, and is the one to compare across deployments. Two commits that
//!   differ only in documentation produce byte-identical output and therefore
//!   the same bundle hash — which is exactly what was measured across the two
//!   production apexes, and is why the commit alone cannot answer "are these
//!   two domains running the same code."
//!
//! Read from the DOM rather than baked in at compile time, deliberately: the
//! shell and the bundle are separately replaceable artifacts, and a compile-time
//! constant would report what the *bundle* believes rather than what the shell
//! that loaded it actually says. When those disagree, the disagreement is the
//! finding.

/// This build's identity, as the loaded shell reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildId {
    /// The commit `tools/build-stamp.sh` recorded, e.g. `a5d88fb` or
    /// `a5d88fb-dirty`. `None` when the shell carries no stamp — an unstamped
    /// dev build, or a shell from before the stamp existed.
    pub commit: Option<String>,
    /// The main bundle's content hash, from its filename. `None` if no bundle
    /// reference is discoverable, which on a booted app should not happen.
    pub bundle: Option<String>,
}

impl BuildId {
    /// A single line for a log or a bug report. Never empty: a build that can
    /// name neither identity still says so, because "unknown" is a finding and
    /// a blank is a bug in this function.
    pub fn describe(&self) -> String {
        match (&self.commit, &self.bundle) {
            (Some(c), Some(b)) => format!("{c} (bundle {b})"),
            (Some(c), None) => format!("{c} (bundle unknown)"),
            (None, Some(b)) => format!("unstamped (bundle {b})"),
            (None, None) => "unknown".to_string(),
        }
    }
}

/// Extract the bundle hash from any text containing a main-bundle reference.
///
/// Split out from the DOM read so it is testable natively — the parsing is the
/// part with a bug in it, and it must stay in step with `BUNDLE_HASH` in
/// `assets/sw.js`, which derives the same value independently for its
/// build-scoped worker cache. Two readers of one fact; if they disagree the
/// worker cache keys on a build the app does not think it is.
///
/// # It SCANS, and it requires the extension — both were bugs, found 2026-09-02
///
/// This used to take the **first** occurrence of `entity-browser-` and accept
/// whatever hex followed, extension or not. `sw.js`'s regex —
/// `entity-browser-([0-9a-f]{8,})(?:_bg)?\.(?:js|wasm)` — scans for the first
/// **match**, and requires the extension. So the two readers this doc comment
/// says must agree **did not**, on any document containing a near-miss ahead of
/// the real reference.
///
/// That is not hypothetical: adding a comment to `index.html` that mentioned
/// `/entity-browser-<hash>.js` in prose made this function return `None` for the
/// whole shell while `sw.js` kept working, so the app could not name the build it
/// was running. Caught by `the_app_reports_the_build_it_is_running`.
///
/// The literal is gone from that comment too — a shell should not carry decoys —
/// but relying on that is relying on nobody ever writing the string again. This
/// is the half that does not depend on memory.
pub fn parse_bundle_hash(haystack: &str) -> Option<String> {
    // `entity-browser-<hex>{,_bg}.{js,wasm}` — the same shape `sw.js` matches,
    // including the extension, which is what makes a prose mention a non-match
    // rather than a hijack.
    const NEEDLE: &str = "entity-browser-";
    let mut from = 0usize;
    while let Some(off) = haystack[from..].find(NEEDLE) {
        let start = from + off + NEEDLE.len();
        let rest = &haystack[start..];
        let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        // Trunk emits 16 hex chars; `sw.js` requires 8+. Match that floor rather
        // than the observed length, so a bundler change in either direction does
        // not silently stop identifying the build.
        if hex.len() >= 8 {
            let tail = &rest[hex.len()..];
            let tail = tail.strip_prefix("_bg").unwrap_or(tail);
            if tail.starts_with(".js") || tail.starts_with(".wasm") {
                return Some(hex);
            }
        }
        from = start;
    }
    None
}

/// Read this build's identity out of the live document.
#[cfg(target_arch = "wasm32")]
pub fn current() -> BuildId {
    let doc = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return BuildId::default(),
    };

    let commit = doc
        .query_selector("meta[name=\"entity-build\"]")
        .ok()
        .flatten()
        .and_then(|el| el.get_attribute("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // The bundle reference lives in the module script trunk generates. Read the
    // whole head rather than guessing at the element shape: trunk has changed
    // how it emits this more than once, and a selector tuned to today's output
    // would fail silently — reporting `None` for a bundle that is plainly
    // loaded — rather than loudly.
    // `query_selector("head")` rather than `Document::head()` — the latter needs
    // the `HtmlHeadElement` web-sys feature, and adding a feature to reach a
    // typed handle we immediately erase to a string is cost for nothing.
    let bundle = doc
        .query_selector("head")
        .ok()
        .flatten()
        .or_else(|| doc.document_element())
        .map(|h| h.inner_html())
        .and_then(|html| parse_bundle_hash(&html));

    BuildId { commit, bundle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_hash_trunk_actually_emits() {
        let head = r#"<script type="module">import init from '/entity-browser-55b2bdc214c60299.js';</script>"#;
        assert_eq!(
            parse_bundle_hash(head).as_deref(),
            Some("55b2bdc214c60299")
        );
    }

    #[test]
    fn parses_the_bg_wasm_spelling_too() {
        let head = r#"<link rel="preload" href="/entity-browser-e1373e07e8a35e2c_bg.wasm" as="fetch">"#;
        assert_eq!(
            parse_bundle_hash(head).as_deref(),
            Some("e1373e07e8a35e2c")
        );
    }

    #[test]
    fn a_too_short_hash_is_not_an_identity() {
        // Below `sw.js`'s own 8-char floor. Returning `Some("abc")` here would
        // let the app and the service worker key on different builds while both
        // believed they had an answer, which is worse than neither having one.
        assert_eq!(parse_bundle_hash("entity-browser-abc.js"), None);
    }

    /// **The two readers of this fact must agree, and they did not.** `sw.js`
    /// scans for a MATCH; this used to take the first *occurrence* and accept
    /// any hex after it. A prose mention of the bundle in `index.html` was
    /// enough to make them disagree.
    #[test]
    fn a_prose_mention_ahead_of_the_real_reference_does_not_hijack_the_parse() {
        let shell = r#"<head>
            <script>
                // ROOT-ABSOLUTE asset references (`/entity-browser-<hash>.js`)
            </script>
            <link rel="preload" href="/entity-browser-bf88e94c97f60358_bg.wasm">
            <script type="module" src="/entity-browser-bf88e94c97f60358.js"></script>
        </head>"#;
        assert_eq!(
            parse_bundle_hash(shell).as_deref(),
            Some("bf88e94c97f60358"),
            "a near-miss BEFORE the real reference must be skipped, not returned and not              treated as a failure for the whole document"
        );
    }

    /// The extension is what separates a reference from a mention, so a hex run
    /// long enough to look like a hash still is not one without it.
    #[test]
    fn a_long_hex_run_with_no_extension_is_not_a_bundle_reference() {
        assert_eq!(parse_bundle_hash("entity-browser-deadbeefdeadbeef"), None);
        assert_eq!(parse_bundle_hash("entity-browser-deadbeefdeadbeef.txt"), None);
        assert_eq!(
            parse_bundle_hash("entity-browser-deadbeefdeadbeef_bg.wasm").as_deref(),
            Some("deadbeefdeadbeef")
        );
        assert_eq!(
            parse_bundle_hash("entity-browser-deadbeefdeadbeef.js").as_deref(),
            Some("deadbeefdeadbeef")
        );
    }

    #[test]
    fn no_bundle_reference_is_none_not_a_panic() {
        assert_eq!(parse_bundle_hash("<title>Entity Browser</title>"), None);
    }

    #[test]
    fn describe_never_returns_an_empty_string() {
        // Each arm, because a blank line in a bug report is indistinguishable
        // from the field not being printed at all.
        for id in [
            BuildId { commit: Some("a5d88fb".into()), bundle: Some("dead".into()) },
            BuildId { commit: Some("a5d88fb".into()), bundle: None },
            BuildId { commit: None, bundle: Some("dead".into()) },
            BuildId::default(),
        ] {
            assert!(!id.describe().is_empty(), "empty describe() for {id:?}");
        }
    }

    #[test]
    fn a_dirty_stamp_is_carried_verbatim() {
        // `-dirty` is information, not noise: it says the build did not come
        // from a commit anyone else can fetch. Never strip it.
        let id = BuildId { commit: Some("a5d88fb-dirty".into()), bundle: None };
        assert!(id.describe().contains("-dirty"));
    }
}
