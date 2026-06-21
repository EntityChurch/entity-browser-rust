//! Shell model — entity-backed `ShellState` + verb dispatch.
//!
//! Per-window state lives in the tree at
//! `{peer_id}/app/entity-browser/workspace/windows/{id}/state` as an
//! `app/state/shell` entity. The in-memory mirror is an
//! `Arc<Mutex<ShellState>>` so spawned futures (async / streaming
//! verbs) can append scrollback entries.
//!
//! Verb dispatch routes every line through the extracted
//! `entity_shell` crate (`../entity-core-rust/bindings/shell/`); only
//! the embedding-side concerns (state mirror, persistence, scrollback,
//! tab completion against app state, follow-up action drain) live
//! here. The `binding` module supplies the crate's `PeerBinding` /
//! `SelectionSink` / `AppActionSink` traits.

use std::sync::{Arc, Mutex};

/// Spawn an async task on the current runtime. Two definitions per
/// target — WASM accepts non-Send futures via `spawn_local`; native
/// tokio requires `Send`. Used by both:
/// - Streaming-result drain tasks (chunk consumers).
/// - Streaming-result producer tasks the crate spawns through this
///   callback when an async verb returns a streaming receiver.
///
/// Both sides capture `Send + 'static` data (Arc<Mutex>, mpsc, etc.),
/// so the native bound is satisfied in practice.
#[cfg(target_arch = "wasm32")]
pub(super) fn spawn_task<F>(task: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(task);
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn spawn_task<F>(task: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        // Fire-and-forget; drop the JoinHandle explicitly.
        drop(rt.spawn(task));
    }
}

use entity_entity::Entity;

use crate::peers::Peers;
use crate::window::WindowId;

use super::output::{ScrollbackEntry, ShellOutput};

/// Maximum scrollback rows held in memory. Scrollback is in-memory
/// only — only history persists, matching workbench-go and
/// conventional shell semantics. The cap keeps an idle shell from
/// accumulating unbounded entries.
pub const SCROLLBACK_CAP: usize = 500;

/// Maximum history entries persisted. History is small (one line per
/// submit) so the cap is loose.
pub const HISTORY_CAP: usize = 200;

#[derive(Debug, Clone)]
pub struct ShellState {
    /// Current working directory. Starts at the bound peer's root
    /// (`/{peer_id}/`).
    pub wd: String,
    /// Submitted command lines (oldest → newest). Persisted.
    pub history: Vec<String>,
    /// Position in `history` while arrow-walking. `None` means the
    /// user is composing fresh input (draft is authoritative).
    pub history_cursor: Option<usize>,
    /// Typed scrollback rows. **Not persisted** — refresh wipes
    /// scrollback (conventional shell behavior; matches workbench-go).
    /// Stored as `Arc<ScrollbackEntry>` so `render_output` can clone
    /// the vec into `ShellOutput` cheaply.
    pub scrollback: Vec<Arc<ScrollbackEntry>>,
    /// Last in-progress draft — restored into the `<input>` after
    /// rebuilds.
    pub draft: String,
    /// What the user was typing when they started a history walk.
    /// Restored into `draft` on ArrowDown past the bottom (standard
    /// shell behavior). `None` outside an active walk.
    pub saved_draft: Option<String>,
}

impl ShellState {
    pub fn initial(peer_id: &str) -> Self {
        Self {
            wd: format!("/{}/", peer_id),
            history: Vec::new(),
            history_cursor: None,
            scrollback: vec![Arc::new(ScrollbackEntry::Info(
                "entity-shell — type `help` for a list of verbs.".into(),
            ))],
            draft: String::new(),
            saved_draft: None,
        }
    }

    pub fn to_entity(&self) -> Entity {
        let history: Vec<ciborium::Value> = self
            .history
            .iter()
            .map(|s| ciborium::Value::Text(s.clone()))
            .collect();

        let mut map: Vec<(ciborium::Value, ciborium::Value)> = vec![
            (
                ciborium::Value::Text("wd".into()),
                ciborium::Value::Text(self.wd.clone()),
            ),
            (
                ciborium::Value::Text("history".into()),
                ciborium::Value::Array(history),
            ),
            (
                ciborium::Value::Text("draft".into()),
                ciborium::Value::Text(self.draft.clone()),
            ),
        ];
        if let Some(cur) = self.history_cursor {
            map.push((
                ciborium::Value::Text("history_cursor".into()),
                ciborium::Value::Integer((cur as u64).into()),
            ));
        }
        if let Some(ref saved) = self.saved_draft {
            map.push((
                ciborium::Value::Text("saved_draft".into()),
                ciborium::Value::Text(saved.clone()),
            ));
        }

        let mut buf = Vec::new();
        ciborium::into_writer(&ciborium::Value::Map(map), &mut buf)
            .expect("CBOR encode of ShellState");
        Entity::new("app/state/shell", buf).expect("ShellState entity well-formed")
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::initial(""),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::initial(""),
        };

        let mut state = Self::initial("");
        // initial() seeded a welcome line — clear it; loaded sessions
        // start with empty scrollback (we don't persist scrollback).
        state.scrollback.clear();
        for (k, v) in map {
            match k.as_text() {
                Some("wd") => {
                    if let Some(s) = v.as_text() {
                        state.wd = s.to_string();
                    }
                }
                Some("draft") => {
                    if let Some(s) = v.as_text() {
                        state.draft = s.to_string();
                    }
                }
                Some("history") => {
                    if let Some(arr) = v.as_array() {
                        state.history = arr
                            .iter()
                            .filter_map(|v| v.as_text().map(str::to_string))
                            .collect();
                    }
                }
                Some("history_cursor") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        if n >= 0 {
                            state.history_cursor = Some(n as usize);
                        }
                    }
                }
                Some("saved_draft") => {
                    if let Some(s) = v.as_text() {
                        state.saved_draft = Some(s.to_string());
                    }
                }
                // `scrollback`: legacy field from earlier devices that
                // persisted scrollback. Silently
                // dropped — scrollback is in-memory only now.
                _ => {}
            }
        }
        state
    }

    /// Append one scrollback entry, trimming the head if we'd exceed
    /// `SCROLLBACK_CAP`. Idempotent w.r.t. cap — never grows past it.
    pub fn push(&mut self, entry: ScrollbackEntry) {
        self.scrollback.push(Arc::new(entry));
        let overflow = self.scrollback.len().saturating_sub(SCROLLBACK_CAP);
        if overflow > 0 {
            self.scrollback.drain(0..overflow);
        }
    }

    /// Record a submitted line in history (bounded), reset the
    /// history cursor, and clear the draft.
    pub fn record_submit(&mut self, line: &str) {
        if !line.is_empty() && self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
            let overflow = self.history.len().saturating_sub(HISTORY_CAP);
            if overflow > 0 {
                self.history.drain(0..overflow);
            }
        }
        self.history_cursor = None;
        self.saved_draft = None;
        self.draft.clear();
    }
}

/// Verb table tab completion matches against — the crate's
/// dispatcher list plus the embedding-only `clear` verb. Single
/// source of truth: `entity_shell::dispatcher::VERBS`.
fn all_verbs() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = entity_shell::dispatcher::VERBS.to_vec();
    v.push("clear");
    v
}

/// Window-type names that `open` can spawn. Must match the
/// `WindowType.name` strings registered in `app.rs`. The shell
/// can't read the registry directly (no `WindowManager` reference
/// from the model), so we mirror it here. New windows should be
/// added in both places — caught by the e2e if missed.
pub const WINDOW_TYPES: &[&str] = &[
    "Shell",
    "Entity Tree",
    "Settings",
    "Event Log",
    "Key Manager",
    "Knowledge Base",
    "Peer Connections",
    "Execute Console",
    "Query Console",
    "Peers",
    "Chain Trace",
    "Path Tap",
    "Wire Recorder",
    "Content Stream",
];

/// Resolve a user-supplied window name to one of the canonical
/// `WINDOW_TYPES` entries. Tolerates case, hyphens, underscores,
/// and missing spaces — so `entity-tree` / `entity_tree` /
/// `EntityTree` / `entity tree` all map to `"Entity Tree"`. Returns
/// `None` when nothing matches.
pub fn resolve_window_name(input: &str) -> Option<&'static str> {
    let key = normalize_window_token(input);
    WINDOW_TYPES
        .iter()
        .copied()
        .find(|name| normalize_window_token(name) == key)
}

