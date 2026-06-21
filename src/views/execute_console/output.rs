//! Renderer-neutral output for the Execute Console window.

#![allow(dead_code)]

use crate::views::EventEntry;

#[derive(Debug, Clone)]
pub struct ExecuteConsoleOutput {
    /// The window's bound local peer. The execute dispatches against
    /// this peer's SDK; a bare `handler_uri` runs against its tree, an
    /// `entity://{remote}/...` URI resolves through its connection
    /// pool. Not necessarily the primary (palette-bound consoles).
    pub peer_id: String,
    pub mode: ExecuteMode,
    /// Peer selector options (pre-built with `selected` flag).
    pub peer_options: Vec<PeerOption>,
    /// Discovered handlers for the active peer (Guided mode only).
    /// In Raw mode this is empty.
    pub guided: Option<GuidedView>,
    /// Raw-mode form values.
    pub raw: Option<RawView>,
    /// Initial value of the resource input.
    pub resource_initial: String,
    /// Pre-resolved (handler_uri, operation) for the Execute click —
    /// the renderer falls back to these if Raw mode's input fields
    /// can't be read.
    pub resolved: ResolvedExecute,
    /// Result log (shared event log, pre-classified).
    pub events: Vec<EventEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecuteMode {
    Guided,
    Raw,
}

#[derive(Debug, Clone)]
pub struct PeerOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct GuidedView {
    pub handlers: Vec<HandlerOption>,
    /// Operations for the currently-selected handler. Empty if no
    /// handlers are available.
    pub operations: Vec<OperationOption>,
}

#[derive(Debug, Clone)]
pub struct HandlerOption {
    pub index: usize,
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct OperationOption {
    pub index: usize,
    pub name: String,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct RawView {
    pub handler_uri_initial: String,
    pub operation_initial: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedExecute {
    pub handler_uri: String,
    pub operation: String,
}
