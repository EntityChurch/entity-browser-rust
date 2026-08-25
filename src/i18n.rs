//! Locale registry — the single source of truth for the app's languages and
//! layout direction. **Theming's structural twin** (see `theme_tokens.rs`):
//! where a theme is `name → color map` + a `scheme` (ltr/dark render mode),
//! a locale is `id → (label, dir)` + eventually a message catalog. This
//! module is the i18n counterpart of the theme *registry* — it owns the roster
//! of locales, boot detection, persistence, and the act of driving `lang`/`dir`
//! onto the DOM. The message *catalog* (`t()` / `t_plural()`, the compiled-in
//! `en` strings, plural selection) is a separate layer that lands in P1; this
//! is P0 — the wiring that makes the app *announce* its locale and light up
//! `dir` correctly, useful on its own even before a single string is
//! translated. Design: `docs/architecture/reviews/DESIGN-I18N-L10N.md` (§3.2,
//! §5 P0); format ADR: `docs/adr/0001-i18n-catalog-format.md`.
//!
//! ## The `dir` primitive (the load-bearing part)
//!
//! `dir` is to layout what a theme's `scheme` is to color: an irreducible,
//! per-locale primitive with exactly two values (`ltr` / `rtl`). Applying a
//! locale sets `lang`/`dir` on **two** elements (implementation-review
//! finding, DESIGN §3.2):
//!
//! - the **shadow host** (`#dom-layer`) — the app renders into an open shadow
//!   root, and `dir` on `<html>` does *not* cross that boundary. The host's
//!   `dir` inherits inward (the same way `dom/style.rs` pushes theme vars
//!   across `:host`). Setting only `<html>` is a silent RTL no-op.
//! - `<html>` — for the **light DOM** that lives outside the shadow root
//!   (status bar, `#site-layer` overlay, loading screen, runtime banners).
//!
//! Both, every time. The localStorage mirror (`LANG_LS_KEY`) lets [`boot_choice`]
//! pick the locale synchronously at first paint — no flash, no wait on the tree
//! round-trip (the same paint-hint pattern the theme boot mirror uses, and the
//! same reason: the Worker-arm settings read returns the default until the
//! cache seeds, so we detect from `navigator`/localStorage directly).

/// A registered locale: a stable id, a human label for the Settings picker,
/// and its layout direction. The twin of [`crate::theme_tokens::Theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    /// BCP-47 language id, persisted in `SettingsState.language` (e.g. `"en"`).
    pub id: &'static str,
    /// Human label for the Settings picker (e.g. `"English"`).
    pub label: &'static str,
    /// Layout direction: `"ltr"` or `"rtl"`. The non-string primitive — twin of
    /// a theme's `scheme`. Emitted as the `dir` attribute on the shadow host +
    /// `<html>`.
    pub dir: &'static str,
    /// A development pseudo-locale (e.g. `en-XA`), never auto-selected by
    /// [`detect`] — it exists to surface RTL / hardcoded-string bugs, not to be
    /// a real user language. Explicitly pickable in Settings.
    pub pseudo: bool,
}

/// The default locale id — always complete, compiled in, frame one.
pub const DEFAULT: &str = "en";

/// The locale roster. **P0 ships the real `en` plus one pseudo-locale**
/// (`en-XA`), deliberately: `en-XA` forces `dir=rtl` with *no* string change,
/// so the shadow-host `dir` plumbing and every physical-CSS layout bug become
/// visible and testable end-to-end *now* — before any catalog or translation
/// exists ("don't get blindsided by RTL", DESIGN §5.1). In P1 the same id grows
/// its string-mangling transform (bracket + accent + pad) once `t()` exists;
/// today it is dir-only. Real target locales (`es`/`ar`/`he`) join here as
/// their catalogs land (P4) — one row each, exactly as adding a built-in theme
/// is one entry in `THEMES`.
pub const LOCALES: &[Locale] = &[
    Locale { id: "en", label: "English", dir: "ltr", pseudo: false },
    // First real target locales (P4). Labels are endonyms (each language's own
    // name — picker convention). Their catalogs are the `locales/*.json`
    // overlays baked at build time; `he`/`ar` carry `dir: "rtl"` — the primitive
    // that drives the whole logical-CSS layout flip. A locale is pickable from
    // this roster even under a lean `en`-only build (strings fall back to `en`,
    // but `dir` still flips) — the roster and the embedded catalog are
    // independent (see `locales/README.md`).
    Locale { id: "es", label: "Español", dir: "ltr", pseudo: false },
    Locale { id: "he", label: "עברית", dir: "rtl", pseudo: false },
    Locale { id: "ar", label: "العربية", dir: "rtl", pseudo: false },
    // The broader major-language roster. Endonym labels; `fa`/`ur` are RTL. A
    // row is pickable (and flips `dir`) as soon as it lands here; its catalog
    // (`locales/<id>.json`) can arrive later — until then strings fall back to
    // `en` (the roster and the embedded catalog are independent). Plural rules
    // for every id here are pinned in `plural_category`.
    Locale { id: "fr", label: "Français", dir: "ltr", pseudo: false },
    Locale { id: "de", label: "Deutsch", dir: "ltr", pseudo: false },
    Locale { id: "it", label: "Italiano", dir: "ltr", pseudo: false },
    Locale { id: "pt", label: "Português", dir: "ltr", pseudo: false },
    Locale { id: "nl", label: "Nederlands", dir: "ltr", pseudo: false },
    Locale { id: "sv", label: "Svenska", dir: "ltr", pseudo: false },
    Locale { id: "da", label: "Dansk", dir: "ltr", pseudo: false },
    Locale { id: "no", label: "Norsk", dir: "ltr", pseudo: false },
    Locale { id: "fi", label: "Suomi", dir: "ltr", pseudo: false },
    Locale { id: "ru", label: "Русский", dir: "ltr", pseudo: false },
    Locale { id: "uk", label: "Українська", dir: "ltr", pseudo: false },
    Locale { id: "pl", label: "Polski", dir: "ltr", pseudo: false },
    Locale { id: "cs", label: "Čeština", dir: "ltr", pseudo: false },
    Locale { id: "ro", label: "Română", dir: "ltr", pseudo: false },
    Locale { id: "el", label: "Ελληνικά", dir: "ltr", pseudo: false },
    Locale { id: "hu", label: "Magyar", dir: "ltr", pseudo: false },
    Locale { id: "tr", label: "Türkçe", dir: "ltr", pseudo: false },
    Locale { id: "zh", label: "中文", dir: "ltr", pseudo: false },
    Locale { id: "ja", label: "日本語", dir: "ltr", pseudo: false },
    Locale { id: "ko", label: "한국어", dir: "ltr", pseudo: false },
    Locale { id: "vi", label: "Tiếng Việt", dir: "ltr", pseudo: false },
    Locale { id: "th", label: "ไทย", dir: "ltr", pseudo: false },
    Locale { id: "id", label: "Bahasa Indonesia", dir: "ltr", pseudo: false },
    Locale { id: "hi", label: "हिन्दी", dir: "ltr", pseudo: false },
    Locale { id: "bn", label: "বাংলা", dir: "ltr", pseudo: false },
    Locale { id: "fa", label: "فارسی", dir: "rtl", pseudo: false },
    Locale { id: "ur", label: "اردو", dir: "rtl", pseudo: false },
    // Pseudo-locale: RTL, no translation. The RTL-blindside detector.
    Locale { id: "en-XA", label: "Pseudo (RTL)", dir: "rtl", pseudo: true },
];

/// The full roster (for the Settings picker). Twin of `all_themes()`.
pub fn available_locales() -> &'static [Locale] {
    LOCALES
}

/// Look up a locale by id. `None` for an unknown id.
pub fn locale(id: &str) -> Option<&'static Locale> {
    LOCALES.iter().find(|l| l.id == id)
}

/// Resolve a locale by id, falling back to [`DEFAULT`] (`en`) for an unknown or
/// stale id (e.g. a persisted locale from before it existed / after removal).
/// Never fails — the twin of theming's `lookup(name) -> DARK` fallback.
pub fn resolve(id: &str) -> &'static Locale {
    locale(id).unwrap_or_else(|| locale(DEFAULT).expect("en locale always present"))
}

// ---------------------------------------------------------------------------
// Boot detection, persistence, and applying `lang`/`dir` (wasm)
// ---------------------------------------------------------------------------

/// localStorage key mirroring the chosen locale id. The tree
/// (`SettingsState.language`) is the durable record, but it isn't readable
/// until peers boot; this mirror lets [`boot_choice`] pick the locale
/// synchronously at first paint. Twin of [`crate::theme_tokens::THEME_LS_KEY`].
pub const LANG_LS_KEY: &str = "entity_language";

#[cfg(target_arch = "wasm32")]
fn ls_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|ls| ls.get_item(key).ok().flatten())
}

#[cfg(target_arch = "wasm32")]
fn ls_set(key: &str, value: Option<&str>) {
    if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        match value {
            Some(v) => {
                let _ = ls.set_item(key, v);
            }
            None => {
                let _ = ls.remove_item(key);
            }
        }
    }
}

/// Detect the user's preferred locale from `navigator.language`, intersected
/// with the **selectable** (non-pseudo) roster, matching on the primary subtag
/// (`"en-US"` → `en`). Falls back to [`DEFAULT`]. Pseudo-locales are never
/// auto-detected — they are dev tools, chosen explicitly in Settings.
#[cfg(target_arch = "wasm32")]
pub fn detect() -> String {
    let nav_lang = web_sys::window()
        .map(|w| w.navigator())
        .and_then(|n| n.language())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let primary = nav_lang.split('-').next().unwrap_or("");
    LOCALES
        .iter()
        .filter(|l| !l.pseudo)
        .find(|l| l.id.eq_ignore_ascii_case(&nav_lang) || l.id.eq_ignore_ascii_case(primary))
        .map(|l| l.id.to_string())
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// Native stub — no `navigator`; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn detect() -> String {
    DEFAULT.to_string()
}

/// The locale to install at boot: the localStorage mirror if it names a known
/// locale, else the `navigator.language` [`detect`]ion, else [`DEFAULT`].
/// Twin of [`crate::theme_tokens::boot_choice`].
#[cfg(target_arch = "wasm32")]
pub fn boot_choice() -> String {
    ls_get(LANG_LS_KEY)
        .filter(|id| locale(id).is_some())
        .unwrap_or_else(detect)
}

/// Native stub — no localStorage; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn boot_choice() -> String {
    DEFAULT.to_string()
}

/// Apply a locale, the twin of [`crate::theme_tokens::apply_and_persist`]:
///
/// 1. set the process-active locale (what [`t`] resolves against),
/// 2. bump [`locale_generation`] so the frame loop force-rebuilds every open
///    window ([`mark_all_dirty`]) — a language switch changes baked DOM *text*
///    (unlike a CSS-only theme flip), so per-window dirty flags aren't enough
///    (DESIGN §3.2, finding 1),
/// 3. (wasm) persist the boot mirror + drive `lang`/`dir` onto the shadow host
///    (`#dom-layer`, inherits into the shadow tree) **and** `<html>` (light DOM).
///
/// The durable tree write is the caller's job (`SettingsState`); this is the
/// runtime side. Steps 1–2 run on native too so `t()` + the generation counter
/// are unit-testable.
pub fn apply(id: &str) {
    let loc = resolve(id);
    set_active(loc.id);
    mark_all_dirty();
    #[cfg(target_arch = "wasm32")]
    {
        ls_set(LANG_LS_KEY, Some(loc.id));
        install_lang_dir(loc);
    }
}

/// Set `lang`/`dir` on the shadow host + `<html>`. Idempotent; safe to call at
/// boot (the `#dom-layer` host exists in the static HTML before the shadow root
/// attaches — the attribute persists and inheritance applies once shadow
/// content renders).
#[cfg(target_arch = "wasm32")]
pub fn install_lang_dir(loc: &Locale) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    // `<html>` — light DOM (status bar, site overlay, loading screen, banners)
    // and the document's declared language.
    if let Some(html) = document.document_element() {
        let _ = html.set_attribute("lang", loc.id);
        let _ = html.set_attribute("dir", loc.dir);
    }
    // Shadow host — `dir` on `<html>` does NOT cross the shadow boundary, so it
    // must be set here for the app's rendered UI to flip (DESIGN §3.2, finding
    // 2). The same boundary the theme layer already crosses for `:host` vars.
    if let Some(host) = document.get_element_by_id("dom-layer") {
        let _ = host.set_attribute("lang", loc.id);
        let _ = host.set_attribute("dir", loc.dir);
    }
}

// ===========================================================================
// P1 — the message catalog + `t()` (the string seam)
// ===========================================================================
//
// The twin of the theme *token* layer: raw English → a message **key** resolved
// through one indirection (`t(key, args)`), just as raw hex became `var(--tok)`.
// P1 stands up the seam and its reactivity primitive; the 358-string extraction
// is P4. Only a small seed catalog + the Settings demonstrators flow through
// `t()` today ("migrate no strings yet" — the mechanism-first / theming play).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// CLDR plural categories. `en`/`es` use only `One`/`Other`; `he` uses
/// `One`/`Two`/`Other`; `ar` uses all six. Category sets are pinned to the
/// authoritative CLDR cardinal rules (see [`plural_category`] + its vector
/// tests, and ADR-0001 §"Plural resolution"). The selector stays **honest**
/// about which categories it supports — that honesty was the single input that
/// could have flipped the bespoke-vs-Fluent decision (design §8); it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// One catalog entry: a simple template, or a set of CLDR plural forms selected
/// by the `n` argument. Templates carry `{named}` slots interpolated (and
/// bidi-isolated) by [`t`] / [`t_plural`].
pub enum Message {
    Simple(&'static str),
    Plural(&'static [(PluralCategory, &'static str)]),
}

