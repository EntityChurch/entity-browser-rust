//! Renderer-neutral output for the File Transfer window.
//!
//! Slice 0 (DESIGN-CROSS-DEVICE-FILE-TRANSFER §7): the window dispatches
//! `local/files` list/read against a connected remote peer's exposed
//! `local/files/shared/` root and surfaces the results. Dynamic browsing,
//! byte-level Save-As, and the reverse (push) direction are later phases.

#![allow(dead_code)]

use crate::views::EventEntry;

#[derive(Debug, Clone)]
pub struct FileTransferOutput {
    /// The bound local peer the execute dispatches originate from. A
    /// remote target is reached as `entity://{target}/local/files` through
    /// this peer's connection pool — never via the local peer registry
    /// (remote peers aren't registered locally).
    pub peer_id: String,
    /// Connected remote peers that can be transfer targets, pre-built with
    /// the `selected` flag. Empty when nothing is connected yet.
    pub target_options: Vec<TargetOption>,
    /// The currently-selected target peer id (empty when none connected).
    pub selected_target: String,
    /// Tree prefix the peer exposes its share at (matches the backend's
    /// `SHARE_PREFIX`). Rendered read-only so the demo is legible.
    pub share_prefix: String,
    /// Initial value of the filename input (defaults to the seeded file).
    pub filename_initial: String,
    /// True when at least one remote peer is connected — drives the
    /// role-aware hint ("connect a backend peer first" vs. the controls).
    pub has_target: bool,
    /// Result log (shared event log, pre-classified) — where list/read
    /// responses surface.
    pub events: Vec<EventEntry>,
}

#[derive(Debug, Clone)]
pub struct TargetOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}
