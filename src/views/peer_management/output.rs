//! Renderer-neutral output for the Peer Management window.

#![allow(dead_code)]

use crate::peer_display::{PeerDescriptor, PeerDisplay};

/// One selectable create-peer mode + whether it's creatable in this runtime.
/// Unsupported modes are rendered disabled with `reason` appended, so a config
/// we can't build "understands that" (the operator's ask) rather than vanishing.
#[derive(Debug, Clone)]
pub struct CreateOption {
    /// Option value = `PeerMode::persist_key`, or `"native"` — durable contract,
    /// what the e2e drives by; never localize this.
    pub value: &'static str,
    /// i18n key for the "where · persistence" label (S6); resolved via `t()`
    /// at the DOM boundary.
    pub label: &'static str,
    /// Creatable in this runtime?
    pub available: bool,
    /// i18n key for the "why not" reason (appended to the label) when
    /// `!available`; `None` when available.
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct PeerManagementOutput {
    pub rows: Vec<PeerRow>,
    /// Show the peer-create panel (alias input + the three `+ …` buttons).
    /// `false` when the deployment's capability posture disables peer creation
    /// (`session_config.peer_creation_enabled` — 1b / MAP §10): a kiosk hides
    /// the affordance entirely, defense-in-depth with the action guard.
    pub show_peer_create: bool,
    /// The create-peer mode options, each with its availability in THIS runtime
    /// (system-aware): unsupported modes are shown **disabled with a reason**,
    /// not hidden, so the operator sees *why* a config isn't offered.
    pub create_options: Vec<CreateOption>,
    /// Whether the collapsible create-peer card is expanded (S8 create
    /// affordance). Model-held so it survives a snapshot rebuild; toggled by the
    /// `collapsible_header`, reset to `false` after a successful Add.
    pub create_open: bool,
    /// Total peer count, surfaced in the footer.
    pub total_count: usize,
    /// Number of hosted SDKs (1 in pure Direct or Worker boot;
    /// grows as backend-mode peers spawn additional Worker SDKs).
    /// Surfaced in the footer so multi-SDK boot state is visible.
    pub sdk_count: usize,
}

#[derive(Debug, Clone)]
pub struct PeerRow {
    pub peer_id: String,
    pub short_pid: String,
    /// Structural classification (which SDK hosts it) — drives badge color.
    pub kind: PeerDisplay,
    /// The truthful role · runtime · storage facets, replacing the old single
    /// "backend (memory)"-style role string. Rendered as a glyph + chips.
    pub descriptor: PeerDescriptor,
    pub label: Option<String>,
    pub persisted: bool,
    pub address: AddressDisplay,
    /// Show "Tree" button — peer has a local PeerContext.
    pub show_open_tree: bool,
    /// Backend peer Start/Stop control. `None` for non-backend peers.
    pub backend_button: Option<BackendButton>,
    /// Show "Delete" button (non-primary peers).
    pub show_delete: bool,
}

#[derive(Debug, Clone)]
pub enum AddressDisplay {
    /// No addresses configured (non-backend peers).
    None,
    /// Backend peer that's not currently listening.
    Stopped,
    /// Backend peer with one or more listen addresses.
    Addresses(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendButton {
    Start,
    Stop,
}