/// The compiled-in `en` base catalog — always complete, frame one (the twin of
/// the built-in `DARK` theme). Keys are dotted + namespaced by surface. P1 seeds
/// a small **base vocabulary** (shared across apps — translate once, extend
/// per-app, design §7) plus the Settings demonstrators actually wired through
/// `t()`; the long-tail extraction is P4.
pub const EN: &[(&str, Message)] = &[
    // -- base vocabulary (ecosystem-shareable common UI terms) --
    // Buttons/verbs — reused across every window; translate once, extend per-app.
    ("btn.save", Message::Simple("Save")),
    ("btn.cancel", Message::Simple("Cancel")),
    ("btn.delete", Message::Simple("Delete")),
    ("btn.copy", Message::Simple("Copy")),
    ("btn.connect", Message::Simple("Connect")),
    ("btn.edit", Message::Simple("Edit")),
    ("btn.refresh", Message::Simple("Refresh")),
    ("btn.back", Message::Simple("← Back")),
    ("btn.revert", Message::Simple("Revert")),
    ("btn.create", Message::Simple("Create")),
    ("btn.move", Message::Simple("Move")),
    ("btn.authorize", Message::Simple("Authorize")),
    ("btn.add_peer", Message::Simple("Add peer")),
    ("btn.find", Message::Simple("Find")),
    ("btn.count", Message::Simple("Count")),
    ("btn.trace", Message::Simple("Trace")),
    ("btn.dismiss", Message::Simple("Dismiss")),
    ("btn.reload", Message::Simple("Reload")),
    ("btn.clear", Message::Simple("Clear")),
    ("btn.expand", Message::Simple("Expand")),
    ("btn.collapse", Message::Simple("Collapse")),
    // Field/section labels — common nouns shared across windows.
    ("label.status", Message::Simple("Status")),
    ("label.title", Message::Simple("Title")),
    ("label.peer", Message::Simple("Peer")),
    ("label.target", Message::Simple("Target")),
    ("label.address", Message::Simple("Address")),
    ("label.device", Message::Simple("Device")),
    ("label.name", Message::Simple("Name")),
    ("label.label", Message::Simple("Label")),
    ("label.ice_servers", Message::Simple("Reflectors (STUN)")),
    // A relay is a different kind of thing from a reflector and the labels say
    // so: a reflector only tells you your own address, a relay forwards every
    // packet for you. Naming the protocol in parentheses keeps the field
    // findable for someone who was handed "TURN credentials" by an operator.
    ("label.relay", Message::Simple("Relay (TURN)")),
    ("label.relay_username", Message::Simple("Relay username")),
    ("label.relay_credential", Message::Simple("Relay credential")),
    ("label.role", Message::Simple("Role")),
    ("label.peer_id", Message::Simple("Peer ID")),
    ("label.level", Message::Simple("Level")),
    ("label.results", Message::Simple("Results")),
    ("label.content", Message::Simple("Content")),
    ("label.system", Message::Simple("System")),
    ("label.user", Message::Simple("User")),
    ("label.state_path", Message::Simple("State: {path}")),
    // Target-selector option shared by File Transfer + Execute Console.
    ("label.remote_option", Message::Simple("Remote: {name}")),
    // Transient status glyphs.
    ("status.copied", Message::Simple("Copied ✓")),
    ("status.loading", Message::Simple("Loading…")),
    ("status.saved", Message::Simple("saved")),
    ("status.stopped", Message::Simple("stopped")),
    ("status.granted", Message::Simple("granted")),
    // Status-bar durability label (capitalized, sentence position — distinct
    // from status.saved "saved" used as an inline chip).
    ("statusbar.saved", Message::Simple("Saved")),
    ("statusbar.not_saved", Message::Simple("Not saved")),
    // Inline on/off state words (system-peers posture line).
    ("status.on", Message::Simple("on")),
    ("status.off", Message::Simple("off")),
    // -- status chips (components::conn_chip / auth_chip) --
    ("chip.connected", Message::Simple("Connected")),
    ("chip.connecting", Message::Simple("Connecting…")),
    ("chip.offline", Message::Simple("Offline")),
    ("chip.authorized", Message::Simple("Authorized")),
    ("chip.pending", Message::Simple("Pending")),
    ("chip.not_authorized", Message::Simple("Not authorized")),
    ("chip.not_verified", Message::Simple("Not verified")),
    // -- execute console surface --
    ("execute.handler", Message::Simple("Handler")),
    ("execute.operation", Message::Simple("Operation")),
    ("execute.handler_uri", Message::Simple("Handler URI")),
    ("execute.resource", Message::Simple("Resource")),
    ("execute.execute", Message::Simple("Execute")),
    ("execute.guided", Message::Simple("Guided")),
    ("execute.raw", Message::Simple("Raw")),
    // -- peer connections surface --
    ("peers.known_devices", Message::Simple("Known devices")),
    ("peers.connect_device", Message::Simple("Connect to a device")),
    ("peers.pair_qr", Message::Simple("Pair a device (QR)")),
    ("peers.scan_qr", Message::Simple("Scan a QR code")),
    (
        "peers.scan_hint",
        Message::Simple("Scan this from another device to connect it here."),
    ),
    ("peers.show_qr", Message::Simple("Show QR code")),
    ("peers.reconnect", Message::Simple("Reconnect")),
    ("peers.forget", Message::Simple("Forget")),
    // -- peer management window --
    ("peers.add_a_peer", Message::Simple("Add a peer")),
    ("peers.col_peer_id", Message::Simple("Peer ID")),
    ("peers.col_kind", Message::Simple("Kind")),
    ("peers.group_system", Message::Simple("System peers")),
    ("peers.group_system_sub", Message::Simple("always-on")),
    ("peers.group_user", Message::Simple("Your peers")),
    ("peers.group_user_sub", Message::Simple("created by you")),
    ("peers.open_tree", Message::Simple("Tree")),
    ("peers.start", Message::Simple("Start")),
    ("peers.stop", Message::Simple("Stop")),
    ("peers.footer_workers", Message::Simple("{peers} — 1 boot + {workers}")),
    // create-peer kind one-line descriptions (S6). Keep the technical acronyms
    // (IndexedDB/OPFS/Web Worker) verbatim; translate the surrounding prose.
    (
        "peers.kind_desc.frontend",
        Message::Simple("Main thread of this tab, in-memory. Temporary — cleared when you reload."),
    ),
    (
        "peers.kind_desc.frontend_idb",
        Message::Simple("Main thread of this tab, saved to IndexedDB. Survives reload."),
    ),
    (
        "peers.kind_desc.backend_memory",
        Message::Simple("A background Web Worker, in-memory. Temporary — cleared when you reload."),
    ),
    (
        "peers.kind_desc.backend_opfs",
        Message::Simple("A background Web Worker, saved to OPFS. Survives reload."),
    ),
    (
        "peers.kind_desc.native",
        Message::Simple("A separate native desktop process with its own on-disk store. Saved."),
    ),
    // create-peer kind short labels + unavailability reasons.
    ("peers.kind_label.frontend", Message::Simple("This tab · temporary")),
    ("peers.kind_label.frontend_idb", Message::Simple("This tab · saved")),
    ("peers.kind_label.backend_memory", Message::Simple("Background · temporary")),
    ("peers.kind_label.backend_opfs", Message::Simple("Background · saved")),
    ("peers.kind_label.native", Message::Simple("Native app · saved")),
    (
        "peers.kind_unavail.worker_mode",
        Message::Simple("not available in worker mode"),
    ),
    (
        "peers.kind_unavail.web_worker",
        Message::Simple("needs Web Worker support"),
    ),
    ("peers.kind_unavail.opfs", Message::Simple("needs OPFS storage")),
    ("peers.kind_unavail.desktop", Message::Simple("desktop app only")),
    // -- system peers window: posture line + boot-surface descriptions --
    ("system_peers.yours", Message::Simple("{n} yours")),
    ("system_peers.startup_line", Message::Simple("Startup: {value}")),
    ("system_peers.creation_line", Message::Simple("Peer creation: {value}")),
    ("boot_surface.chrome", Message::Simple("Window chrome")),
    ("boot_surface.site", Message::Simple("Content site")),
    ("boot_surface.site_named", Message::Simple("Content site: {id}")),
    ("boot_surface.window", Message::Simple("Window: {label}")),
    // -- window-chrome tooltips (title attrs) --
    ("tooltip.close_window", Message::Simple("Close window")),
    ("tooltip.open_windows", Message::Simple("Open windows")),
    ("tooltip.fill_window", Message::Simple("Fill the window")),
    ("tooltip.restore_size", Message::Simple("Back to normal size")),
    ("tooltip.maximize_window", Message::Simple("Maximize window")),
    ("tooltip.restore_window", Message::Simple("Restore window")),
    ("tooltip.menu", Message::Simple("Menu")),
    ("tooltip.back", Message::Simple("Back")),
    ("tooltip.site_home", Message::Simple("Go to site home")),
    ("tooltip.unsaved", Message::Simple("Unsaved changes")),
    // -- input placeholders --
    ("kb.title_placeholder", Message::Simple("Article title")),
    ("theme.name_placeholder", Message::Simple("name (a-z, 0-9, dashes)")),
    // Built-in theme names, keyed `theme.<name>` (dashes → underscores) and
    // resolved by `theme_tokens::display_label`. The descriptive words are
    // translated; the scheme proper nouns (Solarized, Nord, Dracula, Gruvbox,
    // Monokai) name a specific published palette and stay as written, the way
    // a typeface name would.
    ("theme.dark", Message::Simple("Dark")),
    ("theme.light", Message::Simple("Light")),
    ("theme.sepia", Message::Simple("Sepia")),
    ("theme.neon", Message::Simple("Neon")),
    ("theme.solarized_dark", Message::Simple("Solarized Dark")),
    ("theme.solarized_light", Message::Simple("Solarized Light")),
    ("theme.nord", Message::Simple("Nord")),
    ("theme.nord_light", Message::Simple("Nord Light")),
    ("theme.dracula", Message::Simple("Dracula")),
    ("theme.gruvbox_dark", Message::Simple("Gruvbox Dark")),
    ("theme.gruvbox_light", Message::Simple("Gruvbox Light")),
    ("theme.monokai", Message::Simple("Monokai")),
    // Site-appearance dropdown.
    ("theme.site_theme", Message::Simple("Site's theme")),
    ("theme.match_system", Message::Simple("Match system theme")),
    ("theme.always", Message::Simple("Always {theme}")),
    // User-theme name/scheme validation — surfaced on the Theme Editor status
    // line, so user-facing despite reading like developer errors.
    ("theme.err_name_empty", Message::Simple("theme name is empty")),
    (
        "theme.err_name_too_long",
        Message::Simple("theme name too long (max {max})"),
    ),
    (
        "theme.err_name_charset",
        Message::Simple("theme name must be lowercase letters, digits, and dashes"),
    ),
    (
        "theme.err_name_reserved",
        Message::Simple("“{name}” is a reserved appearance mode"),
    ),
    (
        "theme.err_name_builtin",
        Message::Simple("“{name}” is a built-in theme"),
    ),
    (
        "theme.err_scheme",
        Message::Simple("scheme must be “dark” or “light”, got “{got}”"),
    ),
    ("theme.err_no_tokens", Message::Simple("theme has no token values")),
    ("theme.label_placeholder", Message::Simple("display label")),
    // -- file transfer surface --
    (
        "filetransfer.no_results",
        Message::Simple("List or pull a file to see results."),
    ),
    ("filetransfer.device", Message::Simple("Device")),
    ("filetransfer.shared_files", Message::Simple("Shared files")),
    ("filetransfer.send_file", Message::Simple("Send a file")),
    ("filetransfer.from_peer", Message::Simple("From peer")),
    ("filetransfer.results", Message::Simple("Results")),
    ("filetransfer.pull_selected", Message::Simple("\u{2b07} Pull selected file")),
    ("filetransfer.upload_file", Message::Simple("Upload a file")),
    ("filetransfer.title_files", Message::Simple("Files — {label}")),
    (
        "filetransfer.needs_auth",
        Message::Simple(
            "The exposing device must authorize this one before transfers succeed.",
        ),
    ),
    ("filetransfer.authorize_device", Message::Simple("Authorize this device")),
    ("filetransfer.browse_shared", Message::Simple("Browse shared files")),
    ("filetransfer.share_empty", Message::Simple("This share is empty.")),
    ("filetransfer.offering", Message::Simple("Files you are offering")),
    (
        "filetransfer.offer_hint",
        Message::Simple(
            "Anyone you have met can pull these from this device — you do not \
             need to be connected when you put one up. Up to {limit} per file.",
        ),
    ),
    ("filetransfer.offer_file", Message::Simple("\u{2b06} Offer a file")),
    (
        "filetransfer.offer_none",
        Message::Simple("You are not offering anything."),
    ),
    ("filetransfer.stop_offering", Message::Simple("Stop offering")),
    ("filetransfer.col_file", Message::Simple("File")),
    ("filetransfer.col_size", Message::Simple("Size")),
    // -- site editor surface --
    (
        "siteeditor.hint",
        Message::Simple(
            "Build a site as a tree of folders and pages. Saves write to your \
             peer's tree; the Site Browser window picks them up automatically.",
        ),
    ),
    ("siteeditor.create_site", Message::Simple("Create site")),
    ("siteeditor.delete_site", Message::Simple("Delete site")),
    ("siteeditor.save_page", Message::Simple("Save page")),
    ("siteeditor.delete_page", Message::Simple("Delete page")),
    ("siteeditor.title_optional", Message::Simple("Title (optional)")),
    ("siteeditor.page_title_ph", Message::Simple("Page title")),
    ("siteeditor.move_rename", Message::Simple("Move/rename to:")),
    ("siteeditor.editing", Message::Simple("Editing: {id}")),
    ("siteeditor.adding_to", Message::Simple("Adding to: {target}")),
    (
        "siteeditor.confirm_move_unsaved",
        Message::Simple("This page has unsaved changes that will be lost when it moves. Move anyway?"),
    ),
    // site editor — list / create / navigator / editor chrome
    ("siteeditor.your_sites", Message::Simple("Your sites")),
    ("siteeditor.new_site", Message::Simple("New site")),
    (
        "siteeditor.no_sites",
        Message::Simple("(none yet — use \u{201c}New site\u{201d} below)"),
    ),
    ("siteeditor.renders_ok", Message::Simple("Renders in the browser")),
    ("siteeditor.wont_render", Message::Simple("Won't render: {reason}")),
    (
        "siteeditor.site_id_placeholder",
        Message::Simple("new site-id (letters, digits, - _)"),
    ),
    (
        "siteeditor.confirm_delete_site",
        Message::Simple("Delete the entire site '{id}' and all its pages? This cannot be undone."),
    ),
    ("siteeditor.pages", Message::Simple("Pages")),
    ("siteeditor.site_root_row", Message::Simple("\u{1f3e0} / (site root)")),
    ("siteeditor.no_pages", Message::Simple("(no pages yet — add one below)")),
    ("siteeditor.site_root", Message::Simple("site root")),
    (
        "siteeditor.page_name_placeholder",
        Message::Simple("page or folder name"),
    ),
    ("siteeditor.add_page", Message::Simple("+ Add page")),
    ("siteeditor.add_folder", Message::Simple("+ Add folder")),
    ("siteeditor.markdown_label", Message::Simple("Markdown — {page}")),
    ("siteeditor.unsaved", Message::Simple("\u{25cf} Unsaved changes")),
    ("siteeditor.hide_preview", Message::Simple("Hide preview")),
    ("siteeditor.show_preview", Message::Simple("Show preview")),
    (
        "siteeditor.confirm_delete_page",
        Message::Simple("Delete the page '{page}'? This cannot be undone."),
    ),
    // site editor — model status notices
    (
        "siteeditor.site_exists",
        Message::Simple("A site '{site_id}' already exists."),
    ),
    (
        "siteeditor.default_page",
        Message::Simple("# {title}\n\nWelcome to **{title}**.\n"),
    ),
    (
        "siteeditor.site_created",
        Message::Simple("Created site '{site_id}'."),
    ),
    (
        "siteeditor.folder_note",
        Message::Simple("Folder '{name}' — add a page here to keep it."),
    ),
    (
        "siteeditor.select_site_page",
        Message::Simple("Select a site and page first."),
    ),
    ("siteeditor.page_saved", Message::Simple("Saved '{page}'.")),
    ("siteeditor.select_site", Message::Simple("Select a site first.")),
    (
        "siteeditor.page_exists",
        Message::Simple("A page '{slug}' already exists."),
    ),
    ("siteeditor.page_added", Message::Simple("Added page '{slug}'.")),
    ("siteeditor.page_deleted", Message::Simple("Deleted page '{slug}'.")),
    (
        "siteeditor.site_deleted",
        Message::Simple("Deleted site '{site_id}'."),
    ),
    (
        "siteeditor.same_path",
        Message::Simple("New path is the same as the current one."),
    ),
    (
        "siteeditor.no_page_to_move",
        Message::Simple("No page '{from}' to move."),
    ),
    (
        "siteeditor.page_moved",
        Message::Simple("Moved '{from}' → '{to}'."),
    ),
    // site editor — validation errors
    ("siteeditor.err_enter_site_id", Message::Simple("Enter a site id.")),
    (
        "siteeditor.err_site_id_long",
        Message::Simple("Site id is too long (max {max})."),
    ),
    (
        "siteeditor.err_site_id_chars",
        Message::Simple("Site id may use only letters, digits, '-' and '_'."),
    ),
    (
        "siteeditor.err_enter_page_name",
        Message::Simple("Enter a page name."),
    ),
    (
        "siteeditor.err_page_slashes",
        Message::Simple("Page name must not start or end with '/'."),
    ),
    (
        "siteeditor.err_page_empty_seg",
        Message::Simple("Page name has an empty path segment ('//')."),
    ),
    (
        "siteeditor.err_page_dots",
        Message::Simple("Page name must not contain '.' or '..' segments."),
    ),
    (
        "siteeditor.err_page_seg_long",
        Message::Simple("A page-name segment is too long (max {max})."),
    ),
    (
        "siteeditor.err_page_chars",
        Message::Simple("Page name may use only letters, digits, '-', '_' and '/'."),
    ),
    (
        "siteeditor.no_manifest",
        Message::Simple("no manifest for this site"),
    ),
    (
        "siteeditor.no_root_page",
        Message::Simple("no '{root}' page — create it to make the site render"),
    ),
    // -- entity tree surface --
    ("entitytree.source_none", Message::Simple("None (manual)")),
    (
        "entitytree.source_app",
        Message::Simple("App aggregate (peer: {peer})"),
    ),
    ("entitytree.up", Message::Simple("Up")),
    ("entitytree.selection_source", Message::Simple("Selection source")),
    (
        "entitytree.select_prompt",
        Message::Simple("Select an entity from the tree"),
    ),
    ("entitytree.inspector", Message::Simple("Inspector")),
    ("entitytree.none_selected", Message::Simple("No entity selected")),
    ("entitytree.raw_hash", Message::Simple("Raw Hash")),
    ("entitytree.no_entity_at", Message::Simple("No entity at: {path}")),
    ("entitytree.type_line", Message::Simple("Type: {type}")),
    // -- developer tools: the field labels / hints that are real UI, as
    //    distinct from the command output around them (DESIGN-I18N-L10N §6) --
    ("queryconsole.type_filter", Message::Simple("Type Filter:")),
    (
        "queryconsole.type_filter_hint",
        Message::Simple("Exact type, glob (app/*), or * for all"),
    ),
    ("queryconsole.path_prefix", Message::Simple("Path Prefix:")),
    (
        "queryconsole.path_prefix_hint",
        Message::Simple("Filter results by path prefix (optional)"),
    ),
    ("queryconsole.ref_filter", Message::Simple("Ref Filter (hash):")),
    (
        "queryconsole.ref_filter_hint",
        Message::Simple("Find entities referencing a content hash (hex, optional)"),
    ),
    ("queryconsole.path_filter", Message::Simple("Path Filter:")),
    (
        "queryconsole.path_filter_hint",
        Message::Simple("Find entities linking to this path (optional)"),
    ),
    ("queryconsole.limit", Message::Simple("Limit:")),
    (
        "queryconsole.include_entities",
        // Leading space is intentional: it separates the label from its checkbox.
        Message::Simple(" Include full entities in results"),
    ),
    ("queryconsole.no_results", Message::Simple("(no results yet — run a query)")),
    (
        "chaintrace.hint",
        Message::Simple("Enter the chain_id to walk continuation + chain-error markers on this peer."),
    ),
    (
        "pathtap.hint",
        Message::Simple("Live dispatch facts from this peer (newest first; ring buffer)."),
    ),
    // Inspector field labels.
    ("entitytree.field_path", Message::Simple("Path")),
    ("entitytree.field_type", Message::Simple("Type")),
    ("entitytree.field_hash", Message::Simple("Hash")),
    ("entitytree.field_data_size", Message::Simple("Data size")),
    ("entitytree.field_algorithm", Message::Simple("Algorithm")),
    // Footer counts + the inspector's data-size value. Three cardinal plurals;
    // the footer joins its two with the app's neutral separator.
    (
        "entitytree.entity_count",
        Message::Plural(&[
            (PluralCategory::One, "{n} entity"),
            (PluralCategory::Other, "{n} entities"),
        ]),
    ),
    (
        "entitytree.path_count",
        Message::Plural(&[
            (PluralCategory::One, "{n} path"),
            (PluralCategory::Other, "{n} paths"),
        ]),
    ),
    (
        "entitytree.bytes",
        Message::Plural(&[
            (PluralCategory::One, "{n} byte"),
            (PluralCategory::Other, "{n} bytes"),
        ]),
    ),
    // -- system peers surface --
    ("syspeers.system_peer", Message::Simple("System peer")),
    ("syspeers.system_backend", Message::Simple("System backend")),
    // -- system overview surface --
    ("sysoverview.clear_logs", Message::Simple("Clear logs")),
    ("sysoverview.logs", Message::Simple("Logs")),
    ("sysoverview.no_logs", Message::Simple("(no logs yet)")),
    (
        "sysoverview.backend_logs_note",
        Message::Simple(
            "The System backend runs in the desktop app. Its live logs aren't \
             streamed over this browser session yet — exposing them to an \
             authorized peer is planned.",
        ),
    ),
    ("sysoverview.native_peer", Message::Simple("Native peer")),
    ("sysoverview.listen", Message::Simple("Listen")),
    ("sysoverview.not_listening", Message::Simple("(not listening)")),
    ("sysoverview.link", Message::Simple("Link (S↔B)")),
    ("sysoverview.share_tree", Message::Simple("Share (tree)")),
    (
        "sysoverview.share_tree_hint",
        Message::Simple("Files a connected device can browse and pull, exposed under this peer's tree"),
    ),
    ("sysoverview.share_disk", Message::Simple("Share (disk)")),
    // --- Rendezvous (§6.5 signaling node) ---
    //
    // The vocabulary here is deliberate. "Rendezvous" rather than "signaling
    // node" because the row answers *what it does for you*, and the on-state
    // string carries the ADDRESS because "it's on" is not something a user can
    // act on — that address is what the other browser types into `connector
    // add`. The off-state says what turning it on would buy, since a person
    // arriving here is asking "why can't my two browsers find each other".
    ("sysoverview.rendezvous", Message::Simple("Rendezvous")),
    (
        "sysoverview.rendezvous_on",
        Message::Simple("Serving — browsers can meet here: {addr}"),
    ),
    (
        "sysoverview.rendezvous_off",
        Message::Simple("Off — browsers cannot use this desktop to find each other"),
    ),
    ("sysoverview.rendezvous_start", Message::Simple("Start serving")),
    ("sysoverview.rendezvous_stop", Message::Simple("Stop serving")),
    (
        "sysoverview.rendezvous_hint",
        Message::Simple(
            "Lets two browsers on this network exchange a connection offer through this desktop, \
             so nobody has to run a server. Restarts the backend, which drops open connections.",
        ),
    ),
    (
        "sysoverview.no_backend",
        Message::Simple("No System backend provisioned."),
    ),
    ("sysoverview.loading_status", Message::Simple("Loading status…")),
    (
        "sysoverview.waiting_link",
        Message::Simple("Waiting for the backend link — devices appear once connected."),
    ),
    (
        "sysoverview.checking_devices",
        Message::Simple("Checking connected devices…"),
    ),
    (
        "sysoverview.devices_error",
        Message::Simple("Couldn't read the backend's devices — the manager link may not be ready yet."),
    ),
    (
        "sysoverview.no_devices",
        Message::Simple("No devices connected to this backend."),
    ),
    ("sysoverview.col_grant", Message::Simple("Grant")),
    ("sysoverview.device_auth", Message::Simple("Device authorizations")),
    ("sysoverview.profile_pull", Message::Simple("Pull only")),
    ("sysoverview.profile_twoway", Message::Simple("Two-way")),
    (
        "sysoverview.grant_backend_hint",
        Message::Simple("Authorized on the backend; the specific scope isn't recorded locally."),
    ),
    // -- knowledge base surface --
    ("kb.new_article", Message::Simple("+ New article")),
    ("kb.back_to_list", Message::Simple("← Back to list")),
    ("kb.body_placeholder", Message::Simple("Markdown body")),
    ("kb.heading_new", Message::Simple("New Article")),
    ("kb.heading_edit", Message::Simple("Edit Article")),
    (
        "kb.empty_on_peer",
        // {button} is the "+ New article" control's own label, passed in by the
        // caller so the sentence can never drift from the button it names.
        Message::Simple("No articles yet on peer {peer}. Click \"{button}\" to create one."),
    ),
    ("kb.article_gone", Message::Simple("The selected article is no longer available.")),
    ("kb.err_title_empty", Message::Simple("Title cannot be empty")),
    (
        "kb.err_title_alnum",
        Message::Simple("Title must contain at least one alphanumeric character"),
    ),
    ("kb.err_no_selection", Message::Simple("No article selected to edit")),
    ("kb.err_not_editable", Message::Simple("Not in editable mode")),
    // -- settings surface (the P1 demonstrators — wired through t()) --
    ("settings.appearance", Message::Simple("Appearance")),
    ("settings.theme", Message::Simple("Theme")),
    ("settings.language", Message::Simple("Language")),
    ("settings.windows", Message::Simple("Windows")),
    ("settings.site_surface", Message::Simple("Site & Surface")),
    ("settings.rendering", Message::Simple("Rendering")),
    ("settings.network", Message::Simple("Network")),
    ("settings.boot_into", Message::Simple("Boot into")),
    ("settings.boot_chrome", Message::Simple("Chrome")),
    ("settings.boot_site", Message::Simple("Site")),
    ("settings.boot_window", Message::Simple("Window")),
    (
        "settings.lockdown_active",
        Message::Simple("Lockdown is active (set by this deployment's config)."),
    ),
    ("settings.site_appearance", Message::Simple("Site appearance")),
    (
        "settings.site_appearance.hint",
        Message::Simple("Controls the in-app site overlay's colors."),
    ),
    (
        "settings.singleton_windows",
        Message::Simple(
            "Single-instance windows: focus an open window instead of opening a duplicate",
        ),
    ),
    (
        "settings.boot_hint",
        Message::Simple(
            "Applies at next launch — use the status-bar toggle to enter Site Mode now.",
        ),
    ),
    (
        "settings.show_toggle",
        Message::Simple("Show the site toggle in the status bar"),
    ),
    ("settings.show_inspector", Message::Simple("Show inspector panel")),
    (
        "settings.auto_connect",
        Message::Simple("Auto-connect to known peers on startup"),
    ),
    ("settings.no_targets", Message::Simple("(none available)")),
    // -- theme editor surface --
    ("theme.themes", Message::Simple("Themes")),
    ("theme.new_from", Message::Simple("New theme from")),
    ("theme.scheme_dark", Message::Simple("Dark")),
    ("theme.scheme_light", Message::Simple("Light")),
    // theme editor card
    (
        "theme.no_theme",
        Message::Simple("No theme loaded — duplicate one above to start."),
    ),
    ("theme.edit_title", Message::Simple("Edit \"{label}\"")),
    (
        "theme.preview_hint",
        Message::Simple("Edits preview live across the whole app. Save keeps them; Revert restores the real theme."),
    ),
    ("theme.scheme", Message::Simple("Scheme")),
    (
        "theme.scheme_hint",
        Message::Simple("Not a color: browsers render native widgets (dropdown popups, scrollbars, carets) in one of exactly two modes, dark or light — pick the one your palette sits closest to so those widgets match."),
    ),
    ("theme.col_token", Message::Simple("Token")),
    ("theme.col_value", Message::Simple("Value")),
    (
        "theme.in_use",
        Message::Simple("This theme is in use (chrome theme or site override) — switch first to delete."),
    ),
    // token group section titles (section_for)
    ("theme.section_fonts", Message::Simple("Fonts")),
    ("theme.section_status", Message::Simple("Status")),
    ("theme.section_peer_badges", Message::Simple("Peer badges")),
    ("theme.section_app_cards", Message::Simple("App cards")),
    ("theme.section_accents", Message::Simple("Accents")),
    ("theme.section_borders", Message::Simple("Borders")),
    ("theme.section_text", Message::Simple("Text")),
    ("theme.section_surfaces", Message::Simple("Surfaces")),
    // status-line outcomes
    ("theme.status_created", Message::Simple("Created \"{name}\"")),
    ("theme.status_saved", Message::Simple("Saved \"{name}\"")),
    ("theme.status_reverted", Message::Simple("Reverted")),
    ("theme.status_deleted", Message::Simple("Deleted \"{name}\"")),
    (
        "theme.status_unknown_base",
        Message::Simple("Unknown base theme \"{base}\""),
    ),
    (
        "settings.language.hint",
        Message::Simple("Sets the interface language and layout direction."),
    ),
    // -- menu category headers + blurbs (WindowCategory label/description) --
    ("category.apps", Message::Simple("Apps & Content")),
    ("category.system", Message::Simple("System")),
    ("category.developer", Message::Simple("Developer")),
    (
        "category.apps.desc",
        Message::Simple("Browse sites, play games, run apps"),
    ),
    (
        "category.system.desc",
        Message::Simple("Peers, keys, connections, settings, storage"),
    ),
    (
        "category.developer.desc",
        Message::Simple("Entity tree, query & execute consoles, shell, logs"),
    ),
    // -- window titles (title bar + menu label, via i18n::window_title) --
    ("window.entity_tree", Message::Simple("Entity Tree")),
    ("window.games", Message::Simple("Games")),
    ("window.apps", Message::Simple("Apps")),
    ("window.knowledge_base", Message::Simple("Knowledge Base")),
    ("window.key_manager", Message::Simple("Key Manager")),
    ("window.peer_connections", Message::Simple("Peer Connections")),
    ("window.file_transfer", Message::Simple("File Transfer")),
    ("window.execute_console", Message::Simple("Execute Console")),
    ("window.query_console", Message::Simple("Query Console")),
    ("window.settings", Message::Simple("Settings")),
    ("window.event_log", Message::Simple("Event Log")),
    ("window.peers", Message::Simple("Peers")),
    ("window.shell", Message::Simple("Shell")),
    ("window.chat", Message::Simple("Chat")),
    ("chat.conversation", Message::Simple("conversation: {id}")),
    ("chat.empty", Message::Simple("No messages yet — say something.")),
    ("chat.placeholder", Message::Simple("Message…")),
    ("chat.start_prompt", Message::Simple("Start a chat with a connected peer:")),
    ("chat.start_hint", Message::Simple("Connect to a peer (Peer Connections) to start a chat.")),
    ("chat.start_by_id_placeholder", Message::Simple("…or paste a peer id, then Enter")),
    ("chat.invalid_peer_id", Message::Simple("Not a valid peer id — paste the peer’s full id.")),
    (
        "chat.no_establisher",
        Message::Simple(
            "This peer can’t be reached back — they can find you, but nothing can \
             connect to you. Switch this window to your main peer.",
        ),
    ),
    // --- Why the network could not carry it (`crate::reachability`) ---
    //
    // Three rules these strings obey, from the design's §3.3:
    //   - never name a specific NAT type — we can observe our own candidate
    //     types, we cannot observe whether the far side is symmetric, and a
    //     confident wrong diagnosis is worse than a vague right one;
    //   - never promise a relay will fix it — it fixes the restrictive-NAT
    //     rows, and it does not fix a friend who closed their laptop, which is
    //     why `no_direct_path` describes the situation instead of predicting;
    //   - never quote a TTL or a retry count.
    // Each names the fix without naming a culprit.
    (
        "chat.reach_no_reflector",
        Message::Simple(
            "No reflector is set up, so this app can only reach devices on your local \
             network. Add one on the connector you rendezvous through.",
        ),
    ),
    (
        "chat.reach_reflector_unreachable",
        Message::Simple(
            "The reflector didn’t answer, so we never learned this device’s address \
             beyond your local network. Check its address, or its operator may be down.",
        ),
    ),
    (
        "chat.reach_no_direct_path",
        Message::Simple(
            "This network needs a relay — neither device can be reached directly. \
             Add a relay on the connector you rendezvous through.",
        ),
    ),
    ("window.chain_trace", Message::Simple("Chain Trace")),
    ("window.path_tap", Message::Simple("Path Tap")),
    ("window.wire_recorder", Message::Simple("Wire Recorder")),
    ("window.content_stream", Message::Simple("Content Stream")),
    ("window.site_browser", Message::Simple("Site Browser")),
    ("window.storage", Message::Simple("Storage")),
    ("window.site_creator", Message::Simple("Site Creator")),
    ("window.system_overview", Message::Simple("System Overview")),
    ("window.access_log", Message::Simple("Access Log")),
    ("window.theme_editor", Message::Simple("Theme Editor")),
    // -- backend authorization profiles (GrantProfile::label / scope_summary
    //    return these keys; callers resolve them) --
    ("backend_auth.profile_file_transfer", Message::Simple("File transfer (pull only)")),
    ("backend_auth.profile_file_transfer_rw", Message::Simple("File transfer (two-way)")),
    ("backend_auth.profile_trusted", Message::Simple("Trusted (full access)")),
    (
        "backend_auth.scope_file_transfer",
        Message::Simple("Can list and read files in the shared folder."),
    ),
    (
        "backend_auth.scope_file_transfer_rw",
        Message::Simple("Can list, read, write, and delete files in the shared folder."),
    ),
    (
        "backend_auth.scope_trusted",
        Message::Simple("Full access — every handler, path, and operation on this backend."),
    ),
    // -- embedded-app launchers (Games / Apps) --
    ("games.empty", Message::Simple("No games available yet.")),
    ("apps.empty", Message::Simple("No apps available yet.")),
    // -- site directory rail --
    ("sitedir.sites_menu", Message::Simple("Sites \u{25be}")),
    ("sitedir.sites", Message::Simple("Sites")),
    ("sitedir.filter_mine", Message::Simple("My")),
    ("sitedir.filter_all", Message::Simple("All")),
    ("sitedir.filter_external", Message::Simple("External")),
    ("sitedir.empty_all", Message::Simple("No sites yet.")),
    ("sitedir.empty_mine", Message::Simple("No sites you own.")),
    ("sitedir.empty_external", Message::Simple("No external sites cached.")),
    ("sitedir.bookmark_add", Message::Simple("Bookmark this site")),
    ("sitedir.bookmark_remove", Message::Simple("Remove bookmark")),
    (
        "sitedir.keep_full",
        Message::Simple("Kept offline (full cache) — click to make manifest-pinned"),
    ),
    (
        "sitedir.keep_pinned",
        Message::Simple("Manifest-pinned — click to keep the full site offline"),
    ),
    // Row subline — provenance of a listed site (lowercase by design: it is a
    // compact metadata line, not a sentence).
    ("sitedir.sub_owned", Message::Simple("owned")),
    ("sitedir.sub_cached", Message::Simple("cached")),
    ("sitedir.sub_cached_from", Message::Simple("cached · {host}")),
    // -- content-site surface --
    ("contentsite.enter_peer", Message::Simple("Enter Peer")),
    ("contentsite.contents_menu", Message::Simple("Contents \u{25be}")),
    ("contentsite.share_link", Message::Simple("Share link \u{1f517}")),
    (
        "contentsite.share_link_hint",
        Message::Simple("Copy a link that re-opens this page in the live entity browser"),
    ),
    // Resolve states. The leading/trailing `_` are markdown emphasis — these
    // strings are rendered through the site's markdown pipeline, so keep them.
    ("contentsite.loading_page", Message::Simple("_Loading the live page…_")),
    (
        "contentsite.err_no_manifest",
        Message::Simple("No site manifest at '{site}' (peer: {peer})."),
    ),
    (
        "contentsite.err_page_not_found",
        Message::Simple("Page '{page}' not found in site '{site}'."),
    ),
    (
        "contentsite.err_unreachable",
        Message::Simple("Couldn't reach peer '{peer}' — no route is registered for it."),
    ),
    (
        "contentsite.offline_page_missing",
        Message::Simple("_This page isn't kept for offline viewing — reconnect to load it._"),
    ),
    (
        "contentsite.offline_source_unreachable",
        Message::Simple("_This site's source is unreachable. Showing its cached outline._"),
    ),
    // -- storage surface --
    (
        "storage.origin_disk",
        Message::Simple("Origin disk (IndexedDB + caches — whole origin)"),
    ),
    (
        "storage.backend_native",
        Message::Simple("System backend — native store"),
    ),
    ("storage.native_sqlite", Message::Simple("Native / SQLite")),
    ("storage.by_path", Message::Simple("By top-level path:")),
    (
        "storage.append_only_hint",
        Message::Simple(
            "Read-only. The content store is append-only — overwriting a path \
             leaves the old value behind; it isn't reclaimed until GC (GUIDE-GC).",
        ),
    ),
    (
        "storage.peer_not_created",
        Message::Simple("Peer not created — {reason}"),
    ),
    ("storage.refresh", Message::Simple("Refresh disk usage")),
    // -- Programs window (compute-program host) --
    ("window.programs", Message::Simple("Entity Native Apps")),
    (
        "programs.empty",
        Message::Simple("No programs available."),
    ),
    (
        "programs.subtitle",
        Message::Simple(
            "Transferable compute programs — authored once (workbench-go), \
             mounted from their descriptors, evaluated by this peer's compute engine.",
        ),
    ),
    ("programs.install", Message::Simple("Install")),
    ("programs.restart", Message::Simple("Restart")),
    ("programs.status_absent", Message::Simple("not installed")),
    ("programs.status_refused", Message::Simple("cannot mount")),
    (
        "programs.status_materializing",
        Message::Simple("installing {done}/{total}…"),
    ),
    ("programs.status_running", Message::Simple("running")),
    ("programs.status_faulted", Message::Simple("faulted")),
    (
        "programs.status_line",
        Message::Simple("{status} · tick {ticks} · {rate}/s"),
    ),
    (
        "programs.display_waiting",
        Message::Simple("waiting for the first frame"),
    ),
    // -- L5 app-host (the stripped ?app-host= boot) — failure surfaces the user
    //    sees INSIDE the sandboxed iframe. Never a blank/frozen frame (D13). The
    //    `{reason}` slot carries the diagnostic detail (which stage / which
    //    shape); the frame is what's translated.
    (
        "apphost.cannot_run",
        Message::Simple("{program} can't run here — {reason}"),
    ),
    (
        "apphost.stopped",
        Message::Simple("{program} stopped — {reason}"),
    ),
    ("storage.no_peers", Message::Simple("(no hosted peers)")),
    ("storage.used_quota", Message::Simple("Used / quota")),
    ("storage.persisted", Message::Simple("Persisted")),
    (
        "storage.persisted_yes",
        Message::Simple("yes (eviction-protected)"),
    ),
    (
        "storage.persisted_no",
        Message::Simple("no (best-effort / evictable)"),
    ),
    ("storage.persisted_unknown", Message::Simple("unknown")),
    // Technical acronyms (SQLite/OPFS/IDB) kept verbatim; prose translated.
    ("storage.on_disk_sqlite", Message::Simple("On-disk (SQLite)")),
    ("storage.content_blobs", Message::Simple("Content-store blobs")),
    ("storage.live_paths", Message::Simple("Live tree paths")),
    (
        "storage.backend_stopped",
        Message::Simple("(backend stopped — live counts unavailable)"),
    ),
    (
        "storage.orphaned_blobs",
        Message::Simple("Superseded / orphaned blobs (approx.)"),
    ),
    ("storage.save_state_paths", Message::Simple("Save-state paths")),
    (
        "storage.no_breakdown",
        Message::Simple("(per-prefix breakdown unavailable on the Worker/OPFS arm)"),
    ),
    // -- developer-tool prose --
    //
    // These windows had their hints and buttons localized in the dev-tools
    // pass; their empty states and attach errors were invisible to the gate
    // (blind spot #4 — they render as innerHTML). Extracting them brings the
    // windows to parity rather than leaving them half-translated.
    ("devtools.no_events", Message::Simple("(no events yet)")),
    ("state.loading", Message::Simple("Loading…")),
    ("entitytree.empty", Message::Simple("(empty tree)")),
    ("accesslog.direction_label", Message::Simple("Direction:")),
    ("peers.alias_placeholder", Message::Simple("alias (optional)")),
    (
        "filetransfer.no_peer_hint",
        Message::Simple(
            "No peer connected. Open {window} and pair a Tori-native backend \
             (scan its QR or connect its {scheme} address), then come back \
             here to transfer files.",
        ),
    ),
    // Peer runtime/storage chips (Peers, System Overview). `IndexedDB` and
    // `OPFS` are product names and stay as written everywhere; the descriptive
    // ones are words and get translated.
    (
        "peerdisplay.runtime_main_thread",
        Message::Simple("main thread"),
    ),
    ("peerdisplay.kind_primary", Message::Simple("primary")),
    ("peerdisplay.kind_local", Message::Simple("local")),
    ("peerdisplay.kind_remote", Message::Simple("remote")),
    ("peerdisplay.runtime_worker", Message::Simple("worker")),
    ("peerdisplay.runtime_native", Message::Simple("native")),
    ("peerdisplay.storage_in_memory", Message::Simple("in-memory")),
    ("peerdisplay.storage_indexeddb", Message::Simple("IndexedDB")),
    ("peerdisplay.storage_opfs", Message::Simple("OPFS")),
    (
        "peerdisplay.storage_native_store",
        Message::Simple("native store"),
    ),
    (
        "inspect.attach_failed",
        Message::Simple("Inspect routing failed to attach on this peer."),
    ),
    (
        "inspect.attach_failed_detail",
        Message::Simple(
            "No facts will arrive. Check tracing logs for the \
             install_inspect_sink error.",
        ),
    ),
    (
        "pathtap.empty",
        Message::Simple(
            "(no dispatch facts yet — trigger an exec, query, put, etc. on \
             this peer)",
        ),
    ),
    (
        "contentstream.hint",
        Message::Simple(
            "Live binding events (entity writes/removes/snapshots) for this \
             peer (newest first; ring buffer).",
        ),
    ),
    (
        "contentstream.empty",
        Message::Simple(
            "(no binding events yet — trigger a put / remove / snapshot on \
             this peer)",
        ),
    ),
    (
        "wirerecorder.hint",
        Message::Simple(
            "Live wire frames for this peer (newest first; ring buffer). Only \
             populates when cross-peer traffic flows.",
        ),
    ),
    (
        "wirerecorder.empty",
        Message::Simple(
            "(no wire frames yet — connect to a remote peer or accept an \
             inbound dial to see traffic)",
        ),
    ),
    (
        "shell.scrollback_cleared",
        Message::Simple("(scrollback cleared)"),
    ),
    ("executeconsole.local", Message::Simple("Local ({peer})")),
    // -- chain trace surface --
    (
        "chaintrace.no_marker",
        Message::Simple(
            "(no continuation or chain-error marker bound for chain_id \
             {chain} on peer {peer})",
        ),
    ),
    (
        "chaintrace.body_redacted",
        Message::Simple("(body redacted per renderer policy)"),
    ),
    (
        "chaintrace.body_undecoded",
        Message::Simple("(body not yet decoded)"),
    ),
    ("chaintrace.chain_id", Message::Simple("Chain ID:")),
    ("chaintrace.continuations", Message::Simple("Continuations")),
    ("chaintrace.error_markers", Message::Simple("Chain-error markers")),
    (
        "chaintrace.enter_hint",
        Message::Simple("(enter a chain_id and press Trace)"),
    ),
    // -- window switcher / desktop chrome --
    ("windows.none_open", Message::Simple("No windows open")),
    // -- key manager / peer connections header / site nav overflow --
    (
        "keymanager.subtitle",
        Message::Simple("Hosted-peer public identities (Ed25519)"),
    ),
    ("peerconn.this_peer", Message::Simple("This peer {pid} · {kind}")),
    ("peerconn.listening", Message::Simple(" · listening {addr}")),
    (
        "peerconn.qr_failed",
        Message::Simple("Failed to generate QR code"),
    ),
    // The outcome of a manual Connect press. All three states are reported —
    // a success that named nobody was as opaque as the silent failure it sat
    // beside (D13). `{reason}` carries the connect future's own message.
    (
        "peerconn.connect_dialing",
        Message::Simple("Connecting to {addr}…"),
    ),
    (
        "peerconn.connect_failed",
        Message::Simple("Couldn't connect to {addr} — {reason}"),
    ),
    ("peerconn.connect_ok", Message::Simple("Connected to {peer}")),
    // The connector registry — signaling nodes this peer may rendezvous
    // through. The same operations the `connector` shell verb exposes.
    ("peerconn.connectors", Message::Simple("Connectors")),
    // Stated at the point of choosing, deliberately: a connector learns who is
    // looking for whom and nothing else — it introduces, then gets out of the
    // way. That property is what makes running a community node safe to offer,
    // and REVIEW-CONNECTIVITY-LAYER-COHERENCE §5.5 asks for it to be said
    // wherever the UI asks a user to pick one.
    (
        "peerconn.connectors_hint",
        Message::Simple(
            "Nodes that introduce peers to each other. Rendezvous only — never in the data path.",
        ),
    ),
    (
        "peerconn.connector_none",
        Message::Simple("No connectors yet — add a signaling node to rendezvous through."),
    ),
    (
        "peerconn.connector_reload_pending",
        Message::Simple(
            "Your connector choice takes effect on reload — this session is still running on the previous one.",
        ),
    ),
    (
        "peerconn.ice_help",
        Message::Simple(
            "Optional. Needed to connect across different networks; leave empty for same-network only.",
        ),
    ),
    (
        "peerconn.relay_help",
        Message::Simple(
            "Optional. Needed only when neither device can be reached directly — \
             the app tells you when that happens. Forwards your traffic, so it is \
             usually rented or self-hosted.",
        ),
    ),
    (
        "peerconn.relay_secret_help",
        Message::Simple(
            "Stored with your settings on this device, unencrypted. Both the \
             username and the credential are required.",
        ),
    ),
    ("peerconn.connector_add", Message::Simple("Add connector")),
    ("peerconn.connector_use", Message::Simple("Use")),
    ("peerconn.connector_in_use", Message::Simple("In use")),
    ("peerconn.connector_check", Message::Simple("Check")),
    (
        "peerconn.connector_serves",
        Message::Simple("{node} serves {endpoint} · lobby {lobby}"),
    ),
    // Meet at a name — the lobby/tag/secret rendezvous modes
    // (`crate::rendezvous`), the localized surface over the `meet` shell verb.
    ("peerconn.meet", Message::Simple("Meet at a name")),
    // Both halves matter and neither is decoration: what rendezvous *is*
    // (introduction, never the data path, never authorization), and what a name
    // is worth — a tag is public by design, and a memorable "secret" is a tag in
    // disguise (EXTENSION-SIGNALING §2.2).
    (
        "peerconn.meet_hint",
        Message::Simple(
            "Find a peer by name instead of by its id. A label is public — anyone who knows \
             it meets. A secret is only as strong as its randomness, and it introduces you; \
             it never grants anything.",
        ),
    ),
    (
        "peerconn.meet_needs_connector",
        Message::Simple("Select a connector first — a meet happens at a signaling node."),
    ),
    // The meet runs, and the peers it finds are real — but they will not be able
    // to reach back, because this peer has no §6.5 establisher (it is installed
    // on the primary peer only, and this window acts as the peer you selected).
    // Worth saying at the moment of meeting: the alternative is a stranger
    // holding an id that silently never connects.
    (
        "peerconn.meet_no_establisher",
        Message::Simple(
            "Heads up: this peer can't be connected back to — peers you meet will \
             find you but won't reach you. Switch this window to your main peer to \
             be reachable.",
        ),
    ),
    ("peerconn.meet_mode", Message::Simple("Meet by")),
    ("peerconn.meet_mode_tag", Message::Simple("Label (public)")),
    ("peerconn.meet_mode_secret", Message::Simple("Secret")),
    ("peerconn.meet_mode_lobby", Message::Simple("Lobby (anyone here)")),
    ("peerconn.meet_start", Message::Simple("Meet")),
    ("peerconn.meet_stop", Message::Simple("Stop")),
    (
        "peerconn.meet_searching",
        Message::Simple("Searching at {mode} via {node} — {polls}/{max}"),
    ),
    // The honest empty result, with the two things that have to be true for a
    // meet to work — otherwise "nobody there" reads as "this is broken".
    (
        "peerconn.meet_none",
        Message::Simple(
            "Nobody else was there. Both sides have to be searching at the same name, \
             through the same connector.",
        ),
    ),
    ("peerconn.meet_unverified", Message::Simple("unverified claim")),
    ("peerconn.meet_remember", Message::Simple("Remember")),
    ("peerconn.meet_remembered", Message::Simple("Remembered")),
    // The caret is part of the affordance, so it lives in the value — that
    // also lets an RTL locale put it on the correct side.
    ("contentsite.more", Message::Simple("More \u{25be} ({n})")),
    ("windows.open_windows", Message::Simple("Open Windows")),
    ("windows.menu", Message::Simple("Menu")),
    (
        "windows.open_windows_hint",
        Message::Simple("— jump to or close your active windows"),
    ),
    // The palette's own group header. A separate key from `open_windows`
    // rather than `format!("{} ({})", …)` so a locale controls where the
    // count sits, and so `t()` bidi-isolates the number for RTL.
    (
        "windows.open_windows_count",
        Message::Simple("Open Windows ({n})"),
    ),
    (
        "windows.welcome_body",
        Message::Simple(
            "Entity Browser is a workspace over your entity tree. \
             Pick a window from the menu to get started.",
        ),
    ),
    (
        "windows.welcome_modes",
        Message::Simple(
            "Sites can open full-screen in Site Mode, and any window — games and \
             apps included — can be maximized to fill the screen. Explore and enjoy.",
        ),
    ),
    // -- storage durability / watchdog / peer-creation refusal --
    //
    // The copy a user reads when something has gone wrong: their tree is not
    // being saved, the UI froze, an action was refused. Least likely to be
    // seen in casual testing; most important to get right in every locale.
    (
        "durability.ephemeral_direct",
        Message::Simple(
            "Direct mode: your entity tree lives in memory only and is lost on \
             reload (your identity is preserved).",
        ),
    ),
    (
        "durability.storage_unavailable",
        Message::Simple(
            "Storage unavailable: background (Worker) storage failed to start, so \
             your entity tree won't be saved this session and any previously saved \
             tree isn't loaded. Try reloading.",
        ),
    ),
    (
        "durability.secondary_tab",
        Message::Simple(
            "This app is already open in another tab, which owns your saved data. \
             Changes in THIS tab are not being saved. Close the other tab and \
             reload here to edit your saved tree.",
        ),
    ),
    (
        "durability.evictable",
        Message::Simple(
            "Your data is saved on this device, but the browser hasn't granted \
             persistent storage — it may be cleared if the device runs low on space, \
             or (on iOS/Safari) after about a week without opening this site. \
             Bookmark or install to Home Screen to make it permanent.",
        ),
    ),
    (
        "watchdog.snag",
        Message::Simple(
            "The app hit a snag and briefly stopped responding. Reload to get back \
             to a clean state — your saved data is kept.",
        ),
    ),
    (
        "peercreate.disabled",
        Message::Simple("peer creation is disabled in this deployment"),
    ),
    (
        "peercreate.cannot_save",
        Message::Simple(
            "this tab can't save — another tab owns your storage, or storage is \
             unavailable. Close the other tab and reload to create peers here.",
        ),
    ),
    (
        "peercreate.log",
        Message::Simple("Cannot create peer: {reason}"),
    ),
    // -- access log surface --
    ("accesslog.operations", Message::Simple("operations: {n}")),
    (
        "accesslog.hint",
        Message::Simple(
            "Access crossing the boundary — → out (you called a peer), ← in \
             (a peer called this device), · local — showing who, the target, the \
             operation, and whether it was allowed or denied (newest first).",
        ),
    ),
    (
        "accesslog.empty_all",
        Message::Simple(
            "No operations yet. Dispatch something — browse a peer, run a shell \
             verb, transfer a file — and it appears here.",
        ),
    ),
    // The two filter labels are slots, not baked copy: they name options the
    // user has to find in the `<select>` above, and those options are
    // themselves localized (`accesslog.all_peers` / `accesslog.filter_all`).
    // Spelling them out here would let the instruction drift from the UI in
    // any of the 30 locales.
    (
        "accesslog.empty_filtered",
        Message::Simple(
            "Nothing for this peer / direction yet. Widen the filters (peer \
             “{peers}”, direction “{all}”) to see every access.",
        ),
    ),
    (
        "accesslog.caps_hint",
        Message::Simple(
            "What each peer has actually done — the minimal grant it would need if \
             enforcement were on. Aggregated from the activity log (this session). \
             Compare against what a peer is authorized for to find the gap. (Resource \
             path is captured on outbound calls only; “—” elsewhere means the path \
             wasn't exposed to the log, not that none was used.)",
        ),
    ),
    (
        "accesslog.caps_empty",
        Message::Simple(
            "No capabilities observed yet. Dispatch something — browse a peer, \
             transfer a file — and each peer's used grants appear here.",
        ),
    ),
    (
        "accesslog.no_grant",
        Message::Simple(
            "No explicit grant recorded — an owned system peer, or a device not \
             yet authorized.",
        ),
    ),
    (
        "accesslog.authorized_head",
        Message::Simple("Authorized: {profile} — {summary}"),
    ),
    // activity table + capability table columns
    ("accesslog.col_actor", Message::Simple("Actor")),
    ("accesslog.col_result", Message::Simple("Result")),
    ("accesslog.col_uses", Message::Simple("Uses")),
    // capability section: "<actor> — N capabilit{y,ies} observed"
    (
        "accesslog.capabilities_observed",
        Message::Plural(&[
            (PluralCategory::One, "{actor} — {n} capability observed"),
            (PluralCategory::Other, "{actor} — {n} capabilities observed"),
        ]),
    ),
    (
        "accesslog.grants_summary",
        Message::Simple("grants — handlers: {handlers} · operations: {operations} · paths: {paths}"),
    ),
    ("accesslog.own_peer", Message::Simple("— (own peer)")),
    ("accesslog.target_local", Message::Simple("(local)")),
    ("accesslog.all_peers", Message::Simple("All peers")),
    ("accesslog.peer_filter_label", Message::Simple("Peer:")),
    // direction tooltips (full words behind the → ← · glyphs)
    (
        "accesslog.dir_outbound",
        Message::Simple("Outbound — this app called a remote peer"),
    ),
    (
        "accesslog.dir_inbound",
        Message::Simple("Inbound — a remote peer called this device"),
    ),
    (
        "accesslog.dir_local",
        Message::Simple("Local — a dispatch on this app's own peer"),
    ),
    // outcome chips
    ("accesslog.result_allowed", Message::Simple("Allowed")),
    ("accesslog.result_denied", Message::Simple("Denied")),
    ("accesslog.result_error", Message::Simple("Error")),
    // direction filter <select> options
    ("accesslog.filter_all", Message::Simple("All")),
    (
        "accesslog.filter_outbound",
        Message::Simple("→ Outbound (you called a peer)"),
    ),
    (
        "accesslog.filter_inbound",
        Message::Simple("← Inbound (a peer called this device)"),
    ),
    (
        "accesslog.filter_local",
        Message::Simple("· Local (this app's own peer)"),
    ),
    // view switcher
    ("accesslog.tab_activity", Message::Simple("Activity (live log)")),
    ("accesslog.tab_capabilities", Message::Simple("Observed capabilities")),
    // system actor display names
    ("accesslog.actor_system_backend", Message::Simple("System backend")),
    ("accesslog.actor_system_peer", Message::Simple("System peer")),
    (
        "mod.learn_more",
        Message::Simple("Learn more and get involved at "),
    ),
    // -- QR scanner surface (dom/scanner.rs) --
    ("scanner.scanned_codes", Message::Simple("Scanned Codes")),
    ("scanner.scanned_codes_count", Message::Simple("Scanned Codes ({n} unique)")),
    ("scanner.photo_capture", Message::Simple("Photo Capture")),
    ("scanner.photo_hint", Message::Simple("Take a photo of a QR code")),
    ("scanner.take_photo", Message::Simple("Take Photo")),
    ("scanner.processing", Message::Simple("Processing…")),
    (
        "scanner.no_detector",
        Message::Simple("No BarcodeDetector — enter code manually"),
    ),
    ("scanner.detector_init_failed", Message::Simple("Detector init failed")),
    ("scanner.detect_call_failed", Message::Simple("Detect call failed")),
    ("scanner.qr_found", Message::Simple("QR code found!")),
    ("scanner.no_qr", Message::Simple("No QR found — try again")),
    ("scanner.decode_error", Message::Simple("Decode error")),
    ("scanner.live_scanner", Message::Simple("Live Scanner")),
    ("scanner.live_hint", Message::Simple("Real-time camera scanning")),
    ("scanner.start_live", Message::Simple("Start Live Scan")),
    ("scanner.stop", Message::Simple("Stop")),
    ("scanner.stopped", Message::Simple("Stopped")),
    ("scanner.camera_unavailable", Message::Simple("Camera not available")),
    ("scanner.camera_denied", Message::Simple("Camera denied")),
    ("scanner.starting_camera", Message::Simple("Starting camera…")),
    ("scanner.stream_error", Message::Simple("Stream error")),
    ("scanner.scanning", Message::Simple("Scanning…")),
    ("scanner.detector_failed", Message::Simple("Detector failed")),
    ("scanner.scan_detect_error", Message::Simple("Scan {n} — detect error")),
    ("scanner.found_continuing", Message::Simple("Found! Continuing scan…")),
    ("scanner.scan_n", Message::Simple("Scan {n}…")),
    ("scanner.scan_error", Message::Simple("Scan {n} — error")),
    // -- count plurals: the status-bar summary ("N windows · M peers · …") --
    (
        "peer.count",
        Message::Plural(&[
            (PluralCategory::One, "{n} peer"),
            (PluralCategory::Other, "{n} peers"),
        ]),
    ),
    (
        "window.count",
        Message::Plural(&[
            (PluralCategory::One, "{n} window"),
            (PluralCategory::Other, "{n} windows"),
        ]),
    ),
    (
        "worker.count",
        Message::Plural(&[
            (PluralCategory::One, "{n} dedicated worker"),
            (PluralCategory::Other, "{n} dedicated workers"),
        ]),
    ),
];

