//! Renderer-neutral output for the entity-shell window.
//!
//! The DOM renderer turns this into a status header + `<pre>`
//! scrollback + `<input>` prompt. A future text/CLI renderer would
//! consume the same struct.
//!
//! Scrollback rows are *typed entries*, not pre-styled lines: each
//! `ScrollbackEntry` is either a verb-dispatcher output (`VerbOutput`,
//! `ShellError`, streaming chunks) or a small "framing" variant
//! (prompt echo, info text, subscription event from `tail`). The
//! renderer matches on the variant — colors and structure live in the
//! renderer, not in the model.

use std::sync::Arc;

use entity_shell::{DispatchChunk, ShellError, StreamChunk, VerbOutput};

/// Top-level output for one render pass of the shell window.
#[derive(Debug, Clone)]
pub struct ShellOutput {
    /// "Current working directory" — the path the shell is anchored
    /// at. Displayed in the status header and used as the relative
    /// root for `cd ./...` / `ls` with no argument.
    pub wd: String,
    /// Typed scrollback rows (oldest → newest). Bounded —
    /// see `model::SCROLLBACK_CAP`. `Arc` because `VerbOutput`
    /// contains non-`Clone` mpsc receivers; wrapping each entry
    /// lets `ShellOutput` derive `Clone` cheaply.
    pub scrollback: Vec<Arc<ScrollbackEntry>>,
    /// Last input the user was composing — restored into the
    /// `<input>` element after every rebuild so a tree-driven
    /// rebuild mid-keystroke doesn't drop the user's typing.
    pub draft: String,
    /// Tree path the shell state lives at — surfaced in the footer
    /// for transparency, matching the Settings pattern.
    pub state_path: String,
}

/// One row of scrollback. Renderer matches per-variant. Not `Clone`
/// — `Result(VerbOutput)` can hold a `VerbOutput` that contains a
/// non-`Clone` `mpsc::Receiver`. Use `Arc<ScrollbackEntry>` when you
/// need cheap fan-out (which `ShellState.scrollback` does).
#[derive(Debug)]
pub enum ScrollbackEntry {
    /// Echo of the prompt + submitted command.
    PromptEcho { wd: String, line: String },
    /// Synchronous verb result.
    Result(VerbOutput),
    /// Verb error (sync, or stream-terminal failure surfaced as the
    /// dispatcher's `Err` arm).
    Error(ShellError),
    /// One chunk from a streaming `VerbOutput::Lines(rx)` verb.
    StreamChunk(StreamChunk),
    /// One chunk from a streaming `VerbOutput::Dispatch(rx)` verb
    /// (exec-style results).
    DispatchChunk(DispatchChunk),
    /// Non-verb info text — welcome line, `tail` callback's
    /// `Put`/`Resync` event, ad-hoc dispatcher messages.
    Info(String),
    /// Non-verb error text — `tail` callback's `Remove` event,
    /// unknown-verb fallback, dispatcher-side message.
    ErrorText(String),
    /// Listing-styled non-verb text — currently used by `tail`'s
    /// `Put` events so subscription rows render visually distinct
    /// from generic info.
    Listing(String),
}

impl ScrollbackEntry {
    /// Concatenated rendered text — convenience for tests and the
    /// CLI fallback renderer. Streaming variants flatten to their
    /// per-chunk text; multi-row results join with newlines.
    #[allow(dead_code)]
    pub fn render_text(&self) -> String {
        match self {
            Self::PromptEcho { wd, line } => format!("{} > {}", wd, line),
            Self::Result(v) => render_verb_output_lines(v).join("\n"),
            Self::Error(e) => e.to_string(),
            Self::StreamChunk(c) => match c {
                StreamChunk::Dispatched(t)
                | StreamChunk::Line(t)
                | StreamChunk::Complete(t) => t.clone(),
                StreamChunk::Failed(e) => e.to_string(),
            },
            Self::DispatchChunk(c) => match c {
                DispatchChunk::Dispatched(t)
                | DispatchChunk::Progress(t)
                | DispatchChunk::Complete(t) => t.clone(),
                DispatchChunk::Failed(e) => e.to_string(),
            },
            Self::Info(t) | Self::ErrorText(t) | Self::Listing(t) => t.clone(),
        }
    }

