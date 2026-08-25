//! In-process ring buffer of the backend peer's `tracing` output, so S can
//! stream B's native logs *inside* Tori — not just the terminal
//! (`DESIGN-SYSTEM-BACKEND-PEER.md` §2, §4). This is the live-console transport
//! (§7 chose IPC over a tree-backed log for now: simple, low overhead, in-memory
//! only). The WebView polls [`tail`] via the `backend_log_tail` IPC command and
//! renders the lines in the System Backend window.
//!
//! Wiring: [`make_writer`] returns a [`std::io::Write`] that tees each `tracing`
//! event into this ring *and* stdout, installed as the fmt subscriber's writer
//! in `run()`. Capped at [`CAP`] lines (a ring — oldest drop first) so a chatty
//! backend can't grow memory without bound.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, reload, EnvFilter, Registry};

/// Max buffered lines. A ring — the newest [`CAP`] survive; older drop. Sized so
/// a live tail has useful scrollback without unbounded growth on a chatty peer.
const CAP: usize = 4000;

/// One buffered log line with its monotonic sequence number. `seq` is the
/// cursor the client advances past; it never repeats or resets for the process
/// lifetime, so the poll is a simple "everything after `after`".
#[derive(Clone, Serialize)]
pub struct LogLine {
    pub seq: u64,
    pub text: String,
}

/// Response for the `backend_log_tail` IPC. `cursor` is the value the client
/// should pass as `after` next time (exclusive high-water mark) — stable even
/// when `lines` is empty, so a caller that misses a poll still advances.
#[derive(Serialize)]
pub struct LogTail {
    pub lines: Vec<LogLine>,
    pub cursor: u64,
}

struct Ring {
    /// Next sequence number to assign (also the exclusive cursor high-water
    /// mark — one past the last line appended).
    next_seq: u64,
    entries: VecDeque<LogLine>,
}

impl Ring {
    const fn new() -> Self {
        Self {
            next_seq: 0,
            entries: VecDeque::new(),
        }
    }
}

// Const-initializable (no lazy-init dep): `Mutex::new` and `VecDeque::new` are
// both const fns.
static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// Append one line (trailing newline trimmed). Called from the tracing writer on
/// the backend's runtime threads; a poisoned lock is swallowed (logging must
/// never panic the peer).
fn append(text: &str) {
    let text = text.trim_end_matches(['\n', '\r']).to_string();
    if text.is_empty() {
        return;
    }
    if let Ok(mut ring) = RING.lock() {
        let seq = ring.next_seq;
        ring.next_seq += 1;
        ring.entries.push_back(LogLine { seq, text });
        while ring.entries.len() > CAP {
            ring.entries.pop_front();
        }
    }
}

/// Return every buffered line with `seq >= after`, plus the new cursor. A fresh
/// client passes `after = 0` (gets the whole buffer, capped at [`CAP`]); it then
/// passes back the returned `cursor` each poll.
pub fn tail(after: u64) -> LogTail {
    let ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    let lines: Vec<LogLine> = ring
        .entries
        .iter()
        .filter(|l| l.seq >= after)
        .cloned()
        .collect();
    LogTail {
        lines,
        cursor: ring.next_seq,
    }
}

/// A [`std::io::Write`] that tees `tracing` output into the ring *and* stdout, so
/// the terminal keeps its logs while the WebView can stream them too. Each
/// `tracing` fmt event is written in one `write` call, so a whole formatted line
/// lands as a single ring entry. Public only because it surfaces through
/// [`TeeMakeWriter`]'s associated `Writer` type; not meant to be named directly.
pub struct TeeWriter;

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        append(&String::from_utf8_lossy(buf));
        // Keep stdout as the source of truth for the terminal / container logs.
        std::io::stdout().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

/// [`MakeWriter`](tracing_subscriber::fmt::MakeWriter) factory for the fmt
/// subscriber — hand this to `.with_writer(...)` so every event tees to the ring
/// and stdout.
pub struct TeeMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TeeMakeWriter {
    type Writer = TeeWriter;
    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter
    }
}

// ---------------------------------------------------------------------------
// Tracing install + runtime-adjustable level (System Backend window control)
// ---------------------------------------------------------------------------

/// The log levels the window offers, coarsest → finest. `off` silences the
/// backend entirely. Kept in this order so the UI can render a stable dropdown.
pub const LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Reload handle for the backend's level filter — `set_level` swaps the filter
/// through it at runtime. `None` if tracing init lost the `try_init` race.
static RELOAD: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// The current level as a display string, for `get_backend_log_level`. Empty
/// until `init_tracing` runs.
static CURRENT_LEVEL: Mutex<String> = Mutex::new(String::new());

