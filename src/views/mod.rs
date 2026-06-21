//! Window view implementations.

pub mod chain_trace;
pub mod content_site;
pub mod content_stream;
pub mod entity_tree;
pub mod path_tap;
pub mod event_log;
pub mod execute_console;
pub mod games;
pub mod key_manager;
pub mod knowledge_base;
pub mod peer_connections;
pub mod peer_management;
pub mod query_console;
pub mod settings;
pub mod shell;
pub mod site_editor;
pub mod storage;
pub mod wire_recorder;

/// Shorten a peer ID for display.
#[allow(dead_code)]
pub fn short_pid(pid: &str) -> String {
    if pid.len() > 16 {
        format!("{}...{}", &pid[..8], &pid[pid.len()-6..])
    } else {
        pid.to_string()
    }
}

/// User-facing peer identity: the peer's metadata `label` (the alias
/// set at create time) when present, else the truncated peer-id.
/// This is what windows/panels show so a peer can be tracked by name
/// instead of by comparing hash prefixes. The label is the
/// already-system-backed `PeerMetadata.label` — not a UI-only shadow.
#[allow(dead_code)]
pub fn display_name(peers: &crate::peers::Peers, pid: &str) -> String {
    peers
        .peer_metadata(pid)
        .and_then(|m| m.label)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| short_pid(pid))
}

/// Semantic category for one event-log message — used by the renderer
/// to color-code entries. Pre-classifying in the model keeps the
/// renderer pure (no string heuristics in DOM code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum EventCategory {
    /// Success: connections established, OK results, remote types fetched.
    Success,
    /// Failure: errors, FAIL markers, "failed" substring.
    Failure,
    /// Info: in-flight operations (Connecting, Listening, Fetching).
    Info,
    /// Anything else.
    Neutral,
}

impl EventCategory {
    /// Classify a raw event-log message into a semantic category.
    /// Same heuristics that were previously inlined in event_log,
    /// execute_console, and query_console DOM renderers.
    #[allow(dead_code)]
    pub fn classify(msg: &str) -> Self {
        if msg.starts_with('\u{2190}') // ←
            || msg.starts_with("Connected")
            || msg.starts_with("Remote types")
            || msg.contains(" OK:")
        {
            Self::Success
        } else if msg.starts_with('\u{2717}') // ✗
            || msg.contains("FAIL")
            || msg.contains("failed")
            || msg.contains("error")
        {
            Self::Failure
        } else if msg.starts_with('\u{2192}') // →
            || msg.starts_with("Connecting")
            || msg.starts_with("Listening")
            || msg.starts_with("Fetching")
        {
            Self::Info
        } else {
            Self::Neutral
        }
    }
}

/// One pre-classified event-log entry ready for rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EventEntry {
    pub message: String,
    pub category: EventCategory,
}