    /// True for variants the renderer styles as errors (red tone).
    #[cfg(test)]
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::Error(_)
                | Self::ErrorText(_)
                | Self::StreamChunk(StreamChunk::Failed(_))
                | Self::DispatchChunk(DispatchChunk::Failed(_))
        )
    }

    /// True for entity-dump entries (`cat` output).
    #[cfg(test)]
    pub fn is_entity(&self) -> bool {
        matches!(self, Self::Result(VerbOutput::Entity(_)))
    }

    /// True for prompt-echo entries.
    #[cfg(test)]
    pub fn is_prompt(&self) -> bool {
        matches!(self, Self::PromptEcho { .. })
    }

    /// True for listing-row entries (`ls`/`peers`/`tree` output,
    /// streaming `tail`/`query` chunks, tail subscription events).
    #[cfg(test)]
    pub fn is_listing(&self) -> bool {
        matches!(
            self,
            Self::Listing(_)
                | Self::StreamChunk(StreamChunk::Line(_))
                | Self::Result(VerbOutput::Listing { .. })
                | Self::Result(VerbOutput::Tree(_))
        )
    }

    /// Catch-all for non-error, non-prompt, non-entity output —
    /// covers `Result(VerbOutput::Info|Message|Path|Listing|Tree)`,
    /// streaming progress chunks, ad-hoc info text. Tests use this
    /// to filter to "verb produced a successful response" and combine
    /// with `text_contains` for substring checks. Note: this is the
    /// rendered-tone catch-all — `Result(VerbOutput::Listing)`
    /// returns both `is_info()` and `is_listing()` true because its
    /// header rows render as info while entries render as listings.
    #[cfg(test)]
    pub fn is_info(&self) -> bool {
        !self.is_error() && !self.is_prompt() && !self.is_entity()
    }

    /// Substring match against the rendered text of the entry. Tests
    /// use this in place of the old `l.text.contains(...)` idiom.
    #[cfg(test)]
    pub fn text_contains(&self, needle: &str) -> bool {
        self.render_text().contains(needle)
    }
}

/// Lower a `VerbOutput` to one or more display-text rows. The DOM
/// renderer uses the same shape today (one row per row of text);
/// future per-variant rendering can deviate. Public so the CLI/test
/// helpers can reuse the same lowering.
pub fn render_verb_output_lines(out: &VerbOutput) -> Vec<String> {
    let mut rows = Vec::new();
    match out {
        VerbOutput::Path(p) => rows.push(p.clone()),
        VerbOutput::Message(m) => rows.push(m.clone()),
        VerbOutput::Listing { sections } => {
            for s in sections {
                if let Some(h) = &s.header {
                    rows.push(h.clone());
                }
                rows.extend(s.entries.iter().cloned());
            }
        }
        VerbOutput::Entity(e) => {
            rows.push(format!("{}  type={} bytes={}", e.path, e.entity_type, e.byte_len));
            for line in e.body.lines() {
                rows.push(line.to_string());
            }
        }
        VerbOutput::Tree(t) => {
            rows.push(match t.depth_limit {
                Some(d) => format!("tree {} (depth={})", t.root, d),
                None => format!("tree {}", t.root),
            });
            for entry in &t.entries {
                rows.push(format!("{}{}", "  ".repeat(entry.depth), entry.path));
            }
        }
        VerbOutput::Info(rows_) => {
            for row in rows_ {
                rows.push(match &row.label {
                    Some(l) => format!("{}: {}", l, row.value),
                    None => row.value.clone(),
                });
            }
        }
        VerbOutput::Lines(_) | VerbOutput::Dispatch(_) => {
            rows.push("(streaming output)".into());
        }
    }
    rows
}
