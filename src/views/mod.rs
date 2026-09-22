//! Window view implementations.

pub mod access_log;
pub mod chain_trace;
pub mod chat;
pub mod content_site;
pub mod content_stream;
pub mod entity_tree;
pub mod path_tap;
pub mod feed;
pub mod event_log;
pub mod execute_console;
pub mod file_transfer;
pub mod games;
pub mod key_manager;
pub mod files;
pub mod knowledge_base;
pub mod peer_connections;
pub mod peer_management;
pub mod programs;
pub mod query_console;
pub mod registry_browser;
pub mod settings;
pub mod shell;
pub mod site_editor;
pub mod storage;
pub mod system_monitor;
pub mod system_overview;
pub mod system_peers;
pub mod theme_editor;
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

/// A Unix-ms timestamp as `YYYY-MM-DD` (UTC).
///
/// Exists for `GUIDE-SERVING-MODE` §8's third state, which is
/// *"Verified as of {published_at}"* — **with the date, always**. Bare
/// *"Verified"* is read by every user as *"this is current"*, which is the one
/// claim a signed root cannot support: a publisher who has not republished and
/// an origin withholding a newer root are byte-identical at our end. The date is
/// what lets a person notice a stale site, and against a withholding origin it is
/// the only detection mechanism that exists.
///
/// **Day resolution, deliberately, and UTC.** `published_at` is a property of the
/// *artifact*, not of our fetch, so minute precision would imply a freshness we
/// cannot offer — the same trap §8 names when it forbids *"last checked N minutes
/// ago"*. Local-time conversion is not attempted: the timestamp is the
/// publisher's, and shifting it into the reader's zone invents precision about
/// someone else's clock.
///
/// Dependency-free (Howard Hinnant's `civil_from_days`) rather than pulling a
/// date crate into the wasm bundle for one label.
pub fn format_day(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    // Shift the era so March is month 1 and the leap day lands at the end.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
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
            || msg.starts_with("Connected") // i18n-ignore — log-prefix match key, not UI text
            || msg.starts_with("Remote types") // i18n-ignore — log-prefix match key, not UI text
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
            || msg.starts_with("Connecting") // i18n-ignore — log-prefix match key, not UI text
            || msg.starts_with("Listening") // i18n-ignore — log-prefix match key, not UI text
            || msg.starts_with("Fetching") // i18n-ignore — log-prefix match key, not UI text
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

#[cfg(test)]
mod format_day_tests {
    use super::format_day;

    /// `format_day` is the instrument for `GUIDE-SERVING-MODE` §8's third state,
    /// so it is pinned against known epochs rather than trusted.
    ///
    /// The era arithmetic is the part worth testing: it shifts the year to start
    /// in March so the leap day falls at the end, which makes February 29 and the
    /// century rules (2000 is a leap year, 1900 and 2100 are not) the cases most
    /// likely to be wrong and least likely to be noticed in a UI label.
    #[test]
    fn a_published_at_renders_as_a_utc_day() {
        // Anchors: the epoch, and a timestamp already used as a fixture in
        // `content_site::cache`.
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(format_day(1_700_000_000_123), "2023-11-14");

        // Sub-day precision is DISCARDED, not rounded: the label is a day, and a
        // millisecond before midnight is still that day.
        assert_eq!(format_day(86_400_000 - 1), "1970-01-01");
        assert_eq!(format_day(86_400_000), "1970-01-02");

        // Leap-year handling, where the era shift earns its keep.
        assert_eq!(format_day(951_782_400_000), "2000-02-29"); // a leap day
        assert_eq!(format_day(951_868_800_000), "2000-03-01"); // the day after
        assert_eq!(format_day(4_107_542_400_000), "2100-03-01"); // 2100 is NOT a leap year
    }
}