/// `en` catalog as a lookup map, built once. (Linear scan would do at this size;
/// the map keeps `t()` O(1) as the catalog grows toward P4.)
fn en_map() -> &'static HashMap<&'static str, &'static Message> {
    static MAP: OnceLock<HashMap<&'static str, &'static Message>> = OnceLock::new();
    MAP.get_or_init(|| EN.iter().map(|(k, m)| (*k, m)).collect())
}

/// A build-time-embedded overlay locale: a locale id + its `(key, message)`
/// entries, generated by `build.rs` from the JSON files under
/// `I18N_LOCALES_ROOT` (default `locales/`). `en` is **not** here — it is the
/// compiled-in [`EN`] base; these are the *additional* locales that layer over
/// it (a missing key falls back to `en`). The pseudo-locale (`en-XA`) is not
/// embedded either — it transforms `en`'s resolved string.
pub struct EmbeddedLocale {
    pub id: &'static str,
    pub entries: &'static [(&'static str, Message)],
}

// Generated by build.rs at compile time. Defines:
//   pub static EMBEDDED_LOCALES: &[EmbeddedLocale] = &[ … ];
include!(concat!(env!("OUT_DIR"), "/embedded_locales.rs"));

/// The embedded overlay catalogs as `id → (key → &Message)`, built once. Empty
/// on a lean `en`-only build.
fn embedded_map() -> &'static HashMap<&'static str, HashMap<&'static str, &'static Message>> {
    static MAP: OnceLock<HashMap<&'static str, HashMap<&'static str, &'static Message>>> =
        OnceLock::new();
    MAP.get_or_init(|| {
        EMBEDDED_LOCALES
            .iter()
            .map(|loc| (loc.id, loc.entries.iter().map(|(k, m)| (*k, m)).collect()))
            .collect()
    })
}

