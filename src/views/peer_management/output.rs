//! Renderer-neutral output for the Peer Management window.

#![allow(dead_code)]

use crate::peer_display::PeerDisplay;

#[derive(Debug, Clone)]
pub struct PeerManagementOutput {
    pub rows: Vec<PeerRow>,
    /// Show the peer-create panel (alias input + the three `+ …` buttons).
    /// `false` when the deployment's capability posture disables peer creation
    /// (`session_config.peer_creation_enabled` — 1b / MAP §10): a kiosk hides
    /// the affordance entirely, defense-in-depth with the action guard.
    pub show_peer_create: bool,
    /// Show the "New Backend Peer" button (only when running in Tauri).
    pub show_backend_create: bool,
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
    pub kind: PeerDisplay,
    /// Role/type glyph (★ system · ● frontend · ◆ backend-mem · ◆⛁ backend-opfs).
    /// Resolved truthfully from the peer's actual mode.
    pub role_glyph: String,
    /// Human role name, paired with `role_glyph`.
    pub role_name: String,
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