/// Install the backend's `tracing` subscriber: a reload-able `EnvFilter` (so the
/// window can change the level live) feeding an ANSI-colored fmt layer that tees
/// into the ring buffer + stdout. Replaces the old `fmt().try_init()`; call once
/// at startup. `try_init` so a double-install never panics.
pub fn init_tracing() {
    let (initial, name) = initial_filter();
    let (filter, handle) = reload::Layer::new(initial);
    let installed = Registry::default()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_ansi(true)
                .with_writer(TeeMakeWriter),
        )
        .try_init()
        .is_ok();
    if installed {
        let _ = RELOAD.set(handle);
        if let Ok(mut cur) = CURRENT_LEVEL.lock() {
            *cur = name;
        }
    }
}

/// Initial filter from `RUST_LOG` (honored for cold-boot verbosity), else `info`.
/// Returns the filter + a display name for the current-level readout.
fn initial_filter() -> (EnvFilter, String) {
    match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => {
            let v = v.trim().to_string();
            (EnvFilter::new(&v), v)
        }
        _ => (EnvFilter::new("info"), "info".to_string()),
    }
}

/// Change the backend log level at runtime (`off`/`error`/…/`trace`). Swaps the
/// reload filter and records the new level. Errors on an unknown level or if the
/// reload handle wasn't installed.
pub fn set_level(level: &str) -> Result<(), String> {
    let level = level.trim();
    if !LEVELS.contains(&level) {
        return Err(format!("unknown log level: {level}"));
    }
    let handle = RELOAD
        .get()
        .ok_or("tracing reload handle not installed")?;
    handle
        .modify(|f| *f = EnvFilter::new(level))
        .map_err(|e| e.to_string())?;
    if let Ok(mut cur) = CURRENT_LEVEL.lock() {
        *cur = level.to_string();
    }
    Ok(())
}

/// The current level display string (defaults to `info` before init).
pub fn current_level() -> String {
    CURRENT_LEVEL
        .lock()
        .ok()
        .map(|s| s.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "info".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize the tests that touch the process-global ring so they don't
    // interleave sequence numbers.
    fn reset() {
        let mut ring = RING.lock().unwrap();
        ring.next_seq = 0;
        ring.entries.clear();
    }

    #[test]
    fn append_and_tail_from_zero() {
        let _g = SERIAL.lock().unwrap();
        reset();
        append("first\n");
        append("second\r\n");
        let tail = tail(0);
        assert_eq!(tail.lines.len(), 2);
        assert_eq!(tail.lines[0].text, "first");
        assert_eq!(tail.lines[1].text, "second");
        assert_eq!(tail.cursor, 2);
    }

    #[test]
    fn tail_after_cursor_returns_only_new() {
        let _g = SERIAL.lock().unwrap();
        reset();
        append("a");
        let first = tail(0);
        assert_eq!(first.lines.len(), 1);
        append("b");
        let next = tail(first.cursor);
        assert_eq!(next.lines.len(), 1);
        assert_eq!(next.lines[0].text, "b");
    }

    #[test]
    fn empty_lines_are_dropped() {
        let _g = SERIAL.lock().unwrap();
        reset();
        append("\n");
        append("   \n"); // whitespace-only survives (only newline trim); assert below
        let tail = tail(0);
        // "\n" trims to empty → dropped; "   " is not empty → kept.
        assert_eq!(tail.lines.len(), 1);
        assert_eq!(tail.lines[0].text, "   ");
    }

    #[test]
    fn ring_caps_at_capacity() {
        let _g = SERIAL.lock().unwrap();
        reset();
        for i in 0..(CAP + 50) {
            append(&format!("line {i}"));
        }
        let tail = tail(0);
        assert_eq!(tail.lines.len(), CAP);
        // Oldest dropped: the first surviving line is #50.
        assert_eq!(tail.lines[0].text, "line 50");
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn levels_are_the_expected_ordered_set() {
        assert_eq!(LEVELS, &["off", "error", "warn", "info", "debug", "trace"]);
    }

    #[test]
    fn set_level_rejects_unknown() {
        assert!(set_level("bogus").is_err());
        assert!(set_level("verbose").is_err());
        // A known level with no reload handle installed (unit-test process) also
        // errors, but for the handle reason — the guard order is what we assert.
        assert!(set_level("bogus").unwrap_err().contains("unknown log level"));
    }

    #[test]
    fn current_level_has_a_nonempty_default() {
        assert!(!current_level().is_empty());
    }
}