/// Look up `key` for `locale_id`: the active locale's own overlay catalog wins;
/// a key it doesn't define (a **partial** locale — the common case, since
/// catalogs start as the base vocabulary only) falls back to the compiled-in
/// `en` base. `en` itself and the pseudo-locale have no overlay → straight to
/// `en`. Never returns `None` for a key present in `en` — the D13 "always
/// render, never a raw key" guarantee rides on the `en` completeness.
fn catalog_entry(locale_id: &str, key: &str) -> Option<&'static Message> {
    if let Some(overlay) = embedded_map().get(locale_id) {
        if let Some(msg) = overlay.get(key) {
            return Some(*msg);
        }
    }
    en_map().get(key).copied()
}

/// Slugify a canonical window name into its catalog-key suffix:
/// `"File Transfer"` → `"file_transfer"` (non-alphanumerics → `_`).
fn window_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The localized display title for a window, by its **canonical (English)
/// name**. The one place both the title bar (`WindowView::title`) and the
/// menu/picker label (`window::window_display_name`) resolve through, so they
/// stay in sync (the invariant `window.rs` flags). The canonical name remains
/// the identity key — matching, grouping, and persistence use it verbatim; only
/// the *rendered* title is translated. A name with no `window.<slug>` key
/// renders verbatim (never a raw `window.` key leaks to the user — D13).
pub fn window_title(name: &str) -> String {
    let key = format!("window.{}", window_slug(name));
    match catalog_entry(active_id(), &key) {
        Some(_) => t(&key, &[]),
        None => name.to_string(),
    }
}

