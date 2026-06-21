//! Renderer-neutral output types for the Settings window.
//!
//! Settings has a flat form layout with three sections (Appearance,
//! Rendering, Network). The output captures everything needed to draw
//! the form; section organization (h3 headings, layout) lives in the
//! renderer.

#![allow(dead_code)]

use crate::window::WindowId;

/// Top-level output for one render pass of the Settings window.
#[derive(Debug, Clone)]
pub struct SettingsOutput {
    /// Used to scope per-window radio-group names so checked state in
    /// one Settings window doesn't bleed into another.
    pub window_id: WindowId,
    /// Tree path the state lives at — surfaced in the footer for
    /// transparency.
    pub state_path: String,
    /// Theme dropdown options (pre-flagged with `selected`).
    pub themes: Vec<ThemeOption>,
    /// "Site appearance" dropdown options — how the Content Site overlay is
    /// themed (its own theme / match system / a strict per-theme override).
    pub site_appearance: Vec<SiteAppearanceOption>,
    /// Whether the inspector panel should be shown.
    pub show_inspector: bool,
    /// Whether the app should auto-connect to known peers on startup.
    pub auto_connect: bool,
    /// Whether single-instance ("immutable") windows are enabled.
    pub singleton_windows: bool,
    /// Site & Surface settings, read from the session config entity.
    pub session: SessionSettings,
}

#[derive(Debug, Clone)]
pub struct ThemeOption {
    pub value: &'static str,
    pub label: &'static str,
    pub selected: bool,
}

/// One "Site appearance" option. Unlike [`ThemeOption`] the labels are computed
/// (e.g. `"Always Light"`), so these are owned `String`s.
#[derive(Debug, Clone)]
pub struct SiteAppearanceOption {
    /// The stored value: `"site"` / `"system"` / a registered theme name.
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// The "Site & Surface" settings section — a view onto the
/// [`SessionConfig`](crate::session_config) spine (§5). The startup surface is a
/// **(peer, kind, target)** triple: which peer, what kind of surface (Chrome /
/// Site / Window), and which target on that peer. One row, declarative — pick
/// it, it's stored, boot honors it (handoff §5).
#[derive(Debug, Clone)]
pub struct SessionSettings {
    /// Profile preset options (pre-flagged with `selected`).
    pub profiles: Vec<ProfileOption>,
    /// The boot-surface kind discriminant: `"chrome"` / `"site"` / `"window"`.
    /// Drives the radio group and which target list is shown.
    pub boot_kind: &'static str,
    /// The peer dropdown — every reachable peer, the configured boot target
    /// pre-`selected`, default the system peer. Mirrors the command palette.
    pub peers: Vec<PeerOption>,
    /// The contextual target dropdown: site ids (kind=Site) or scope-filtered
    /// window-type names (kind=Window). Empty + `target_disabled` for Chrome.
    pub targets: Vec<TargetOption>,
    /// Chrome has no target → the target dropdown is disabled.
    pub target_disabled: bool,
    /// Whether the chrome ↔ site status-bar toggle is shown.
    pub show_toggle: bool,
    /// Whether Phase-1 fast paint is enabled (cut 2c): paint the remote home
    /// over HTTP while the peer boots. User-flippable kill switch.
    pub fast_paint: bool,
    /// Whether the overlay is locked (lockdown posture). Read-only in the UI
    /// for now — a **held seam** (§4-C "hold the seam, defer the feature"):
    /// surfaced so it's visible, but no UI control flips it yet.
    pub locked: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileOption {
    pub value: &'static str,
    pub label: &'static str,
    pub selected: bool,
}

/// One peer in the startup-target dropdown.
#[derive(Debug, Clone)]
pub struct PeerOption {
    /// The peer id (the `<option value>` and what's persisted).
    pub id: String,
    /// Human label — display name + role glyph, as in the palette.
    pub label: String,
    pub selected: bool,
}

/// One target option (a site id or a window-type name) for the current kind.
#[derive(Debug, Clone)]
pub struct TargetOption {
    /// The stored value (site id or window-type name).
    pub value: String,
    /// Display label (today same as `value`).
    pub label: String,
    pub selected: bool,
}