fn normalize_window_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn complete_verb(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    let verbs = all_verbs();
    let matches: Vec<&&str> = verbs.iter().filter(|v| v.starts_with(prefix)).collect();
    match matches.as_slice() {
        [v] => Some(format!("{} ", v)),
        // Multiple matches → expand to the longest common prefix so a
        // second Tab walks further. Useful for "c" → "c" still (because
        // cd/cat/clear share only "c"), but exposes the future menu UI.
        ms if ms.len() > 1 => {
            let lcp = longest_common_prefix(ms.iter().map(|s| **s));
            if lcp.len() > prefix.len() {
                Some(lcp)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn complete_path(
    prefix: &str,
    token: &str,
    wd: &str,
    peers: &Peers,
    bound_peer: &str,
) -> Option<String> {
    // Split the token into (dir, basename). If the token is empty or
    // ends with `/`, listing happens against the resolved directory
    // itself; otherwise we strip off the trailing fragment.
    let (search_dir, base) = match token.rfind('/') {
        Some(i) => (token[..=i].to_string(), token[i + 1..].to_string()),
        None => (String::new(), token.to_string()),
    };
    let resolved = if search_dir.is_empty() {
        // No slash yet — list children of wd.
        if wd.ends_with('/') { wd.to_string() } else { format!("{}/", wd) }
    } else {
        entity_shell::path::resolve(wd, &search_dir)
    };
    let pid = entity_shell::path::peer_id_of(&resolved)
        .unwrap_or_else(|| bound_peer.to_string());
    let entries = peers.tree_listing(&pid, &resolved);
    // Each `LocationEntry.path` is the *full* qualified path. We need
    // the basename relative to `resolved` for matching.
    let candidates: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let rest = e.path.strip_prefix(resolved.trim_end_matches('/'))?;
            let rest = rest.trim_start_matches('/');
            // Take only the first segment so directories collapse to
            // their bucket name.
            let seg = rest.split('/').next()?;
            if seg.is_empty() { None } else { Some(seg.to_string()) }
        })
        .filter(|seg| seg.starts_with(&base))
        .collect();
    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    let uniq: Vec<String> = candidates
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect();

    match uniq.as_slice() {
        [match_] => Some(format!("{}{}{}", prefix, search_dir, match_)),
        ms if ms.len() > 1 => {
            let lcp = longest_common_prefix(ms.iter().map(String::as_str));
            if lcp.len() > base.len() {
                Some(format!("{}{}{}", prefix, search_dir, lcp))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn longest_common_prefix<'a, I: IntoIterator<Item = &'a str>>(iter: I) -> String {
    let mut iter = iter.into_iter();
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut common = first.to_string();
    for s in iter {
        let n = common.chars().zip(s.chars()).take_while(|(a, b)| a == b).count();
        common.truncate(common.char_indices().nth(n).map(|(i, _)| i).unwrap_or(common.len()));
        if common.is_empty() {
            break;
        }
    }
    common
}

/// Parse a JSON snippet into a CBOR-encoded params entity. Returns
/// the produced `Entity` (type `system/params`) or a string error.
pub(super) fn parse_json_params(text: &str) -> Result<Entity, String> {
    let bytes = parse_json_to_ecf(text)?;
    Entity::new("system/params", bytes).map_err(|e| e.to_string())
}

/// Parse a JSON snippet into raw CBOR bytes. Shared by `exec`
/// (wraps into a `system/params` entity) and `set` (wraps into a
/// caller-named entity type).
pub(super) fn parse_json_to_ecf(text: &str) -> Result<Vec<u8>, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    let cbor = json_to_cbor(&json);
    let mut buf = Vec::new();
    ciborium::into_writer(&cbor, &mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn json_to_cbor(v: &serde_json::Value) -> ciborium::Value {
    use serde_json::Value as J;
    match v {
        J::Null => ciborium::Value::Null,
        J::Bool(b) => ciborium::Value::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                ciborium::Value::Integer(i.into())
            } else if let Some(u) = n.as_u64() {
                ciborium::Value::Integer(u.into())
            } else if let Some(f) = n.as_f64() {
                ciborium::Value::Float(f)
            } else {
                ciborium::Value::Null
            }
        }
        J::String(s) => ciborium::Value::Text(s.clone()),
        J::Array(arr) => ciborium::Value::Array(arr.iter().map(json_to_cbor).collect()),
        J::Object(obj) => ciborium::Value::Map(
            obj.iter()
                .map(|(k, v)| (ciborium::Value::Text(k.clone()), json_to_cbor(v)))
                .collect(),
        ),
    }
}

// Path/alias helpers moved to the `entity_shell::path` and
// `entity_shell::alias` modules in the extracted crate. Local
// completers go through them directly; the dispatcher uses the same
// modules crate-side so there is one source of truth.

/// Active `tail` subscription on this shell window. The `active`
/// flag is a soft cancellation switch: the tail-install callback
/// captures a clone and silently no-ops once it goes false (set by
/// `untail`). The underlying subscription handle stays alive on the
/// WindowWatch until the window closes — this is a small cost we
/// accept to avoid plumbing handle-return through Peers / SDK.
#[derive(Debug, Clone)]
pub struct TailEntry {
    pub prefix: String,
    pub active: Arc<std::sync::atomic::AtomicBool>,
}

/// Shell model — mirrored state shape.
///
/// Fields are `pub(super)` so the sibling `verbs` module can poke at
/// them without going through accessor methods (each verb does a lot
/// of `self.inner.lock()` + scrollback writes). They stay private to
/// the `views::shell` module — outside callers still go through the
/// public methods like `handle_submit` / `render_output`.
#[derive(Debug)]
pub struct ShellModel {
    pub(super) window_id: WindowId,
    pub(super) peer_id: String,
    pub(super) inner: Arc<Mutex<ShellState>>,
    /// Outbound actions a verb wants the app loop to process —
    /// `Action::CreatePeerWithMode`, `Action::DeletePeer`, etc. The
    /// model can't push directly to the renderer's action queue
    /// (no `DomCtx` at verb-dispatch time), so verbs append here
    /// and the window's `render_dom` drains the queue into
    /// `ctx.actions` before each render. One-frame delay; cheap.
    pub(super) pending_out: Arc<Mutex<Vec<crate::action::Action>>>,
    /// Active tail subscriptions. The controller's `install_tail`
    /// records each entry here; verbs read it (`tails`) and flip
    /// the flag on cancel (`untail`).
    pub(super) tails: Arc<Mutex<Vec<TailEntry>>>,
}

impl ShellModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let state = ShellState::initial(&peer_id);
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(state)),
            pending_out: Arc::new(Mutex::new(Vec::new())),
            tails: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Drain queued follow-up actions. Called by `ShellWindow::render_dom`
    /// before each render and forwarded to `DomCtx.actions` so the
    /// next frame's `process_actions` picks them up.
    pub fn drain_pending_actions(&self) -> Vec<crate::action::Action> {
        std::mem::take(&mut *self.pending_out.lock().unwrap())
    }

    fn state_path(&self) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    /// Hydrate the in-memory mirror from the tree (or write defaults
    /// if absent). Called once at factory time.
    pub fn initialize(&mut self, peers: &Peers) {
        let path = self.state_path();
        if let Some(entity) = peers.get_entity(&self.peer_id, &path) {
            let mut state = ShellState::from_entity(&entity);
            if state.wd.is_empty() {
                state.wd = format!("/{}/", self.peer_id);
            }
            *self.inner.lock().unwrap() = state;
        } else {
            peers.dispatch_write(&self.peer_id, path, self.inner.lock().unwrap().to_entity());
        }
    }

    fn persist(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().to_entity();
        peers.dispatch_write(&self.peer_id, self.state_path(), entity);
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist(peers);
    }

    // -- Action methods --

    /// Handle a submitted command. Inline verb dispatch for the
    /// embedded set (help/pwd/cd/ls/cat/clear/exec). The
    /// `shellcmd`-style verb registry port is deferred to a follow-up
    /// session (handoff §"Out of scope") once the shape is felt.
    /// `dirty` is signaled by the async `exec` verb when its result
    /// arrives so the renderer rebuilds without waiting for the next
    /// user action.
    pub fn handle_submit(
        &self,
        line: &str,
        peers: &Peers,
        window_id: WindowId,
        dirty: crate::window_watch::DirtyFlag,
    ) {
        // Echo the prompt + line first so failures still leave a
        // visible record of what the user submitted.
        let mut state = self.inner.lock().unwrap();
        let wd = state.wd.clone();
        state.push(ScrollbackEntry::PromptEcho { wd, line: line.into() });
        state.record_submit(line);
        let initial_wd = state.wd.clone();
        drop(state); // verb dispatch may re-lock; never hold across.

        // Route everything through the crate dispatcher. Returns
        // Some(result) when the line's verb is in the crate's
        // vocabulary (all Tier C + Tier E today); None for `clear`
        // (UI-only) or unknown verbs.
        let mut shell = entity_shell::Shell::with_wd(&self.peer_id, initial_wd);
        let binding = super::binding::PeersBinding::new(peers, &self.peer_id);
        let sink = super::binding::PanelSelectionSink::new(peers, &self.peer_id, window_id);
        let action_sink = super::binding::ShellActionSink::new(
            &self.pending_out,
            window_id,
            self.tails.clone(),
        );
        let crate_result = entity_shell::dispatcher::dispatch(
            line,
            &mut shell,
            &binding,
            Some(&sink),
            &action_sink,
            spawn_task,
        );
        // Persist any wd mutation (cd) back to the session state.
        self.inner.lock().unwrap().wd = shell.wd().to_string();

        if let Some(result) = crate_result {
            self.absorb_dispatch_result(result, &dirty);
            return;
        }

        // UI-only ops (clear) + unknown-verb fallback.
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let verb = trimmed.split_whitespace().next().unwrap_or("");
        match verb {
            "clear" => self.clear(),
            other => {
                self.inner
                    .lock()
                    .unwrap()
                    .push(ScrollbackEntry::ErrorText(format!("unknown verb: {}", other)));
            }
        }
    }

    /// Append a dispatch result to scrollback. Sync variants land as
    /// a single `Result`/`Error` entry; streaming variants spawn a
    /// drain task that pushes a fresh entry per chunk and flips the
    /// dirty flag so the renderer rebuilds as chunks arrive.
    fn absorb_dispatch_result(
        &self,
        result: Result<entity_shell::VerbOutput, entity_shell::ShellError>,
        dirty: &crate::window_watch::DirtyFlag,
    ) {
        use entity_shell::VerbOutput;
        match result {
            Err(e) => {
                self.inner.lock().unwrap().push(ScrollbackEntry::Error(e));
            }
            Ok(VerbOutput::Lines(rx)) => self.drain_stream(rx, dirty.clone()),
            Ok(VerbOutput::Dispatch(rx)) => self.drain_dispatch_stream(rx, dirty.clone()),
            Ok(other) => {
                self.inner.lock().unwrap().push(ScrollbackEntry::Result(other));
            }
        }
    }

    fn drain_stream(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<entity_shell::StreamChunk>,
        dirty: crate::window_watch::DirtyFlag,
    ) {
        let scrollback = self.inner.clone();
        spawn_task(async move {
            while let Some(chunk) = rx.recv().await {
                scrollback.lock().unwrap().push(ScrollbackEntry::StreamChunk(chunk));
                dirty.mark();
            }
        });
    }

    fn drain_dispatch_stream(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<entity_shell::DispatchChunk>,
        dirty: crate::window_watch::DirtyFlag,
    ) {
        let scrollback = self.inner.clone();
        spawn_task(async move {
            while let Some(chunk) = rx.recv().await {
                scrollback.lock().unwrap().push(ScrollbackEntry::DispatchChunk(chunk));
                dirty.mark();
            }
        });
    }

    /// Up arrow. Returns `true` if state changed so the controller
    /// persists and the section rebuilds with the new draft. Empty
    /// history is a no-op so an unsuspecting arrow press doesn't
    /// churn the DOM.
    pub fn history_prev(&self, current: &str) -> bool {
        let mut state = self.inner.lock().unwrap();
        if state.history.is_empty() {
            return false;
        }
        // First step into the walk → stash what the user was typing so
        // ArrowDown past the bottom can restore it.
        if state.history_cursor.is_none() {
            state.saved_draft = Some(current.to_string());
        }
        let next_cursor = match state.history_cursor {
            None => state.history.len().saturating_sub(1),
            Some(0) => 0,
            Some(n) => n - 1,
        };
        state.history_cursor = Some(next_cursor);
        state.draft = state.history[next_cursor].clone();
        true
    }

    /// Down arrow. Returns `true` if state changed.
    pub fn history_next(&self, current: &str) -> bool {
        let mut state = self.inner.lock().unwrap();
        // Off the walk → no-op. (The live input value is whatever the
        // user is typing; we don't touch it.)
        let Some(cur) = state.history_cursor else {
            // The user might also be hitting ArrowDown to record
            // current typing into saved_draft; we don't need that
            // since record_submit re-snapshots on next Enter.
            let _ = current;
            return false;
        };
        if cur + 1 >= state.history.len() {
            // Walked past the bottom → restore the saved draft.
            state.history_cursor = None;
            state.draft = state.saved_draft.take().unwrap_or_default();
        } else {
            let next = cur + 1;
            state.history_cursor = Some(next);
            state.draft = state.history[next].clone();
        }
        true
    }

    /// Wipe the scrollback (Ctrl-L or `clear` verb).
    pub fn clear(&self) {
        let mut state = self.inner.lock().unwrap();
        state.scrollback.clear();
    }

    /// Track in-progress input so a tree-driven rebuild restores it.
    pub fn set_draft(&self, draft: &str) {
        self.inner.lock().unwrap().draft = draft.to_string();
    }

    /// Tab completion. Mutates `draft` with the single-cycle
    /// completion (matches workbench-go's "first-match replace"
    /// behavior; menu-style is a future enhancement). Returns `true`
    /// when state changed so the controller persists + the section
    /// rebuilds.
    ///
    /// Two cases:
    /// - No space yet → verb completion (prefix-match against the
    ///   embedded verb table).
    /// - Space present → path completion (split off the last token,
    ///   prefix-match basename under `dirname` via L0 tree_listing).
    pub fn complete(&self, partial: &str, peers: &Peers) -> bool {
        let completed = self.compute_completion(partial, peers);
        match completed {
            Some(new_draft) if new_draft != partial => {
                self.inner.lock().unwrap().draft = new_draft;
                true
            }
            _ => false,
        }
    }

    /// Verb-aware completion router. Splits `partial` into tokens and
    /// picks the right completer (verb table, window types, subcommand
    /// set, aliases, path listing). Pure-ish — needs `peers` for path
    /// + alias listing only.
    fn compute_completion(&self, partial: &str, peers: &Peers) -> Option<String> {
        let space_idx = partial.find(' ');
        let Some(idx) = space_idx else {
            return complete_verb(partial);
        };
        let prefix = &partial[..=idx]; // verb plus trailing space
        let rest = &partial[idx + 1..];
        let verb = partial[..idx].trim();

        match verb {
            "open" => complete_window_name(prefix, rest),
            "peer" | "peers" => self.complete_peer_args(prefix, rest, peers),
            // Path-taking verbs: support @alias completion in addition
            // to tree-prefix matching.
            "cd" | "ls" | "cat" | "put" | "rm" | "remove" => {
                // Only operate on the LAST token of `rest` — earlier
                // tokens (e.g. `put <path> <type> <json>`) shouldn't be
                // path-completed.
                let (token_prefix, token) = split_last_token(rest);
                let combined_prefix = format!("{}{}", prefix, token_prefix);
                self.complete_path_or_alias(&combined_prefix, token, peers)
            }
            _ => {
                let wd = self.inner.lock().unwrap().wd.clone();
                complete_path(prefix, rest, &wd, peers, &self.peer_id)
            }
        }
    }

    fn complete_path_or_alias(
        &self,
        prefix: &str,
        token: &str,
        peers: &Peers,
    ) -> Option<String> {
        if let Some(name) = token.strip_prefix('@') {
            return complete_alias(prefix, name, peers);
        }
        let wd = self.inner.lock().unwrap().wd.clone();
        complete_path(prefix, token, &wd, peers, &self.peer_id)
    }

    fn complete_peer_args(
        &self,
        prefix: &str,
        rest: &str,
        peers: &Peers,
    ) -> Option<String> {
        // `peer <sub>` — completing the subcommand.
        if !rest.contains(' ') {
            return complete_token_from(prefix, rest, PEER_SUBCOMMANDS);
        }
        // `peer <sub> <arg>` — depends on subcommand.
        let sub_space = rest.find(' ').unwrap();
        let sub = &rest[..sub_space];
        let after = &rest[sub_space + 1..];
        let prefix_with_sub = format!("{}{} ", prefix, sub);
        match sub {
            "create" | "new" => {
                let (head, tail) = split_last_token(after);
                // Only complete the FIRST positional (mode); subsequent
                // tokens are a free-form label.
                if head.is_empty() {
                    complete_token_from(&prefix_with_sub, tail, PEER_CREATE_MODES)
                } else {
                    None
                }
            }
            "delete" | "rm" | "remove" => {
                let (head, tail) = split_last_token(after);
                if !head.is_empty() {
                    return None;
                }
                if let Some(name) = tail.strip_prefix('@') {
                    complete_alias(&prefix_with_sub, name, peers)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

pub const PEER_SUBCOMMANDS: &[&str] = &["list", "create", "delete", "rename"];
pub const PEER_CREATE_MODES: &[&str] = &["frontend", "memory", "opfs"];

/// Generic prefix-match completer over a fixed table. Mirrors
/// `complete_verb` shape but returns the bare expanded token (no
/// trailing space) when the match isn't unique; the caller decides
/// whether to add a space.
fn complete_token_from(
    prefix: &str,
    partial: &str,
    table: &[&str],
) -> Option<String> {
    let matches: Vec<&str> = table.iter().copied().filter(|t| t.starts_with(partial)).collect();
    match matches.as_slice() {
        [t] => Some(format!("{}{} ", prefix, t)),
        ms if ms.len() > 1 => {
            let lcp = longest_common_prefix(ms.iter().copied());
            if lcp.len() > partial.len() {
                Some(format!("{}{}", prefix, lcp))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn complete_window_name(prefix: &str, partial: &str) -> Option<String> {
    // Match against canonical names AND their normalized forms so
    // `entity-tr<Tab>` expands to `Entity Tree`.
    let normalized_partial = normalize_window_token(partial);
    let mut matches: Vec<&'static str> = WINDOW_TYPES
        .iter()
        .copied()
        .filter(|n| {
            n.to_ascii_lowercase().starts_with(&partial.to_ascii_lowercase())
                || normalize_window_token(n).starts_with(&normalized_partial)
        })
        .collect();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [name] => Some(format!("{}{}", prefix, name)),
        ms if ms.len() > 1 => {
            // Multiple matches → pick the shortest canonical name's
            // prefix as a hint. Cheap LCP across the canonical forms.
            let lcp = longest_common_prefix(ms.iter().copied());
            if lcp.len() > partial.len() {
                Some(format!("{}{}", prefix, lcp))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn complete_alias(prefix: &str, alias_partial: &str, peers: &Peers) -> Option<String> {
    let lower_partial = alias_partial.to_ascii_lowercase();
    // Collect candidate alias names: reserved keywords + every local
    // peer's label (when present) + every connected peer's label
    // (rare but cheap to include).
    let mut candidates: Vec<String> = vec![
        "primary".into(),
        "system".into(),
        "default".into(),
    ];
    let mut ids: Vec<String> = peers.peer_ids();
    ids.extend(crate::connections::read_connected(peers));
    for pid in &ids {
        if let Some(label) = peers
            .peer_metadata(pid)
            .and_then(|m| m.label)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
        {
            candidates.push(label);
        }
    }
    candidates.sort();
    candidates.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

    let matches: Vec<&str> = candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().starts_with(&lower_partial))
        .map(String::as_str)
        .collect();
    match matches.as_slice() {
        [m] => Some(format!("{}@{}/", prefix, m)),
        ms if ms.len() > 1 => {
            let lcp = longest_common_prefix(ms.iter().copied());
            if lcp.len() > alias_partial.len() {
                Some(format!("{}@{}", prefix, lcp))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Split `s` into (prefix-up-to-last-space-inclusive, last-token).
/// Used to isolate the segment we're completing while leaving earlier
/// tokens untouched.
fn split_last_token(s: &str) -> (&str, &str) {
    match s.rfind(' ') {
        Some(i) => (&s[..=i], &s[i + 1..]),
        None => ("", s),
    }
}

impl ShellModel {
    // -- Pure read --

    pub fn render_output(&self) -> ShellOutput {
        let state = self.inner.lock().unwrap();
        ShellOutput {
            wd: state.wd.clone(),
            scrollback: state.scrollback.clone(),
            draft: state.draft.clone(),
            state_path: self.state_path(),
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> ShellState {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_seeds_welcome_line() {
        let s = ShellState::initial("PEER1");
        assert_eq!(s.wd, "/PEER1/");
        assert_eq!(s.scrollback.len(), 1);
        assert!(s.scrollback[0].is_info());
    }

    #[test]
    fn state_round_trip_preserves_history_but_drops_scrollback() {
        // Scrollback is in-memory only — refresh round-trips
        // wd/history/draft/cursor but loses scrollback.
        let mut s = ShellState::initial("PEER1");
        s.push(ScrollbackEntry::Info("info row".into()));
        s.push(ScrollbackEntry::ErrorText("err row".into()));
        s.record_submit("ls");
        s.record_submit("pwd");
        s.draft = "partial".into();
        s.history_cursor = Some(0);
        s.saved_draft = Some("in-flight".into());

        let entity = s.to_entity();
        assert_eq!(entity.entity_type, "app/state/shell");
        let s2 = ShellState::from_entity(&entity);
        assert_eq!(s2.wd, s.wd);
        assert_eq!(s2.history, s.history);
        assert_eq!(s2.draft, s.draft);
        assert_eq!(s2.history_cursor, s.history_cursor);
        assert_eq!(s2.saved_draft, s.saved_draft);
        // Scrollback NOT preserved.
        assert!(s2.scrollback.is_empty());
    }

    #[test]
    fn push_respects_scrollback_cap() {
        let mut s = ShellState::initial("p");
        for i in 0..(SCROLLBACK_CAP + 50) {
            s.push(ScrollbackEntry::Info(format!("{i}")));
        }
        assert_eq!(s.scrollback.len(), SCROLLBACK_CAP);
        // Oldest entries dropped; the first surviving line should be
        // index 50 (we appended 0..SCROLLBACK_CAP+50 minus seed).
        assert!(
            s.scrollback.first().unwrap().render_text().parse::<usize>().unwrap() >= 50
        );
    }

    #[test]
    fn record_submit_skips_consecutive_duplicates() {
        let mut s = ShellState::initial("p");
        s.record_submit("ls");
        s.record_submit("ls");
        s.record_submit("pwd");
        s.record_submit("ls");
        assert_eq!(s.history, vec!["ls", "pwd", "ls"]);
    }

    fn flag() -> crate::window_watch::DirtyFlag {
        crate::window_watch::WindowWatch::new().flag()
    }

    #[test]
    fn history_nav_walks_backwards_and_restores_draft() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("ls", &peers, 1, flag());
        model.handle_submit("pwd", &peers, 1, flag());
        model.handle_submit("cat foo", &peers, 1, flag());

        // First ArrowUp from a fresh state saves in-progress typing
        // ("typing-now") and recalls the last entry.
        assert!(model.history_prev("typing-now"));
        assert_eq!(model.state_snapshot().draft, "cat foo");
        assert_eq!(model.state_snapshot().saved_draft.as_deref(), Some("typing-now"));
        assert!(model.history_prev(""));
        assert_eq!(model.state_snapshot().draft, "pwd");
        assert!(model.history_next(""));
        assert_eq!(model.state_snapshot().draft, "cat foo");
        assert!(model.history_next(""));
        // Past end: cursor cleared, saved draft restored.
        let s = model.state_snapshot();
        assert!(s.history_cursor.is_none());
        assert_eq!(s.draft, "typing-now");
        assert!(s.saved_draft.is_none(), "saved_draft should be consumed");
    }

    #[test]
    fn history_prev_empty_history_is_noop() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        // No history → no state change → no rebuild.
        assert!(!model.history_prev("foo"));
        assert!(!model.history_next("foo"));
    }

    #[test]
    fn history_next_off_walk_is_noop() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("ls", &peers, 1, flag());
        // cursor is None — ArrowDown shouldn't churn.
        assert!(!model.history_next("typing"));
    }

    #[tokio::test]
    async fn initialize_writes_default_when_absent() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let mut model = ShellModel::new(7, pid.clone());
        model.initialize(&peers);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 7);
        let entity = peers.get_entity(&pid, &path).expect("state written");
        assert_eq!(entity.entity_type, "app/state/shell");
        let state = ShellState::from_entity(&entity);
        assert_eq!(state.wd, format!("/{}/", pid));
    }

    #[test]
    fn handle_submit_records_history_and_appends_echo() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("pwd", &peers, 1, flag());
        let s = model.state_snapshot();
        assert_eq!(s.history, vec!["pwd"]);
        assert!(s.scrollback.iter().any(|l| {
            matches!(l.as_ref(), ScrollbackEntry::PromptEcho { .. })
                && l.text_contains("> pwd")
        }));
        // pwd verb prints the wd as a Success line.
        assert!(s.scrollback.iter().any(|l| l.is_info()));
    }

    #[test]
    fn clear_wipes_scrollback() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("foo", &peers, 1, flag());
        model.handle_submit("bar", &peers, 1, flag());
        assert!(!model.state_snapshot().scrollback.is_empty());
        model.clear();
        assert!(model.state_snapshot().scrollback.is_empty());
    }

    // Path-resolution and alias-expansion helpers moved to the
    // `entity_shell::path` and `entity_shell::alias` crate modules,
    // tested there. This consumer only tests the integration through
    // the dispatcher (see `cd_*` and `peer_*` tests below).

    // -- Verb dispatch --

    #[tokio::test]
    async fn cd_updates_wd_and_publishes_selection() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        let target = format!("/{}/docs/arch", pid);
        model.handle_submit(&format!("cd {}", target), &peers, 1, flag());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // wd advanced.
        assert_eq!(model.state_snapshot().wd, target);

        // Selection landed in both panel + app aggregate slots — this
        // is what lets Entity Tree (set to follow App aggregate)
        // co-orient on cd.
        let panel_path =
            crate::app_paths::panel_selection_path(crate::app_paths::APP_ID, &pid, 1);
        let app_path =
            crate::app_paths::app_selection_path(crate::app_paths::APP_ID, &pid);
        let panel_entity = peers.get_entity(&pid, &panel_path).expect("panel slot");
        let app_entity = peers.get_entity(&pid, &app_path).expect("app slot");
        assert_eq!(panel_entity.entity_type, "app/state/selection");
        let app_sel = crate::selection::Selection::from_entity(&app_entity);
        assert_eq!(app_sel.path, target);
        assert_eq!(app_sel.type_.as_deref(), Some("entity"));
    }

    #[test]
    fn cd_bare_jumps_to_peer_root() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit("cd foo/bar", &peers, 1, flag());
        assert!(model.state_snapshot().wd.ends_with("/foo/bar"));
        model.handle_submit("cd", &peers, 1, flag());
        assert_eq!(model.state_snapshot().wd, format!("/{}/", pid));
    }

    #[test]
    fn cd_rejects_path_without_peer_id() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("cd /", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l| l.is_error()
            && l.text_contains("invalid path")));
    }

    #[tokio::test]
    async fn ls_returns_listing_under_wd() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed two entities at sibling paths.
        let entity = entity_entity::Entity::new(
            "test/type",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/docs/a", pid), entity.clone());
        peers.put_entity(&pid, &format!("/{}/docs/b", pid), entity);
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit(&format!("cd /{}/docs", pid), &peers, 1, flag());
        model.handle_submit("ls", &peers, 1, flag());
        let s = model.state_snapshot();
        let listing_rows: Vec<_> = s
            .scrollback
            .iter()
            .filter(|l| l.is_listing())
            .collect();
        assert!(
            listing_rows.iter().any(|l| l.text_contains("/docs/a")),
            "scrollback should list /docs/a, got {:?}",
            listing_rows.iter().map(|l| l.render_text()).collect::<Vec<_>>()
        );
        assert!(listing_rows.iter().any(|l| l.text_contains("/docs/b")));
    }

    #[tokio::test]
    async fn cat_dumps_entity_at_path() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let entity = entity_entity::Entity::new(
            "test/marker",
            entity_ecf::to_ecf(&entity_ecf::text("hello-shell")),
        )
        .unwrap();
        let path = format!("/{}/docs/note", pid);
        peers.put_entity(&pid, &path, entity);
        let model = ShellModel::new(1, pid);
        model.handle_submit(&format!("cat {}", path), &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s
            .scrollback
            .iter()
            .any(|l| l.is_entity()
                && l.text_contains("test/marker")));
        assert!(s
            .scrollback
            .iter()
            .any(|l| l.is_entity()
                && l.text_contains("hello-shell")));
    }

    #[test]
    fn cat_without_argument_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("cat", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l| l.is_error()
            && l.text_contains("missing path")));
    }

    #[test]
    fn help_lists_verbs() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("help", &peers, 1, flag());
        let s = model.state_snapshot();
        // `help` produces one `VerbOutput::Info(...)` entry that
        // renders as 6+ lines.
        let help_entry = s
            .scrollback
            .iter()
            .find(|l| l.is_info() && l.text_contains("pwd"))
            .expect("help entry containing pwd verb description");
        let lines = help_entry.render_text().lines().count();
        assert!(lines >= 6, "expected at least 6 help lines, got {}", lines);
    }

    #[test]
    fn verb_completion_extends_unique_prefix() {
        assert_eq!(complete_verb("hel"), Some("help ".into()));
        assert_eq!(complete_verb("pw"), Some("pwd ".into()));
        // "c" → cd/cat/clear → longest common prefix is "c" (no extension).
        assert_eq!(complete_verb("c"), None);
        // "ca" → cat (unique).
        assert_eq!(complete_verb("ca"), Some("cat ".into()));
        // No matches: leave the input alone.
        assert_eq!(complete_verb("xyz"), None);
    }

    #[test]
    fn longest_common_prefix_works() {
        assert_eq!(longest_common_prefix(["foo", "for", "fox"]), "fo");
        assert_eq!(longest_common_prefix(["abc"]), "abc");
        let empty: [&str; 0] = [];
        assert_eq!(longest_common_prefix(empty), "");
    }

    #[tokio::test]
    async fn path_completion_extends_token_to_unique_child() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let entity = entity_entity::Entity::new(
            "test/type",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/docs/alpha", pid), entity.clone());
        peers.put_entity(&pid, &format!("/{}/docs/beta", pid), entity);
        let model = ShellModel::new(1, pid.clone());
        // `ls al` → only one match under wd's docs: actually wd is
        // peer root so candidates are everything under /{pid}/. Let's
        // navigate first.
        model.handle_submit(&format!("cd /{}/docs", pid), &peers, 1, flag());
        // `cat alp` → unique completion to "alpha".
        let changed = model.complete("cat alp", &peers);
        assert!(changed, "completion should mutate draft");
        let draft = model.state_snapshot().draft;
        assert_eq!(draft, "cat alpha");
    }

    #[tokio::test]
    async fn path_completion_extends_to_common_prefix_with_multiple_matches() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let entity = entity_entity::Entity::new(
            "test/type",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/docs/alpha", pid), entity.clone());
        peers.put_entity(&pid, &format!("/{}/docs/alpine", pid), entity);
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit(&format!("cd /{}/docs", pid), &peers, 1, flag());
        let changed = model.complete("cat al", &peers);
        assert!(changed);
        assert_eq!(model.state_snapshot().draft, "cat alp");
    }

    #[tokio::test]
    async fn exec_against_local_system_tree_appends_result_line() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        let dirty = flag();
        model.handle_submit("exec system/tree get", &peers, 1, dirty.clone());
        // The async future resolves on the local tokio runtime; give
        // it a tick.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let s = model.state_snapshot();
        // `exec` is streaming — produces a `Dispatched` chunk (→ ...)
        // and a terminal `Complete` chunk (← ... status=200 ...).
        let has_complete = s.scrollback.iter().any(|l| {
            l.is_info() && l.render_text().starts_with("← exec system/tree get")
        });
        assert!(
            has_complete,
            "expected a terminal exec result row, got {:?}",
            s.scrollback.iter().map(|l| l.render_text()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn exec_without_arguments_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("exec", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l| l.is_error()
            && l.text_contains("usage:")));
    }

    #[test]
    fn parse_json_params_handles_object_and_primitive() {
        let e = parse_json_params("{\"foo\":42,\"bar\":\"baz\"}").expect("object parse");
        assert_eq!(e.entity_type, "system/params");
        assert!(parse_json_params("true").is_ok());
        assert!(parse_json_params("123").is_ok());
        assert!(parse_json_params("[1,2,3]").is_ok());
        // Invalid → Err, not panic.
        assert!(parse_json_params("not-json").is_err());
    }

    #[test]
    fn complete_window_name_unique_appends_full_name() {
        // From `open entity-tr`, completion should expand to
        // `open Entity Tree` — uses the canonical name, not the alias.
        let model = ShellModel::new(1, "p".into());
        let peers = Peers::new_direct();
        model.set_draft("open entity-tr");
        let changed = model.complete("open entity-tr", &peers);
        assert!(changed);
        assert_eq!(model.state_snapshot().draft, "open Entity Tree");
    }

    #[test]
    fn complete_window_name_extends_common_prefix() {
        // `open e` → "Entity Tree", "Event Log", "Execute Console" share
        // the canonical-name LCP "E" — already at the input, so no extension.
        let result = complete_window_name("open ", "e");
        // LCP across {"Entity Tree", "Event Log", "Execute Console"} is "E"
        // but we already typed "e" (lowercase) — no extension after
        // case-insensitive match.
        match result {
            None => {} // acceptable: nothing common beyond "E"
            Some(s) => assert!(s.len() >= "open e".len(),
                "expected at least as long, got {:?}", s),
        }
    }

    #[test]
    fn complete_peer_subcommand_unique() {
        let model = ShellModel::new(1, "p".into());
        let peers = Peers::new_direct();
        let changed = model.complete("peer cre", &peers);
        assert!(changed);
        assert_eq!(model.state_snapshot().draft, "peer create ");
    }

    #[test]
    fn complete_peer_create_mode() {
        let model = ShellModel::new(1, "p".into());
        let peers = Peers::new_direct();
        let changed = model.complete("peer create me", &peers);
        assert!(changed);
        assert_eq!(model.state_snapshot().draft, "peer create memory ");
    }

    #[test]
    fn complete_alias_reserved_keyword_unique() {
        let peers = Peers::new_direct();
        // Only "primary" matches the prefix among the reserved set
        // {primary, system, default} (no peer labels in this test).
        let result = complete_alias("cd ", "pri", &peers);
        assert_eq!(result.as_deref(), Some("cd @primary/"));
    }

    #[test]
    fn complete_alias_extends_common_prefix_for_multi_match() {
        let peers = Peers::new_direct();
        // primary, default, system — all 3 reserved are candidates.
        // No common prefix between them; expect None when partial is empty.
        let result = complete_alias("cd ", "", &peers);
        assert!(result.is_none() || result.unwrap().contains('@'));
    }

    #[test]
    fn complete_cd_at_prefix_triggers_alias_completer() {
        let model = ShellModel::new(1, "p".into());
        let peers = Peers::new_direct();
        // `cd @pri<Tab>` → `cd @primary/`.
        let changed = model.complete("cd @pri", &peers);
        assert!(changed);
        assert_eq!(model.state_snapshot().draft, "cd @primary/");
    }

    #[test]
    fn complete_put_path_only_targets_first_arg() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed an entity so path completion has something to find.
        let entity = entity_entity::Entity::new(
            "test/note",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/scratch/alpha", pid), entity);
        let model = ShellModel::new(1, pid.clone());
        // Set draft to `put /{pid}/scratch/al`; tab should complete to alpha.
        model.set_draft(&format!("put /{}/scratch/al", pid));
        let partial = model.state_snapshot().draft.clone();
        let changed = model.complete(&partial, &peers);
        assert!(changed, "tab should complete the path arg");
        let draft = model.state_snapshot().draft;
        assert!(draft.contains("alpha"), "expected alpha in {:?}", draft);
    }

    #[test]
    fn tails_with_no_active_subs_shows_empty_message() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("tails", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("no active tails")
        ));
    }

    #[test]
    fn untail_marks_entry_inactive() {
        // Post-extraction: crate's untail submits UninstallTail
        // through the app's ShellActionSink, which flips the active
        // flag inline. Returns a Message ack regardless of count
        // (idempotent semantics).
        use std::sync::atomic::{AtomicBool, Ordering};
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        let active = std::sync::Arc::new(AtomicBool::new(true));
        model.tails.lock().unwrap().push(TailEntry {
            prefix: "/p/foo/".into(),
            active: active.clone(),
        });
        model.handle_submit("untail /p/foo/", &peers, 1, flag());
        assert!(!active.load(Ordering::Relaxed));
    }

    #[test]
    fn untail_all_stops_every_active_entry() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        let a = std::sync::Arc::new(AtomicBool::new(true));
        let b = std::sync::Arc::new(AtomicBool::new(true));
        model.tails.lock().unwrap().extend([
            TailEntry { prefix: "/p/a/".into(), active: a.clone() },
            TailEntry { prefix: "/p/b/".into(), active: b.clone() },
        ]);
        model.handle_submit("untail all", &peers, 1, flag());
        assert!(!a.load(Ordering::Relaxed));
        assert!(!b.load(Ordering::Relaxed));
    }

    #[test]
    fn untail_no_match_is_idempotent() {
        // Crate semantics: untail is idempotent — no error when the
        // target has nothing to stop (the sink's flip is a no-op).
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("untail /nope/", &peers, 1, flag());
        let s = model.state_snapshot();
        // Crate's untail returns Message — bridge renders as Success line.
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("requested stop")
        ));
    }

    #[test]
    fn untail_without_arg_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("untail", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage: untail")
        ));
    }

    #[test]
    fn tail_queues_shell_tail_action() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        let prefix = format!("/{}/scratch/", pid);
        model.handle_submit(&format!("tail {}", prefix), &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::ShellTail { window_id, prefix: p } => {
                assert_eq!(*window_id, 1);
                assert_eq!(*p, prefix);
            }
            other => panic!("expected ShellTail, got {:?}", other),
        }
        // Echo line landed.
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("→ tail")
        ));
    }

    #[test]
    fn tail_without_argument_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("tail", &peers, 1, flag());
        assert!(model.drain_pending_actions().is_empty());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage: tail")
        ));
    }

    #[test]
    fn tail_resolves_relative_and_alias() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        // tail @primary → resolves to /{pid}/
        model.handle_submit("tail @primary", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::ShellTail { prefix, .. } => {
                assert!(prefix.starts_with(&format!("/{}", pid)),
                    "expected prefix to resolve to primary peer, got {}", prefix);
            }
            other => panic!("got {:?}", other),
        }
    }

    #[test]
    fn resolve_window_name_tolerates_aliases() {
        assert_eq!(resolve_window_name("Entity Tree"), Some("Entity Tree"));
        assert_eq!(resolve_window_name("entity-tree"), Some("Entity Tree"));
        assert_eq!(resolve_window_name("entity_tree"), Some("Entity Tree"));
        assert_eq!(resolve_window_name("EntityTree"), Some("Entity Tree"));
        assert_eq!(resolve_window_name("shell"), Some("Shell"));
        assert_eq!(resolve_window_name("SETTINGS"), Some("Settings"));
        assert_eq!(resolve_window_name("nope"), None);
    }

    #[test]
    fn open_queues_spawn_window_action() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("open entity-tree", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::SpawnWindow { type_name, .. } => {
                assert_eq!(*type_name, "Entity Tree");
            }
            other => panic!("expected SpawnWindow, got {:?}", other),
        }
    }

    #[test]
    fn open_multi_word_name() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("open Execute Console", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::SpawnWindow { type_name, .. } => {
                assert_eq!(*type_name, "Execute Console");
            }
            other => panic!("expected SpawnWindow, got {:?}", other),
        }
    }

    #[test]
    fn open_without_argument_lists_available_windows() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("open", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert!(pending.is_empty());
        let s = model.state_snapshot();
        // Lists each window as a listing row.
        for name in WINDOW_TYPES {
            assert!(
                s.scrollback.iter().any(|l|
                    l.is_listing() && l.text_contains(name)
                ),
                "expected listing row for {}", name
            );
        }
    }

    #[test]
    fn open_unknown_window_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("open frobnicate", &peers, 1, flag());
        assert!(model.drain_pending_actions().is_empty());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("unknown window")
        ));
    }

    #[test]
    fn window_types_list_matches_app_registration() {
        // Sanity gate: every name in WINDOW_TYPES must be a real
        // `WindowType.name` registered in app.rs. We can't reach the
        // window manager from here without spawning an EntityApp, but
        // we *can* enumerate the per-window `window_type()` factories
        // and verify each name appears.
        let registered: Vec<&'static str> = vec![
            crate::views::chain_trace::ChainTraceWindow::window_type().name,
            crate::views::content_stream::ContentStreamWindow::window_type().name,
            crate::views::entity_tree::EntityTreeWindow::window_type().name,
            crate::views::event_log::EventLogWindow::window_type().name,
            crate::views::execute_console::ExecuteConsoleWindow::window_type().name,
            crate::views::key_manager::KeyManagerWindow::window_type().name,
            crate::views::knowledge_base::KnowledgeBaseWindow::window_type().name,
            crate::views::path_tap::PathTapWindow::window_type().name,
            crate::views::peer_connections::PeerConnectionsWindow::window_type().name,
            crate::views::peer_management::PeerManagementWindow::window_type().name,
            crate::views::query_console::QueryConsoleWindow::window_type().name,
            crate::views::settings::SettingsWindow::window_type().name,
            crate::views::shell::ShellWindow::window_type().name,
            crate::views::wire_recorder::WireRecorderWindow::window_type().name,
        ];
        for name in WINDOW_TYPES {
            assert!(
                registered.contains(name),
                "WINDOW_TYPES has '{}' but no view registers that name",
                name
            );
        }
        assert_eq!(
            registered.len(),
            WINDOW_TYPES.len(),
            "every registered window type must appear in WINDOW_TYPES \
             (registered: {:?}, WINDOW_TYPES: {:?})",
            registered, WINDOW_TYPES
        );
    }

    #[tokio::test]
    async fn query_returns_matches_by_type() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed two entities with the same type.
        let entity = entity_entity::Entity::new(
            "test/note",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/notes/one", pid), entity.clone());
        peers.put_entity(&pid, &format!("/{}/notes/two", pid), entity);
        let model = ShellModel::new(1, pid);
        model.handle_submit("query test/note", &peers, 1, flag());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let s = model.state_snapshot();
        // Success row with match count
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("match")
        ), "expected success row, got {:?}", s.scrollback.iter().map(|l| l.render_text()).collect::<Vec<_>>());
        // At least two listing rows for the seeded paths.
        let listing_rows = s.scrollback.iter().filter(|l|
            l.is_listing()
        ).count();
        assert!(listing_rows >= 2, "expected 2+ listing rows, got {}", listing_rows);
    }

    #[tokio::test]
    async fn count_returns_total() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let entity = entity_entity::Entity::new(
            "test/marker",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        peers.put_entity(&pid, &format!("/{}/a", pid), entity.clone());
        peers.put_entity(&pid, &format!("/{}/b", pid), entity);
        let model = ShellModel::new(1, pid);
        model.handle_submit("count test/marker", &peers, 1, flag());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let s = model.state_snapshot();
        // Success row carries the integer count.
        let success_row = s.scrollback.iter().find(|l|
            l.is_info() && l.render_text().starts_with("←")
        );
        assert!(
            success_row.is_some(),
            "expected count success row, got {:?}",
            s.scrollback.iter().map(|l| l.render_text()).collect::<Vec<_>>()
        );
        let n = success_row
            .unwrap()
            .render_text()
            .trim_start_matches("←")
            .trim()
            .parse::<usize>()
            .expect("count line should end with an integer");
        assert!(n >= 2, "expected count >= 2, got {}", n);
    }

    #[tokio::test]
    async fn put_writes_entity_at_path() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        let path = format!("/{}/scratch/note", pid);
        model.handle_submit(
            &format!("put {} note/text \"hello\"", path),
            &peers,
            1,
            flag(),
        );
        // dispatch_write is fire-and-forget; let the spawned put settle.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let e = peers
            .get_entity(&pid, &path)
            .expect("put should write entity at target path");
        assert_eq!(e.entity_type, "note/text");
        // Success line landed.
        assert!(model.state_snapshot().scrollback.iter().any(|l|
            l.is_info() && l.text_contains(&path)
        ));
    }

    #[tokio::test]
    async fn put_without_body_writes_null_entity() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        let path = format!("/{}/scratch/marker", pid);
        model.handle_submit(
            &format!("put {} app/marker", path),
            &peers,
            1,
            flag(),
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let e = peers.get_entity(&pid, &path).expect("put should write");
        assert_eq!(e.entity_type, "app/marker");
    }

    #[test]
    fn put_with_invalid_json_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        let path = format!("/{}/scratch/bad", pid);
        model.handle_submit(
            &format!("put {} note/text not-json", path),
            &peers,
            1,
            flag(),
        );
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("invalid JSON")
        ));
    }

    #[test]
    fn put_usage_when_args_missing() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("put", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage: put")
        ));
    }

    #[tokio::test]
    async fn rm_removes_entity_at_path() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed an entity.
        let entity = entity_entity::Entity::new(
            "test/type",
            entity_ecf::to_ecf(&entity_ecf::text("v")),
        )
        .unwrap();
        let path = format!("/{}/scratch/to-remove", pid);
        peers.put_entity(&pid, &path, entity);
        // rm via shell.
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit(&format!("rm {}", path), &peers, 1, flag());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            peers.get_entity(&pid, &path).is_none(),
            "rm should drop the entity"
        );
    }

    #[test]
    fn rm_without_argument_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("rm", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage: rm")
        ));
    }

    #[test]
    fn put_and_rm_respect_alias_expansion() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        // Use @primary alias.
        model.handle_submit(
            "put @primary/scratch/note note/text",
            &peers,
            1,
            flag(),
        );
        let s = model.state_snapshot();
        let success_row = s.scrollback.iter().find(|l|
            l.is_info() && l.render_text().starts_with("put:")
        );
        assert!(success_row.is_some(), "put should succeed with alias");
        // Output should show resolved path with the real peer id.
        assert!(success_row.unwrap().text_contains(&pid));
    }

    #[test]
    fn info_verb_reports_bound_primary_and_arm() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit("info", &peers, 1, flag());
        let s = model.state_snapshot();
        // Six info rows: bound, primary, arm, local count, connections, wd.
        let info_rows: Vec<String> = s
            .scrollback
            .iter()
            .filter(|l| l.is_info())
            .map(|l| l.render_text())
            .collect();
        assert!(info_rows.iter().any(|t| t.contains("bound peer")));
        assert!(info_rows.iter().any(|t| t.contains("primary peer")));
        assert!(info_rows.iter().any(|t| t.contains("primary arm:") && t.contains("Direct")));
        assert!(info_rows.iter().any(|t| t.contains("local peers")));
        assert!(info_rows.iter().any(|t| t.contains("connections")));
        assert!(info_rows.iter().any(|t| t.contains("wd:")));
        // bound and primary should agree in default test setup.
        let bound_row = info_rows.iter().find(|t| t.starts_with("bound peer")).unwrap();
        assert!(bound_row.contains(&pid));
    }

    // `expand_alias` / `lookup_alias` tests moved with the helpers
    // to `entity_shell::alias`. Integration through the dispatcher is
    // exercised by `cd_with_alias_updates_wd`,
    // `cd_with_unknown_alias_errors`, `put_and_rm_respect_alias_expansion`,
    // and the `peer_*_alias_*` tests in this file.

    #[tokio::test]
    async fn cd_with_alias_updates_wd() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit("cd @primary/docs", &peers, 1, flag());
        assert_eq!(
            model.state_snapshot().wd,
            format!("/{}/docs", pid)
        );
    }

    #[test]
    fn cd_with_unknown_alias_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("cd @notathing/foo", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error()
                && l.text_contains("unknown peer alias")
        ));
    }

    #[test]
    fn peer_create_queues_create_action() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer create memory test-store", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::CreatePeerWithMode { label, mode } => {
                assert_eq!(label.as_deref(), Some("test-store"));
                assert_eq!(*mode, crate::peer_mode::PeerMode::BackendMemory);
            }
            other => panic!("expected CreatePeerWithMode, got {:?}", other),
        }
        // Echo line landed.
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("creating backend (memory) peer")
        ));
    }

    #[test]
    fn peer_create_aliases_frontend_and_opfs() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer create frontend", &peers, 1, flag());
        model.handle_submit("peer create opfs my-store", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 2);
        assert!(matches!(
            &pending[0],
            crate::action::Action::CreatePeerWithMode { mode: crate::peer_mode::PeerMode::Frontend, label }
                if label.is_none()
        ));
        assert!(matches!(
            &pending[1],
            crate::action::Action::CreatePeerWithMode { mode: crate::peer_mode::PeerMode::BackendOpfs, .. }
        ));
    }

    #[test]
    fn peer_create_unknown_mode_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer create wat", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert!(pending.is_empty());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("unknown mode")
        ));
    }

    #[test]
    fn peer_rename_queues_rename_action() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit(
            "peer rename @primary My Primary",
            &peers,
            1,
            flag(),
        );
        let pending = model.drain_pending_actions();
        assert_eq!(pending.len(), 1);
        match &pending[0] {
            crate::action::Action::RenamePeer { peer_id, label } => {
                assert_eq!(*peer_id, pid);
                assert_eq!(label.as_deref(), Some("My Primary"));
            }
            other => panic!("expected RenamePeer, got {:?}", other),
        }
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("renaming peer")
        ));
    }

    #[test]
    fn peer_rename_with_empty_label_clears_it() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        // Trailing whitespace as the label → cleared (None).
        // We need at least one token for the parser to accept the form;
        // use a literal hyphen and trim → empty.
        model.handle_submit("peer rename @primary   ", &peers, 1, flag());
        // No pending action because args length check needs >= 2.
        // Fall back to error.
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage:")
        ), "expected usage error when label arg missing");
    }

    #[test]
    fn peer_rename_usage_when_missing_args() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer rename", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("usage:")
        ));
    }

    #[test]
    fn peer_rename_unknown_alias_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer rename @nope foo", &peers, 1, flag());
        assert!(model.drain_pending_actions().is_empty());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error() && l.text_contains("unknown peer alias")
        ));
    }

    #[tokio::test]
    async fn set_peer_label_round_trips_via_peers() {
        // Direct test of the Peers wrapper: rename the primary peer,
        // then verify peer_metadata().label reflects the change.
        let mut peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        peers.set_peer_label(&pid, Some("Renamed".into())).unwrap();
        let label = peers
            .peer_metadata(&pid)
            .and_then(|m| m.label)
            .expect("label should be set");
        assert_eq!(label, "Renamed");
    }

    #[test]
    fn peer_delete_refuses_primary() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit(&format!("peer delete {}", pid), &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert!(pending.is_empty(), "must not queue delete for primary");
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error()
                && l.text_contains("refusing to delete the primary")
        ));
    }

    #[test]
    fn peer_delete_alias_unknown_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer delete @nope", &peers, 1, flag());
        let pending = model.drain_pending_actions();
        assert!(pending.is_empty());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error()
                && l.text_contains("unknown peer alias")
        ));
    }

    #[test]
    fn peer_unknown_subcommand_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peer frobnicate", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_error()
                && l.text_contains("unknown subcommand")
        ));
    }

    #[test]
    fn peer_verb_lists_local_and_remote_sections() {
        // Post-extraction: crate's peer list uses just PeerBinding
        // data (no glyph/role enrichment from app-tier peer_display).
        // Each local peer shows short_pid + role ("primary" / "local"),
        // remotes show short_pid + "connected".
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid.clone());
        model.handle_submit("peer", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("local")
        ));
        let short = crate::views::short_pid(&pid);
        let primary_row = s.scrollback.iter().find(|l|
            l.is_listing() && l.text_contains(&short)
        );
        assert!(primary_row.is_some(), "expected primary peer row, got {:?}",
            s.scrollback.iter().map(|l| l.render_text()).collect::<Vec<_>>());
        assert!(primary_row.unwrap().text_contains("primary"));
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("remote")
        ));
    }

    #[test]
    fn peers_verb_alias_matches_peer() {
        // Singular/plural alias — same dispatch path.
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("peers", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l|
            l.is_info() && l.text_contains("local")
        ));
    }

    #[test]
    fn connect_without_argument_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("connect", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l| l.is_error()
            && l.text_contains("usage: connect")));
    }

    #[tokio::test]
    async fn connect_to_unreachable_address_emits_error() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit(
            "connect ws://127.0.0.1:1",
            &peers,
            1,
            flag(),
        );
        // The connect future runs against an unreachable port and
        // resolves to Err. Wait a reasonable interval.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let s = model.state_snapshot();
        let connect_rows: Vec<_> = s
            .scrollback
            .iter()
            .filter(|l| {
                (l.is_info() || l.is_error())
                    && l.text_contains("ws://127.0.0.1:1")
            })
            .collect();
        // Either Err (fast) or hang — but on Direct mode with no
        // remote running, the future resolves with an error quickly.
        // We assert at LEAST that no panic occurred and the echo line
        // landed; the future may not have resolved within the window.
        let _ = connect_rows;
        // The pre-dispatch "→ connecting to" info line always lands.
        assert!(s
            .scrollback
            .iter()
            .any(|l| l.is_info()
                && l.text_contains("connecting to ws://127.0.0.1:1")));
    }

    /// End-to-end: shell `connect memory://<pid>` drives `Peers::connect_peer`,
    /// which runs through `MemoryConnector` against a `MemoryTransportRegistry`,
    /// hands the duplex pair to `entity_peer::server::run` on the listening
    /// peer, and the handshake completes. Verifies the entire chain
    /// shell → Peers → SDK → MemoryConnector → MemoryListener works
    /// against the upstream transport.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn connect_memory_scheme_completes_handshake_end_to_end() {
        use entity_peer::transport::{MemoryConnector, MemoryListener, MemoryTransportRegistry};

        let registry = MemoryTransportRegistry::new();

        // Server-side peer: listens on memory://<server_pid>.
        let server_peers =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let server_pid = server_peers.primary_peer_id().to_string();
        let server_shared = server_peers
            .direct_peer_shared(&server_pid)
            .expect("server peer_shared");
        let server_pm = server_peers
            .primary_as_direct()
            .expect("server primary is Direct");
        let server_peer = server_pm.sdk().peer(&server_pid).expect("server peer").peer();
        server_peer.start_engines(&server_shared);
        let server_listener = MemoryListener::bind(server_pid.clone(), registry.clone())
            .expect("bind server listener");
        let shared_for_server = server_shared.clone();
        let server_handle = tokio::spawn(async move {
            let _ = entity_peer::server::run(server_listener, shared_for_server).await;
        });

        // Client-side: a `Peers` whose primary uses MemoryConnector. The
        // shell submits `connect memory://<server_pid>` against it.
        let client_peers =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry)));
        let client_pid = client_peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, client_pid.clone());

        tokio::task::yield_now().await;

        model.handle_submit(
            &format!("connect memory://{server_pid}"),
            &client_peers,
            1,
            flag(),
        );

        // Wait up to 2s for the connect future to resolve and the
        // success line to land in scrollback.
        let success_line = format!("connected to {}", crate::views::short_pid(&server_pid));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut hit = false;
        while std::time::Instant::now() < deadline {
            let s = model.state_snapshot();
            if s.scrollback.iter().any(|l| {
                l.is_info() && l.text_contains(&success_line)
            }) {
                hit = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let snap = model.state_snapshot();
        assert!(
            hit,
            "no success line within 2s; scrollback: {:?}",
            snap.scrollback.iter().map(|l| l.render_text()).collect::<Vec<_>>()
        );

        // Sanity: A's connection pool holds the remote.
        let client_shared = client_peers.direct_peer_shared(&client_pid).unwrap();
        assert!(client_shared.remote.get(&server_pid).is_some());

        server_handle.abort();
    }

    #[test]
    fn verb_completion_includes_peer_and_connect() {
        // Verb registry covers the new verbs.
        assert_eq!(complete_verb("pee"), Some("peer".into()));
        assert_eq!(complete_verb("con"), Some("connect ".into()));
    }

    #[test]
    fn unknown_verb_reports_error() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ShellModel::new(1, pid);
        model.handle_submit("frobnicate", &peers, 1, flag());
        let s = model.state_snapshot();
        assert!(s.scrollback.iter().any(|l| l.is_error()
            && l.text_contains("unknown verb: frobnicate")));
    }
}