thread_local! {
    /// The process-active locale id. WASM is single-threaded (render + apply
    /// share the main thread); on native each test thread carries its own,
    /// which is exactly what per-test isolation wants.
    static ACTIVE_LOCALE: std::cell::RefCell<&'static str> = const { std::cell::RefCell::new(DEFAULT) };
    /// Keys already logged missing — so a gap logs once, not every frame.
    static LOGGED_MISSING: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Monotonic locale generation. Bumped by [`mark_all_dirty`] on every locale
/// apply; the DOM frame loop compares it to its last-seen value and force-
/// rebuilds every open window when it advances (the "dirty all" the per-window
/// `WindowWatch` can't express). Process-global (an atomic), unlike the
/// thread-local active locale, because the one renderer is the sole reader.
static LOCALE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Set the process-active locale (resolved to a known id; unknown → `en`).
pub fn set_active(id: &str) {
    let loc = resolve(id);
    ACTIVE_LOCALE.with(|a| *a.borrow_mut() = loc.id);
}

/// The process-active locale id.
pub fn active_id() -> &'static str {
    ACTIVE_LOCALE.with(|a| *a.borrow())
}

/// The current locale generation — read by the DOM frame loop.
pub fn locale_generation() -> u64 {
    LOCALE_GENERATION.load(Ordering::Relaxed)
}

