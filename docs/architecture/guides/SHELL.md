# Entity Shell — Verb Reference

Verbs live in the extracted `entity_shell` crate at
`../entity-core-rust/bindings/shell/`; this repo is a consumer.
DOM renderer matches typed `ScrollbackEntry` per-variant; scrollback
is in-memory only (history persists). 20 dispatcher-handled verbs
(Tier C + Tier E + `inspect`) plus the embedding-only `clear`.

The entity shell is the 10th window — a text REPL over the same
substrate the DOM windows use. It's positioned as the leading-edge
feature surface: new identity/capability/role features land here as
shell verbs first, then GUI panels become thin presentation
adapters. Mirrors workbench-go's `shellcmd` direction.

See also:
- `docs/plans/LONG-TERM-SHELL-PLAN.md` — strategy
- `docs/archive/handoffs/SHELL-EXTRACTION-PLAN.md` — Phase 4–6 plan
  (Tier E expansion, standalone binary, persistence helpers)
- `docs/archive/handoffs/NEXT-SESSION-HANDOFF.md` — current state
- `../entity-core-rust/bindings/shell/src/verbs/` — verb
  implementations (crate-side, one file per verb)
- `src/views/shell/{model,binding,mod,output}.rs` — this app's
  consumer: state mirror,
  `PeerBinding`/`SelectionSink`/`AppActionSink` impls,
  typed `ScrollbackEntry`, tab completion against app state

## Keybindings

| Key | Action |
|---|---|
| `Enter` | Submit current line |
| `↑` / `↓` | Walk command history (saves in-progress draft on first `↑`, restores on `↓` past bottom) |
| `Tab` | Complete verb / path / window-name / `@alias`. Single-cycle replace; longest-common-prefix for multi-match |
| `Ctrl-L` | Clear scrollback (equivalent to `clear` verb) |

History and scrollback persist to the tree at the window's state
path (`app/state/shell` entity at
`/{peer_id}/app/entity-browser/workspace/windows/{id}/state`).
Scrollback is bounded; history is fully retained.

## The peer-centric model

**Every shell is bound to exactly one peer.** That binding —
`ShellWindow.peer_id`, set at spawn — is THE peer whose tree this
shell reads, writes, queries, and executes against. The path's
first segment is part of an address within the bound peer's tree,
NOT a peer-routing decision.

```
shell on peer A:  cat /B/foo   →  peers.get_entity("A", "/B/foo")
                                  (A's local mirror of B's foo)
```

When peer A has synced with peer B, A's tree contains B's content
under `/{B_pid}/...`. That's A's stored knowledge of B. To see B's
actual tree, **open a shell bound to B**. Looking at the same path
through two shells (one on A, one on B) is the canonical "are
these peers in sync" check — divergence is visible directly.

This differs from workbench-go, where a session has a current peer
that switches. Here, switching peers means opening another window.
The per-window peer binding is what makes multi-peer test scenarios
work naturally: a test opens shells on N peers and drives each
independently, with full isolation.

### What the bound peer changes

- **Tree ops** (`cat`, `ls`, `set`, `rm`, `query`, `count`, `tail`,
  `exec`) — always route against the bound peer.
- **`cd`** — purely navigational within the bound peer's tree.
  `cd /B/foo` from a shell bound to A means "navigate A's tree to
  the /B/foo address" (where A may or may not have data mirrored).
- **`open`** — defaults to the bound peer. Use
  `open <window> @<peer>` to spawn on a different peer (or open
  via the palette).

### What stays system/primary-scoped

- **`peer list` / `peer create` / `peer delete` / `peer rename`** —
  peer lifecycle is app-level, not per-peer-tree.

### What dispatches from the bound peer

- **`connect <addr>`** — connects via the **shell's bound peer**,
  not the primary. This is what makes `xworker://<other-pid>` from
  a backend-bound shell meaningful (only that backend Worker has
  `MessagePortConnector` wired). For primary-bound shells the
  behavior is equivalent to the prior "always primary" form.
  Address schemes: `ws://`, `wss://`, `xworker://<peer-id>` (in
  WASM Workers with a control port), `memory://<peer-id>` (native
  in-process tests).

## Working directory

`wd` is a tree path of the form `/<peer_id>/...`. Default is the
bound peer's root (`/{peer_id}/`). `cd` updates `wd` and publishes
a `Selection` to the app-aggregate panel-source slot **on the
bound peer's context** — Entity Tree windows bound to the same
peer (set to "follow: App aggregate") co-orient automatically.
That's the cross-window interop demo: text command moves a
graphical panel.

## Aliases

`@alias` prefixes resolve to a peer id. Lookup order:

1. **`@self`** — the bound peer (this window's peer). Use this when
   you've `cd`'d to another peer's mirror and want to refer back to
   your own tree explicitly.
2. **`@primary` / `@system` / `@default`** — the **primary peer**
   (the session-level system peer). May differ from `@self` when
   the shell is bound to a backend peer.
3. **Label**: case-insensitive match against `PeerMetadata.label`
   over local + connected peers.
4. **Peer-id prefix**: first peer whose id starts with the alias.

`@alias` is also accepted on its own (without `/...`) wherever a
peer ref is expected (`peer delete @foo`, `peer rename @foo bar`,
`open Shell @primary`).

## Path resolution

- **Absolute** — starts with `/`, kept as-is.
- **`@alias[/...]`** — expanded to `/{peer_id}[/...]` first, then
  treated as absolute.
- **Relative** — resolved against `wd`. Standard `.` / `..`
  semantics. `~` is not (yet) bound.

The peer to operate on is **always `self.peer_id` (the bound
peer)** — the path's first segment is part of the address within
that tree. `path_peer_id(target)` is used only for path-shape
validation, never for routing.

## Verbs

### Navigation

| Verb | Form | Notes |
|---|---|---|
| `pwd` | `pwd` | Print `wd`. |
| `cd` | `cd [path]` | Bare `cd` jumps to the bound peer root (`/{peer_id}/`). Publishes a `Selection` to the app-aggregate slot. |
| `ls` | `ls [path]` | Lists direct children under path (or `wd`). Listing rows show full paths. |
| `cat` | `cat <path>` | Show the entity at path: type, size, decoded body. |
| `tree` | `tree [path] [--depth N]` | Recursive listing. Default depth bounded at 100 levels; pass `--depth N` to limit. |

### Entity manipulation

| Verb | Form | Notes |
|---|---|---|
| `put` | `put <path> <type> [<json-body>]` | Write an entity. Empty body → CBOR null. JSON body encoded via `parse_json_to_ecf`. Entity type is whatever you type — no client-side type-registry validation. Renamed from `set` in Phase 2 per guide §4.5 (matches EXTENSION-TREE primitive). |
| `rm` | `rm <path>` | Remove an entity (`dispatch_remove`). |

### System / query

| Verb | Form | Notes |
|---|---|---|
| `info` | `info` | Bound peer, primary peer, primary arm (Direct/Worker), local count, connection count, `wd`. Renamed from `status` in Phase 2 per guide §4.5 (cross-impl 9-core convergence). |
| `query` | `query [type]` | Find entities (optional type filter), limit 50. Routes against the bound peer. |
| `count` | `count [type]` | Count matching entities, no limit. |

### Handler execution

| Verb | Form | Notes |
|---|---|---|
| `exec` | `exec <handler_uri> <operation> [<json-params>]` | Calls `ops::execute`. Routes against the `wd` peer. Result shows as Success/Error scrollback line. |

### Peer lifecycle

| Verb | Form | Notes |
|---|---|---|
| `peer` (or `peer list`) | `peer [list]` | List local peers (with role/glyph/short-pid/label) and connected remotes. |
| `peer create` | `peer create <mode> [<label>]` | Mode: `frontend` / `memory` / `opfs`. Spawns via `Action::CreatePeerWithMode`. |
| `peer delete` | `peer delete <@alias-or-pid>` | Refuses to delete the primary peer. Spawns `Action::DeletePeer`. |
| `peer rename` | `peer rename <@alias-or-pid> <new-label>` | Empty label clears. Spawns `Action::RenamePeer`. |

### Remote connections

| Verb | Form | Notes |
|---|---|---|
| `connect` | `connect <ws://addr \| wss://addr \| xworker://peer-id \| memory://peer-id>` | Open a connection from the shell's **bound peer**. Scheme picks the connector: `ws`/`wss` → browser WebSocket / native WS; `xworker` → cross-Worker MessagePort (WASM, requires control port at boot); `memory` → in-process duplex (native). Adds the remote to the connections registry on success. |
| `disconnect` | `disconnect <peer-or-alias>` | Per guide §4.7: idempotent against not-connected (returns Message, not error). **Phase 2 limitation:** removes from app-tier connections registry only — SDK-level transport teardown is upstream TODO (no `Peers::disconnect_peer` symmetric to `connect_peer` yet). |

### Windows

| Verb | Form | Notes |
|---|---|---|
| `open` | `open [<window>] [@peer]` | Bare `open` lists registered window types. Accepts canonical names (`"Entity Tree"`), aliases (`entity-tree`, `EntityTree`, `entity_tree`), case-insensitive. Defaults to the **bound peer**; pass a trailing `@<peer-alias>` to spawn on a different peer (e.g. `open Shell @primary`). System-scope windows redirect via the spawn handler. |

### Live subscriptions

| Verb | Form | Notes |
|---|---|---|
| `tail` | `tail <path-prefix>` | Stream live `Put`/`Remove`/`Resync` events under prefix into scrollback. Trailing-slash semantics matter: `/p/foo/` matches descendants of `foo`, `/p/foo` would also match `/p/foobar`. The shell normalizes a trailing slash. Subscription lives on the window's `WindowWatch`; closing the window stops it. |
| `tails` | `tails` | List active tail subscriptions and their state (`active` / `stopped`). |
| `untail` | `untail <prefix \| all>` | Soft-cancel: flips an `active` flag so the callback no-ops. The underlying subscription handle stays parked on the watch until the window closes (no plumbing to retrieve it; harmless dispatch waste). |

### Inspect (diagnostics)

The `inspect` verb is the diagnostics backbone — pure substrate reads
that surface observability state without dispatching into handlers.
All sub-ops route against the **bound peer**. The Chain Trace window
(11th window, Peer-scoped) is the visual companion to `inspect chain`.

| Verb | Form | Notes |
|---|---|---|
| `inspect help` | `inspect help` | List sub-ops + one-line summaries. |
| `inspect chain` | `inspect chain <chain_id>` | Walk a continuation chain: lists `/system/continuation/**` entries plus matching `/system/runtime/chain-errors/**` markers for that `chain_id`. |
| `inspect under` | `inspect under <prefix>` | Enumerate every binding under `prefix` on the bound peer. Relative path joins with `wd`. |
| `inspect errors` | `inspect errors` | All chain-error markers on the bound peer, grouped by `chain_id`. Fresh peers report `(no chain-error markers)`. |
| `inspect entity` | `inspect entity <path>` | Read the entity at `path` and render type / hash / size + CBOR-pretty body. Empty path branches to `(no entity at ...)`. |
| `inspect dump` | `inspect dump <hash-hex> [--paths]` | Look up entity by content hash. `--paths` walks the tree to enumerate every binding that resolves to this hash (O(N), opt-in). |
| `inspect find` | `inspect find <substring> [--limit N]` | Path substring search across the bound peer's tree. Refuses empty substring. `--limit` defaults to a sane cap. |

Render-policy redaction (`src/render_policy.rs`, conservative-default
Sensitive per audit §2.6) applies to the Chain Trace + Path Tap windows'
body rendering, not to the shell verb output — verbs print what the
store returns.

The path-bound primitives (entity reader, path enumerator, hash
lookup) are reachable against the existing SDK. The **live
event taps** (dispatch / wire / binding) ship via the
v9 wire protocol + `entity_sdk::PeerContext::install_inspect_sink`
(Direct arm) / `WorkerProxy::install_inspect_sink` (Worker arm). The
**Path Tap**
window (12th overall, peer-scoped) consumes the dispatch stream;
open via `open Path Tap` on any peer. Wire Recorder + Content Stream
windows (for the other two fact kinds) are not yet built.

### Misc

| Verb | Form | Notes |
|---|---|---|
| `help` | `help` | Inline cheatsheet of all verbs. |
| `clear` | `clear` | Wipe scrollback (also `Ctrl-L`). |

## Output kinds

Scrollback lines carry a `ShellLineKind` that maps to a CSS class:

| Kind | Meaning |
|---|---|
| `PromptEcho` | The submitted line (dim). |
| `Info` | Default text; status, headers, `→` dispatch arrows. |
| `Success` | `←` completion, command success. |
| `Error` | `✗` failures, validation errors. |
| `Listing` | `ls` rows, peer-list rows, tail events. |
| `Entity` | `cat` output. |

The async verbs (`query`, `count`, `connect`, `exec`) echo a `→`
"dispatched" line synchronously and append the `←` / `✗` result
line when the future resolves. The window watch dirty flag fires
on completion, so the scrollback re-renders without user input.

## Adding a verb

Verbs live in the crate (`../entity-core-rust/bindings/shell/`), not
in this repo. Steps:

1. Add a new file under
   `../entity-core-rust/bindings/shell/src/verbs/<name>.rs`
   following the existing files as templates (sync ops return
   `Result<VerbOutput, ShellError>`; streaming ops spawn a
   producer task on the `spawn` callback and return
   `VerbOutput::Lines(rx)` / `VerbOutput::Dispatch(rx)`).
2. Register in `bindings/shell/src/verbs/mod.rs` + add the verb
   token to `dispatcher::VERBS` + route in `dispatcher::dispatch`.
3. If the verb needs a new `PeerBinding` method, add it to
   `binding.rs` (and implement on this app's side in
   `src/views/shell/binding.rs::PeersBinding`).
4. If the verb fires an app-level follow-up action (window spawn,
   peer lifecycle, subscription install), extend
   `action::ShellRequest` + handle in `binding.rs::ShellActionSink`.
5. Update this doc.
6. Add an e2e phase exercising the verb in `tests/e2e_worker.rs`
   — see `feedback_e2e_must_exercise_new_features.md` (memory).
   Hardcoded verb-exercise phases live there; a new verb that
   doesn't get a phase risks silently regressing.

This app's tab-completion list is built from
`entity_shell::dispatcher::VERBS` automatically — no additional
registration here.

## Testing verbs from e2e

`tests/e2e_worker.rs` provides shell-driven helpers that collapse
the "find shadow-DOM section, set input value, dispatch Enter,
sleep, read scrollback" pattern into one line per verb:

```rust
let sb = shell_submit(&client, "pwd", 200).await?;
assert!(sb.contains('/'));

// Or with auto-asserts on expected substrings:
shell_expect(&client, "status", &["bound peer", "primary arm"], 200).await?;
```

Phase 2e in `worker_boots_and_opens_all_windows` demonstrates this:
each new verb adds two lines (submit + assert) rather than the
30–60 lines of inline JS that DOM-click testing required. When
adding a verb, append a smoke check to Phase 2e.

`settle_ms` guidance:
- Sync L0 verbs (`pwd`, `ls`, `cat`, `status`, `set`, `rm`, `peer
  list`, `open`): **200–300ms**.
- Async verbs spawning a future (`query`, `count`, `connect`,
  `exec`): **800–1500ms** depending on what the future does.
- Cross-window propagation (`open` spawning a window, `cd`
  publishing a Selection): **800ms** (two-frame propagation).

## Async verb pattern

Async verbs (`query`, `count`, `connect`, `exec`) follow this shape:

```rust
pub(super) fn verb_foo(
    &self,
    args: &[&str],
    peers: &Peers,
    dirty: crate::window_watch::DirtyFlag,
) {
    // 1. Parse/validate args; on error, push Error line and return.
    // 2. Push an Info "→ foo ..." dispatch line synchronously.
    // 3. Build future from peers.* / ops::*.
    // 4. Clone scrollback + dirty into the task.
    // 5. spawn_local (wasm) / rt.spawn (native) the result handler.
    //    Handler pushes Success/Error line, then dirty.mark().
}
```

`DirtyFlag` is the watch's flag handle. Marking it triggers the
next render-loop tick to rebuild this window's DOM only — no
global repaint.

## Pending-action queue

Verbs that need to fire an app-level `Action` (peer create/delete/
rename, spawn window, tail install) push to `model.pending_out`
instead of touching the renderer queue directly. The window
controller drains the queue in `render_dom` and pushes into
`ctx.actions`. Two-frame propagation; cheap.

This indirection lets the model stay testable without a DOM
context: tests submit a verb and inspect the pending queue.

## Known limitations / deferred

- **`exec` JSON parsing**: positional only; `params` is `json-params`
  joined from `args[2..]` (so quoting whitespace inside JSON works
  but flag-style `--foo bar` does not).
- **Tab completion**: single-cycle replace + longest-common-prefix.
  No menu UI (Stage E follow-up).
- **No pipes / redirection**: verb composition is out of scope for v1.
- **No multi-line input**: `<input type=text>` is single-line.
  Multi-line lands when a verb needs it.
- **No standalone `entity-shell` binary yet**: planned as Phase 5
  of `SHELL-EXTRACTION-PLAN.md` — readline REPL + one-shot
  CLI mode, validates crate portability from a headless,
  non-DOM frontend.
- **No cross-impl session replay**: gated on arch A2 (action-vocab
  extension process).
- **Path tab completion is L0**: uses `peers.tree_listing` directly;
  capability-checked listing would change behavior under partial
  access.

## Files

### Consumer (this repo)

| File | Purpose |
|---|---|
| `src/views/shell/mod.rs` | `ShellWindow`, `WindowType`, action dispatch, `install_tail` |
| `src/views/shell/model.rs` | `ShellModel`, `ShellState`, scrollback drain, tab completion |
| `src/views/shell/binding.rs` | `PeersBinding` / `PanelSelectionSink` / `ShellActionSink` impls of the crate's traits |
| `src/views/shell/output.rs` | `ShellOutput`, `ScrollbackEntry` typed enum, test helpers |
| `src/dom/shell.rs` | DOM renderer: per-variant `ScrollbackEntry` matching, prompt header, `<pre>` scrollback, `<input>` with keydown handlers |
| `src/ops/execute.rs` | `ExecuteRequest` / `ExecuteResponse` / `execute()` — Stage D factoring, shared with Execute Console |

### Crate (upstream — `../entity-core-rust/bindings/shell/`)

| File | Purpose |
|---|---|
| `src/lib.rs` | Public surface: `Shell`, `PeerBinding`, `SelectionSink`, `AppActionSink`, `VerbOutput`, `ShellError`, chunk types |
| `src/dispatcher.rs` | `VERBS` const + `parse` + `dispatch` (verb-parser tier, dispatcher-tier alias resolution) |
| `src/shell.rs` | `Shell` session state (peer_id + wd) |
| `src/binding.rs` | `PeerBinding` trait + `TreeListingEntry` / `EntityRead` / `QueryResults` types |
| `src/sink.rs` | `SelectionSink` trait |
| `src/action.rs` | `AppActionSink` trait + `ShellRequest` enum (lifecycle follow-ups) |
| `src/result.rs` | `VerbOutput` variants + `ShellError` + `StreamChunk` / `DispatchChunk` |
| `src/alias.rs` | `@alias` expansion (`expand`/`lookup`/`resolve_pid`/`reverse_lookup`) |
| `src/path.rs` | Pure path helpers (`peer_id_of`/`resolve`/`normalize`) |
| `src/verbs/*.rs` | Verb implementations (one file per verb) |