/// Advance the locale generation, signalling the frame loop to rebuild **every**
/// open window on its next pass. Called by [`apply`]; the reactivity primitive
/// the whole feature rides on (DESIGN §3.2, finding 1).
pub fn mark_all_dirty() {
    LOCALE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Resolve a message key to a localized, interpolated string. Args fill
/// `{named}` slots and are **bidi-isolated by construction** (wrapped in
/// FSI/PDI) so an LTR datum (a peer id, a URI) dropped into an RTL sentence
/// can't reorder the surrounding text — no call site can forget it (DESIGN
/// §3.2, finding 4). A missing key falls back to `en`, and if absent there too
/// logs once and returns the key (never a silent blank — D13). Twin of a
/// `var(--token)` lookup.
pub fn t(key: &str, args: &[(&str, &str)]) -> String {
    let active = active_id();
    let pseudo = resolve(active).pseudo;
    match catalog_entry(active, key) {
        Some(Message::Simple(template)) => render_template(template, args, pseudo),
        Some(Message::Plural(_)) => {
            // A plural key rendered without a count — caller should use
            // `t_plural`. Fall back to the `Other` form so we still show text.
            plural_render(key, PluralCategory::Other, args, pseudo).unwrap_or_else(|| {
                log_missing(key);
                key.to_string()
            })
        }
        None => {
            log_missing(key);
            key.to_string()
        }
    }
}

/// Resolve a plural message key, selecting the CLDR form for `n` in the active
/// locale. Args (typically `("n", n)`) are bidi-isolated like [`t`].
///
/// The plural half of the P1 string seam — unit-tested, with no non-test caller
/// until the P4 extraction routes count strings (`"{n} peers"`) through it.
#[allow(dead_code)] // P1 seam; first consumed at P4 (preserve the public surface).
pub fn t_plural(key: &str, n: i64, args: &[(&str, &str)]) -> String {
    let active = active_id();
    let pseudo = resolve(active).pseudo;
    let cat = plural_category(active, n);
    plural_render(key, cat, args, pseudo).unwrap_or_else(|| {
        log_missing(key);
        key.to_string()
    })
}

/// Select the CLDR **cardinal** plural category for `n` in `locale_id`.
///
/// Rules pinned to the authoritative CLDR source (`unicode-org/cldr-json`
/// `supplemental/plurals.json`, corroborated against the `unicode.org/cldr`
/// chart — ADR-0001 §"Plural resolution"), for the **integer-count scope** the
/// app actually renders: every count is a non-negative integer, so the CLDR
/// operands collapse to `v=t=f=e=0` and `i=n`. Under that scope each language's
/// rule reduces to one of a handful of families:
///
/// - **`other`-only** (no count distinction): `zh` `ja` `ko` `vi` `th` `id`.
/// - **`one`⇔n=1** (+ the `en`-derived pseudo): `en` `es` `de` `it` `nl` `sv`
///   `da` `nb`/`no` `fi` `el` `hu` `tr` `ur`. (CLDR `da`'s `t≠0` branch is
///   fraction-only; several carry a compact-notation `many` we never emit.)
/// - **`one`⇔n∈{0,1}**: `fr` `pt` `hi` `bn` `fa`.
/// - **`he`**: `one`⇔1, `two`⇔2, else `other` (no `many` since CLDR v42).
/// - **`ar`**: the full six — `zero`/`one`/`two` exact, `few`⇔n%100∈3..10,
///   `many`⇔n%100∈11..99, else `other`.
/// - **`ru`/`uk`**: `one`⇔n%10=1∧n%100≠11; `few`⇔n%10∈2..4∧n%100∉12..14; else
///   `many` (`other` is fraction-only, unreachable for integers).
/// - **`pl`**: `one`⇔n=1; `few`⇔n%10∈2..4∧n%100∉12..14; else `many`.
/// - **`cs`**: `one`⇔1, `few`⇔2..4, else `other` (`many` is fraction-only).
/// - **`ro`**: `one`⇔1; `few`⇔n=0∨n%100∈1..19; else `other`.
///
/// An unknown locale falls through to the `en` rule — documented, not silent —
/// until its own rule lands here.
#[allow(dead_code)] // reached via t_plural (the plural seam); live once counts route through it.
fn plural_category(locale_id: &str, n: i64) -> PluralCategory {
    use PluralCategory::*;
    let base = locale_id.split('-').next().unwrap_or(locale_id);
    // CLDR operands under the integer scope; rem_euclid keeps the modulo correct
    // even if a negative ever slips in (counts are non-negative in practice).
    let m10 = n.rem_euclid(10);
    let m100 = n.rem_euclid(100);
    match base {
        // other-only — no grammatical count distinction.
        "zh" | "ja" | "ko" | "vi" | "th" | "id" => Other,
        // one ⇔ n = 1.
        "en" | "es" | "de" | "it" | "nl" | "sv" | "da" | "nb" | "no" | "fi" | "el" | "hu"
        | "tr" | "ur" => {
            if n == 1 {
                One
            } else {
                Other
            }
        }
        // one ⇔ n ∈ {0, 1}.
        "fr" | "pt" | "hi" | "bn" | "fa" => {
            if n == 0 || n == 1 {
                One
            } else {
                Other
            }
        }
        "he" => match n {
            1 => One,
            2 => Two,
            _ => Other,
        },
        "ar" => match n {
            0 => Zero,
            1 => One,
            2 => Two,
            _ if (3..=10).contains(&m100) => Few,
            _ if (11..=99).contains(&m100) => Many,
            _ => Other,
        },
        // East Slavic — one/few/many; `other` is fraction-only (unreachable here).
        "ru" | "uk" => {
            if m10 == 1 && m100 != 11 {
                One
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                Few
            } else {
                Many
            }
        }
        "pl" => {
            if n == 1 {
                One
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                Few
            } else {
                Many
            }
        }
        "cs" => match n {
            1 => One,
            2..=4 => Few,
            _ => Other,
        },
        "ro" => {
            if n == 1 {
                One
            } else if n == 0 || (1..=19).contains(&m100) {
                Few
            } else {
                Other
            }
        }
        _ => {
            if n == 1 {
                One
            } else {
                Other
            }
        }
    }
}

/// Render a plural key's `cat` form (falling back to `Other`, then the first
/// listed form), interpolated. `None` if the key isn't a plural entry.
fn plural_render(
    key: &str,
    cat: PluralCategory,
    args: &[(&str, &str)],
    pseudo: bool,
) -> Option<String> {
    let active = active_id();
    let Some(Message::Plural(forms)) = catalog_entry(active, key) else {
        return None;
    };
    let pick = |c: PluralCategory| forms.iter().find(|(fc, _)| *fc == c).map(|(_, t)| *t);
    let template = pick(cat)
        .or_else(|| pick(PluralCategory::Other))
        .or_else(|| forms.first().map(|(_, t)| *t))?;
    Some(render_template(template, args, pseudo))
}

/// Interpolate `{named}` slots and (for a pseudo-locale) transform the literal
/// text. Args are wrapped in Unicode isolates (FSI `\u{2068}` … PDI `\u{2069}`)
/// so bidi reordering can't leak across them. In pseudo mode the *literal*
/// segments are accented, bracketed (`⟦…⟧`), and padded +~33% — placeholders
/// and their (real, un-accented) arg values pass through untouched, so the
/// pseudo output still reads structurally while flagging un-extracted strings
/// and truncation.
fn render_template(template: &str, args: &[(&str, &str)], pseudo: bool) -> String {
    const FSI: char = '\u{2068}';
    const PDI: char = '\u{2069}';
    let mut out = String::new();
    if pseudo {
        out.push('⟦');
    }
    let mut literal_len = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for nc in chars.by_ref() {
                if nc == '}' {
                    break;
                }
                name.push(nc);
            }
            let val = args
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| *v)
                .unwrap_or("");
            out.push(FSI);
            out.push_str(val);
            out.push(PDI);
        } else {
            out.push(if pseudo { accent(c) } else { c });
            literal_len += 1;
        }
    }
    if pseudo {
        for _ in 0..(literal_len / 3) {
            out.push('·');
        }
        out.push('⟧');
    }
    out
}

/// Accent a Latin letter for the pseudo-locale (leaves everything else alone).
fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ó',
        'u' => 'ú',
        'n' => 'ñ',
        'c' => 'ç',
        's' => 'š',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ó',
        'U' => 'Ú',
        'N' => 'Ñ',
        other => other,
    }
}

/// Log a missing catalog key once (per thread). A gap is a bug to fix, not a
/// crash — never render a raw key silently to the user (D13).
fn log_missing(key: &str) {
    LOGGED_MISSING.with(|set| {
        if set.borrow_mut().insert(key.to_string()) {
            tracing::warn!(key, "i18n: missing catalog key (rendered fallback)");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_is_present_and_ltr() {
        let en = locale("en").expect("en present");
        assert_eq!(en.dir, "ltr");
        assert!(!en.pseudo);
    }

    #[test]
    fn resolve_falls_back_to_en() {
        assert_eq!(resolve("zz-nonexistent").id, "en");
        assert_eq!(resolve("zz-nonexistent").dir, "ltr");
    }

    #[test]
    fn pseudolocale_is_rtl_and_flagged() {
        let xa = locale("en-XA").expect("pseudo present");
        assert_eq!(xa.dir, "rtl");
        assert!(xa.pseudo, "pseudo-locale must be flagged so detect() skips it");
    }

    #[test]
    fn default_is_a_real_locale() {
        let d = locale(DEFAULT).expect("DEFAULT resolves");
        assert!(!d.pseudo, "the default must be a real, selectable locale");
    }

    #[test]
    fn roster_is_well_formed() {
        use std::collections::HashSet;
        let mut ids = HashSet::new();
        for l in LOCALES {
            assert!(ids.insert(l.id), "duplicate roster id: {}", l.id);
            assert!(l.dir == "ltr" || l.dir == "rtl", "{} bad dir '{}'", l.id, l.dir);
            assert!(!l.label.is_empty(), "{} has an empty picker label", l.id);
        }
        // plural_category is total over the roster — every selectable locale
        // resolves a category across a wide range without panicking.
        for l in LOCALES {
            for n in [0i64, 1, 2, 5, 11, 21, 100, 1001] {
                let _ = plural_category(l.id, n);
            }
        }
    }

    // -- P1: catalog + t() -------------------------------------------------

    #[test]
    fn t_resolves_a_seeded_key() {
        set_active("en");
        assert_eq!(t("btn.save", &[]), "Save");
        assert_eq!(t("settings.language", &[]), "Language");
    }

    #[test]
    fn t_missing_key_returns_the_key_not_blank() {
        set_active("en");
        // D13: never a silent blank; the key itself surfaces the gap.
        assert_eq!(t("no.such.key", &[]), "no.such.key");
    }

    #[test]
    fn t_interpolates_and_bidi_isolates_args() {
        set_active("en");
        // en has no interpolated seed key, so exercise the renderer directly.
        let s = render_template("hi {name}!", &[("name", "Ann")], false);
        assert_eq!(s, "hi \u{2068}Ann\u{2069}!", "args must be FSI/PDI-wrapped");
    }

    #[test]
    fn pseudolocale_transforms_literals_but_not_args() {
        set_active("en-XA");
        let s = t("btn.save", &[]);
        assert!(s.starts_with('⟦') && s.ends_with('⟧'), "bracketed: {s:?}");
        assert!(s.contains('š') || s.contains('á'), "accented: {s:?}");
        // Args stay real (un-accented) even in pseudo mode.
        let interp = render_template("x {v}", &[("v", "raw")], true);
        assert!(interp.contains("raw"), "arg value must survive: {interp:?}");
    }

    #[test]
    fn t_plural_selects_by_count() {
        set_active("en");
        assert_eq!(t_plural("peer.count", 1, &[("n", "1")]), "\u{2068}1\u{2069} peer");
        assert_eq!(t_plural("peer.count", 3, &[("n", "3")]), "\u{2068}3\u{2069} peers");
        // zero uses Other in en/es.
        assert_eq!(t_plural("peer.count", 0, &[("n", "0")]), "\u{2068}0\u{2069} peers");
    }

    // -- P4: CLDR cardinal plural vectors (ADR-0001 §"Plural resolution") -----
    //
    // Boundaries pinned to the authoritative CLDR source (unicode.org/cldr v47
    // chart + unicode-org/cldr-json plurals.json, corroborated) — asserted, not
    // trusted to memory. Representative n per category, including the modulo
    // boundaries that are the real correctness test. Integer scope (v=0).

    use PluralCategory::{Few, Many, Other, Two, Zero};

    #[test]
    fn cldr_plural_es_one_other() {
        // es: one ⇔ n=1, else other. (CLDR `many` is compact-notation only.)
        assert_eq!(plural_category("es", 1), PluralCategory::One);
        for n in [0, 2, 3, 11, 100, 1000] {
            assert_eq!(plural_category("es", n), Other, "es n={n}");
        }
    }

    #[test]
    fn cldr_plural_he_one_two_other_no_many() {
        // he: one ⇔ n=1, two ⇔ n=2, else other. Crucially NO `many` — 20/30/100
        // (old memory-rule multiples of ten) are `other`, not `many`.
        assert_eq!(plural_category("he", 1), PluralCategory::One);
        assert_eq!(plural_category("he", 2), Two);
        for n in [0, 3, 10, 17, 20, 30, 100, 1000] {
            assert_eq!(plural_category("he", n), Other, "he n={n}");
        }
    }

    #[test]
    fn cldr_plural_ar_all_six_with_modulo_boundaries() {
        // ar: the six-category worst case — the modulo logic is the real test.
        assert_eq!(plural_category("ar", 0), Zero);
        assert_eq!(plural_category("ar", 1), PluralCategory::One);
        assert_eq!(plural_category("ar", 2), Two);
        // few: n%100 = 3..10 (CLDR examples: 3~10, 103~110, 1003)
        for n in [3, 10, 103, 110, 1003] {
            assert_eq!(plural_category("ar", n), Few, "ar few n={n}");
        }
        // many: n%100 = 11..99 (CLDR examples: 11~26, 111, 1011)
        for n in [11, 26, 99, 111, 1011] {
            assert_eq!(plural_category("ar", n), Many, "ar many n={n}");
        }
        // other: n%100 ∈ {0,1,2} for n≥100 (CLDR examples: 100~102, 200~202, 1000)
        for n in [100, 101, 102, 200, 202, 1000] {
            assert_eq!(plural_category("ar", n), Other, "ar other n={n}");
        }
    }

    #[test]
    fn cldr_plural_family_one_other_variants() {
        use PluralCategory::One;
        // one⇔n=1 family: 0 and 2+ are other.
        for loc in ["de", "it", "nl", "sv", "da", "nb", "no", "fi", "el", "hu", "tr", "ur"] {
            assert_eq!(plural_category(loc, 1), One, "{loc} n=1");
            for n in [0, 2, 5, 11, 21, 100] {
                assert_eq!(plural_category(loc, n), Other, "{loc} n={n}");
            }
        }
        // one⇔n∈{0,1} family (fr/pt/hi/bn/fa): 0 is ALSO one.
        for loc in ["fr", "pt", "hi", "bn", "fa"] {
            assert_eq!(plural_category(loc, 0), One, "{loc} n=0 must be one");
            assert_eq!(plural_category(loc, 1), One, "{loc} n=1");
            for n in [2, 5, 11, 100] {
                assert_eq!(plural_category(loc, n), Other, "{loc} n={n}");
            }
        }
    }

    #[test]
    fn cldr_plural_family_other_only() {
        // No count distinction — every n is other (incl. 1).
        for loc in ["zh", "ja", "ko", "vi", "th", "id"] {
            for n in [0, 1, 2, 5, 11, 100] {
                assert_eq!(plural_category(loc, n), Other, "{loc} n={n}");
            }
        }
    }

    #[test]
    fn cldr_plural_ru_uk_one_few_many() {
        use PluralCategory::One;
        // Pinned to CLDR ru/uk cardinal examples (integer scope; other is
        // fraction-only and unreachable here).
        for loc in ["ru", "uk"] {
            // one: n%10=1 and n%100≠11 (1, 21, 31, 101; NOT 11)
            for n in [1, 21, 31, 101, 1001] {
                assert_eq!(plural_category(loc, n), One, "{loc} one n={n}");
            }
            // few: n%10=2..4 and n%100∉12..14 (2~4, 22~24, 102~104)
            for n in [2, 3, 4, 22, 24, 104] {
                assert_eq!(plural_category(loc, n), Few, "{loc} few n={n}");
            }
            // many: n%10=0, or n%10=5..9, or n%100=11..14 (0,5~9,11~14,25~29,111)
            for n in [0, 5, 9, 11, 12, 13, 14, 25, 100, 111] {
                assert_eq!(plural_category(loc, n), Many, "{loc} many n={n}");
            }
        }
    }

    #[test]
    fn cldr_plural_pl_differs_from_ru_at_teens_and_ones() {
        use PluralCategory::One;
        // pl: one is ONLY n=1 (not 21/31 — that's where it diverges from ru).
        assert_eq!(plural_category("pl", 1), One);
        assert_eq!(plural_category("pl", 21), Many, "pl 21 is many, not one (≠ ru)");
        // few: n%10=2..4 and n%100∉12..14
        for n in [2, 3, 4, 22, 23, 24] {
            assert_eq!(plural_category("pl", n), Few, "pl few n={n}");
        }
        // many: everything else (0, 5~21, 25, teens…)
        for n in [0, 5, 11, 12, 14, 25, 111] {
            assert_eq!(plural_category("pl", n), Many, "pl many n={n}");
        }
    }

    #[test]
    fn cldr_plural_cs_and_ro() {
        use PluralCategory::One;
        // cs: one⇔1, few⇔2..4, else other (many is fraction-only).
        assert_eq!(plural_category("cs", 1), One);
        for n in [2, 3, 4] {
            assert_eq!(plural_category("cs", n), Few, "cs few n={n}");
        }
        for n in [0, 5, 11, 22, 100] {
            assert_eq!(plural_category("cs", n), Other, "cs other n={n}");
        }
        // ro: one⇔1, few⇔n=0 or n%100∈1..19, else other.
        assert_eq!(plural_category("ro", 1), One);
        for n in [0, 2, 12, 19, 101, 119] {
            assert_eq!(plural_category("ro", n), Few, "ro few n={n}");
        }
        for n in [20, 21, 99, 100, 120] {
            assert_eq!(plural_category("ro", n), Other, "ro other n={n}");
        }
    }

    #[test]
    fn unknown_locale_falls_back_to_en_rule() {
        // Documented fallback (not silent): an un-modeled locale uses en's rule.
        assert_eq!(plural_category("zz", 1), PluralCategory::One);
        assert_eq!(plural_category("zz", 5), Other);
    }

    // -- P4: the embedded overlay catalogs (loader) ------------------------

    #[test]
    fn embedded_locales_have_no_orphan_keys_and_matching_shape() {
        // Subset-VALIDITY, not completeness: an overlay may be partial (missing
        // keys fall back to `en` by design — the "partial → English" policy).
        // But every key it DOES define must exist in `en` (else it's a
        // typo/orphan that silently never renders), and its Simple/Plural shape
        // must match `en` so the selector behaves. The i18n analog of theming's
        // key-parity test, deliberately weakened: a missing string is a safe en
        // fallback; a missing color is a broken render.
        for loc in EMBEDDED_LOCALES {
            for (key, msg) in loc.entries {
                let en_msg = en_map().get(key).copied().unwrap_or_else(|| {
                    panic!(
                        "overlay locale '{}' defines key '{key}' not in the en \
                         base catalog (orphan/typo)",
                        loc.id
                    )
                });
                assert_eq!(
                    matches!(en_msg, Message::Plural(_)),
                    matches!(msg, Message::Plural(_)),
                    "overlay '{}' key '{key}': Simple/Plural shape differs from en",
                    loc.id
                );
            }
        }
    }

    #[test]
    fn overlay_locale_is_consulted() {
        // Each overlay Simple entry must resolve to ITS OWN template through the
        // full `t()` path — proves `catalog_entry` consults the overlay, not en.
        for loc in EMBEDDED_LOCALES {
            for (key, msg) in loc.entries {
                if let Message::Simple(tpl) = msg {
                    set_active(loc.id);
                    assert_eq!(
                        t(key, &[]),
                        render_template(tpl, &[], false),
                        "overlay '{}' key '{key}' must resolve to its own template",
                        loc.id
                    );
                }
            }
        }
        set_active("en");
    }

    #[test]
    fn overlay_ar_plural_selects_distinct_forms() {
        // The ar worst case, integrated: Arabic peer.count carries all six CLDR
        // forms, and the selector must pick DISTINCT templates across categories
        // *through the overlay* (few ≠ many ≠ other). Guards the plural × overlay
        // × selector wiring. Skipped on a lean en-only build (no ar overlay →
        // en's one/other fallback can't distinguish the categories).
        if !EMBEDDED_LOCALES.iter().any(|l| l.id == "ar") {
            return;
        }
        set_active("ar");
        let few = t_plural("peer.count", 3, &[("n", "3")]); // n%100=3 → few
        let many = t_plural("peer.count", 11, &[("n", "11")]); // n%100=11 → many
        let other = t_plural("peer.count", 100, &[("n", "100")]); // → other
        set_active("en");
        assert_ne!(few, many, "ar few vs many must select different forms");
        assert_ne!(many, other, "ar many vs other must select different forms");
        assert!(few.contains('3'), "the count arg must render: {few:?}");
    }

    #[test]
    fn window_title_maps_and_falls_back() {
        set_active("en");
        assert_eq!(window_title("File Transfer"), "File Transfer");
        assert_eq!(window_title("System Overview"), "System Overview");
        // An unkeyed name renders verbatim — never a raw `window.<slug>` key.
        let unknown = window_title("Totally Unknown Window");
        assert_eq!(unknown, "Totally Unknown Window");
        assert!(!unknown.starts_with("window."), "raw key leaked: {unknown}");
    }

    #[test]
    fn every_registered_window_has_a_title_key() {
        // Parity guard: adding a window without a `window.<slug>` en key would
        // silently fall back to the verbatim name (untranslatable). Catch it.
        for wt in crate::window_registry::standard_window_types() {
            let key = format!("window.{}", window_slug(wt.name));
            assert!(
                en_map().contains_key(key.as_str()),
                "window '{}' has no '{key}' key in the EN catalog",
                wt.name
            );
        }
    }

    #[test]
    fn plural_forms_cover_every_category_the_selector_can_produce() {
        // `tools/i18n_locale_check.py` checks that a locale's plural keys agree
        // with **its own `peer.count`** — self-referential, the same shape as
        // the `es`-as-reference bug. If `peer.count` itself carries the wrong
        // category set for a locale, every other plural key agrees with the
        // wrong set and the gate reports clean. The non-circular reference is
        // the CLDR-pinned selector itself.
        //
        // A gap is not a crash: `plural_render` degrades cat -> Other -> first
        // form. It renders the WRONG grammatical form silently, which is
        // exactly the kind of thing no reviewer notices and no test catches.
        for loc in EMBEDDED_LOCALES {
            // Categories actually reachable for integer counts. 0..=200 covers
            // every modulo-100 branch (ar's 3..10 / 11..99, the Slavic
            // 12..14 exclusions, ro's 1..19).
            let mut reachable: Vec<PluralCategory> = Vec::new();
            for n in 0..=200 {
                let c = plural_category(loc.id, n);
                if !reachable.contains(&c) {
                    reachable.push(c);
                }
            }

            for (key, msg) in loc.entries {
                let Message::Plural(forms) = msg else {
                    continue;
                };
                let have: Vec<PluralCategory> = forms.iter().map(|(c, _)| *c).collect();
                let missing: Vec<_> =
                    reachable.iter().filter(|c| !have.contains(c)).collect();
                assert!(
                    missing.is_empty(),
                    "locale '{}' key '{key}': no form for {missing:?}, which the \
                     CLDR selector produces for some count — renders the wrong \
                     grammatical form via the Other fallback. Has {have:?}",
                    loc.id
                );
                let dead: Vec<_> =
                    have.iter().filter(|c| !reachable.contains(c)).collect();
                assert!(
                    dead.is_empty(),
                    "locale '{}' key '{key}': form(s) {dead:?} can never be \
                     selected for an integer count — dead translation. \
                     Reachable: {reachable:?}",
                    loc.id
                );
            }
        }
    }

    #[test]
    fn every_embedded_locale_is_pickable_and_dir_is_right() {
        // A catalog that ships without a LOCALES row is embedded but
        // unreachable — the user can never select it. The reverse (a roster row
        // with no catalog) silently renders English for that language.
        for loc in EMBEDDED_LOCALES {
            let row = locale(loc.id).unwrap_or_else(|| {
                panic!(
                    "embedded catalog '{}' has no LOCALES row — it ships in the \
                     binary but cannot be selected in Settings",
                    loc.id
                )
            });
            assert!(!row.pseudo, "'{}' is a real catalog, not a pseudo-locale", loc.id);
        }
        // The four RTL languages, and only those, carry dir=rtl.
        const RTL: [&str; 4] = ["ar", "he", "fa", "ur"];
        for row in LOCALES {
            let want = if RTL.contains(&row.id) || row.pseudo { "rtl" } else { "ltr" };
            assert_eq!(
                row.dir, want,
                "locale '{}' has dir={} — expected {want}",
                row.id, row.dir
            );
        }
    }

    #[test]
    fn every_window_title_key_has_a_registered_window() {
        // The reverse of `every_registered_window_has_a_title_key`, and the
        // direction nothing covered: renaming or removing a window leaves its
        // old `window.<slug>` key behind, rendering nowhere while all 30
        // overlays still carry a translation for it. Neither the catalog gate
        // (which compares overlays to EN) nor the prose scanner can see that —
        // the key exists and is "referenced" by the `format!("window.{}")`
        // lookup, so it looks live from every angle except this one.
        let registered: std::collections::HashSet<String> =
            crate::window_registry::standard_window_types()
                .iter()
                .map(|wt| window_slug(wt.name))
                .collect();
        for (key, _) in EN {
            let Some(slug) = key.strip_prefix("window.") else {
                continue;
            };
            // `window.count` is the open-window plural, not a title.
            if slug == "count" {
                continue;
            }
            assert!(
                registered.contains(slug),
                "EN key '{key}' has no registered window — a stale title key \
                 from a rename/removal, still translated in all 30 overlays"
            );
        }
    }

    #[test]
    fn overlay_missing_key_falls_back_to_en() {
        // The user's "partial → English" policy: a key an overlay lacks renders
        // the `en` template verbatim (not the raw key, not a blank).
        for loc in EMBEDDED_LOCALES {
            let overlay: std::collections::HashSet<&str> =
                loc.entries.iter().map(|(k, _)| *k).collect();
            let fallback_key = EN
                .iter()
                .filter(|(k, m)| matches!(m, Message::Simple(_)) && !overlay.contains(k))
                .map(|(k, _)| *k)
                .next();
            if let Some(key) = fallback_key {
                set_active(loc.id);
                let via_locale = t(key, &[]);
                set_active("en");
                let via_en = t(key, &[]);
                assert_eq!(
                    via_locale, via_en,
                    "overlay '{}' lacks '{key}' → must render the en fallback verbatim",
                    loc.id
                );
            }
        }
        set_active("en");
    }

    #[test]
    fn apply_bumps_locale_generation() {
        let before = locale_generation();
        apply("en-XA");
        assert!(locale_generation() > before, "apply must advance the generation");
        assert_eq!(active_id(), "en-XA");
        apply("en"); // restore for other tests on this thread
    }
}
