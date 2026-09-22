//! `file_offer` — a browser peer as the **serving** side of a file transfer.
//!
//! Everything else in the transfer path already exists and is transport-
//! agnostic: upload (`dom/file_transfer.rs` → `Action::UploadFile` → op
//! `write`), browse (`list`), download-to-device (`ops::download`). What a
//! browser could not do is *be the counterpart* — all three target
//! `entity://{peer}/local/files`, and `entity-local-files` is
//! `#![cfg(not(target_arch = "wasm32"))]`, mounted by the Tauri backend. Two
//! browsers had nobody to receive.
//!
//! The receiving surface is a different handler, not new code: `system/content`
//! (`ingest` §6.3 / `get` §6.2) is registered on **every** peer — it rides the
//! `content` feature, which we enable for the native peer and the wasm SDK
//! alike. It is chunked, hash-addressed and frame-budgeted. So this module is a
//! swap and a name:
//!
//! - **Offer (sender).** Chunk the bytes locally (`entity_content`, §3.2 fixed),
//!   `ingest` the blob + chunks into our own `system/content` namespace, then
//!   publish a small **manifest** entity in *our* app namespace
//!   (`app/entity-browser/offers/{blob-hex}`) carrying name/size/blob/from.
//!   Content is hash-addressed; the manifest is what gives a hash a filename.
//! - **Pull (receiver).** List the sender's offers over `system/tree`, then walk
//!   the blob's closure with `system/content:get` and `reassemble` the bytes.
//!
//! **Pull, not push** (`PLAN-2026-08-16` §2b decision 1): the sender ingests
//! into its *own* store and advertises; the receiver fetches. It needs only a
//! **read** grant rather than letting a stranger write into your tree, reuses
//! the reassembly `ops::download` already does, and the accept/decline moment
//! comes for free.
//!
//! **Transport-immaterial by construction.** Every call here goes through
//! `Peers::execute`, which dispatches over whatever pooled connection reaches
//! the target — a WebRTC data channel exactly as readily as a WebSocket. There
//! is no branch on transport in this file, and there must never be one.
//!
//! **Arm-agnostic by construction.** Every call goes through
//! [`DispatchHandle`], which branches on the arm exactly once, in one file —
//! there is no `store()`, no `PeerContext`, no `ContentStore` off the peer
//! here, so a Worker-arm peer offers files the same way a Direct-arm one does.
//! The only `ContentStore` in this module is a private in-memory scratch store
//! used to chunk and to reassemble. The handle is also what makes these flows
//! *spawnable*: they are sequential (list → read each, blob → its chunks), so
//! they keep dispatching after their first `.await`, which a borrowed `&Peers`
//! cannot survive.
//!
//! ## The closure walk (why `pull` is two round-trips, not one)
//!
//! `system/content:get` returns exactly the hashes you ask for. The blob
//! manifest names its chunks, so the receiver must fetch the blob first, decode
//! its chunk list, then fetch the chunks — in batches, because the response has
//! a frame budget (16 MiB default) and the handler moves the overflow to
//! `missing` for the requester to re-ask. Upstream has this sequencer
//! (`entity_content::ensure_closure`), but it takes a `&dyn Dispatcher`, which
//! only the Direct arm can produce; [`pull_offer`] is the same walk over
//! `Peers::execute` so both arms are served. `ops::download`'s "chunks may not
//! be inlined — the follow-up is a `system/content:get` round-trip" note names
//! exactly this loop.

use std::sync::Arc;

use entity_content::{blob_chunk_hashes, create_blob_fixed, reassemble, GET_BATCH_SIZE};
use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_store::{ContentStore, MemoryContentStore};

use crate::app_paths::{offer_path, offers_prefix, APP_ID};
use crate::dispatch_handle::DispatchHandle;
use crate::remote_read::{empty_params, remote_execute, resource_opts};

/// Entity type of an offer manifest — our namespace, our shape
/// (`AGENTS.md`: path namespaces stay app-tier, not SDK).
pub const OFFER_TYPE: &str = "app/entity-browser/file-offer";

/// The `system/content` namespace offered files are ingested into. One
/// namespace for all offers: the manifest, not the namespace, is what
/// distinguishes files.
pub const NAMESPACE: &str = "files";

/// Fixed chunk size (§3.2). 256 KiB keeps a small file to one or two chunks
/// while leaving ~60 chunks per `get` batch inside the 16 MiB frame budget.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// The largest file this peer will offer — **a stated limit, refused at the
/// door, rather than an unstated one discovered as a dead tab.**
///
/// It is not a protocol bound. The *pull* side is already streaming-shaped
/// (`GET_BATCH_SIZE` = 16 chunks ≈ 4 MiB per response, well inside the 16 MiB
/// frame budget), so what sets the ceiling is the **offer** side, and it is
/// memory: [`chunk_bytes`] holds the whole file and a full set of chunk entities
/// at once, [`ingest_params`] then encodes *every* chunk into one CBOR envelope,
/// and the picker handed us a `Vec` copied out of a JS `ArrayBuffer` before any
/// of that. That is ~4 live copies at the peak, in a wasm linear memory a
/// WebView may refuse to grow (`AGENTS.md`: WebKit denying `memory.grow` under
/// pressure) — and the failure mode of exceeding it is an OOM abort or a frozen
/// tab, neither of which tells the user what went wrong.
///
/// 16 MiB therefore buys a ~64 MiB peak, which is survivable everywhere we run.
/// **Raising it is a streaming-ingest change, not a bigger number**: ingest the
/// chunks in batches the way the pull already fetches them, and the constant
/// stops being the binding one.
pub const MAX_OFFER_BYTES: u64 = 16 * 1024 * 1024;

/// One advertised file. `blob` is the content hash of its `system/content/blob`
/// manifest — the id under which the offer is published, so re-offering the
/// same bytes is an idempotent overwrite rather than a second row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffer {
    pub name: String,
    pub size: u64,
    pub blob: Hash,
    /// The peer that offered it — carried in the manifest so a pulled file
    /// remembers where it came from even after the listing is gone.
    pub from: String,
    /// The app that handed this file to the host (`x-file`, [`crate::app_files`]),
    /// or `None` for a file a person offered themselves. See [`OfferSource`].
    pub source: Option<OfferSource>,
}

/// Which app produced an offered file.
///
/// **Encoded only when present**, under the manifest key [`SOURCE_KEY`]: a
/// file a person offered carries no such key, so every manifest written before
/// this field existed — and every one written without an app — keeps its exact
/// bytes and content hash. An older decoder ignores the key (unknown keys are
/// skipped), so a peer on an earlier build still lists the file.
///
/// `set` + `app` are the stable identity (the catalog's set and app id, the
/// same pair that keys the app's saves); `name` is the display name **as it
/// was when the file was kept**, carried so a reader on another peer — who may
/// not have that app installed at all — can still say where the file came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferSource {
    pub set: String,
    pub app: String,
    pub name: String,
}

/// The manifest key an [`OfferSource`] is encoded under.
pub const SOURCE_KEY: &str = "app";

impl FileOffer {
    /// The path segment this offer is published under (hex of the blob hash,
    /// which is already a safe single segment).
    pub fn id(&self) -> String {
        self.blob.to_hex()
    }
}

/// The inverse of [`FileOffer::id`] — an offer id back to its blob hash.
///
/// `to_hex` encodes the **whole wire form** (`[format varint || digest]`), so
/// this decodes to bytes and hands them to `Hash::from_bytes`, which is what
/// validates the format code. Lives beside `id()` so the pair cannot drift.
pub fn hash_from_id(id: &str) -> Result<Hash, String> {
    if !id.len().is_multiple_of(2) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("offer id is not hex: {id:?}"));
    }
    let bytes: Vec<u8> = (0..id.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&id[i..i + 2], 16).expect("validated as hex above"))
        .collect();
    Hash::from_bytes(&bytes).map_err(|e| format!("offer id is not a hash: {e}"))
}

// ---------------------------------------------------------------------------
// Manifest codec
// ---------------------------------------------------------------------------

/// Encode an offer as its manifest entity.
pub fn manifest_entity(offer: &FileOffer) -> Result<Entity, String> {
    manifest_entity_as(offer, OFFER_TYPE)
}

/// [`manifest_entity`] under another entity type. The body is shared with a file
/// an app kept privately (`crate::kept_files`); the TYPE is what keeps the two
/// apart, so a private file can never decode as something we offer.
pub fn manifest_entity_as(offer: &FileOffer, entity_type: &str) -> Result<Entity, String> {
    let mut fields = vec![
        (text("name"), text(offer.name.clone())),
        (text("size"), integer(offer.size as i64)),
        (text("blob"), ecf_bytes(offer.blob.to_bytes())),
        (text("from"), text(offer.from.clone())),
    ];
    // Absent, not empty, for a person's own file: see [`OfferSource`].
    if let Some(src) = &offer.source {
        fields.push((
            text(SOURCE_KEY),
            Value::Map(vec![
                (text("set"), text(src.set.clone())),
                (text("id"), text(src.app.clone())),
                (text("name"), text(src.name.clone())),
            ]),
        ));
    }
    let data = to_ecf(&Value::Map(fields));
    Entity::new(entity_type, data).map_err(|e| format!("file manifest: {e}"))
}

/// Decode a manifest entity. `None` for a wrong type or any malformed body —
/// a stranger's tree is untrusted input, so this never panics and never
/// half-fills an offer.
pub fn decode_manifest(entity: &Entity) -> Option<FileOffer> {
    decode_manifest_as(entity, OFFER_TYPE)
}

/// [`decode_manifest`] for another entity type; `None` for any other type.
pub fn decode_manifest_as(entity: &Entity, entity_type: &str) -> Option<FileOffer> {
    if entity.entity_type != entity_type {
        return None;
    }
    let value: Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    let (mut name, mut size, mut blob, mut from, mut source) = (None, None, None, None, None);
    for (k, v) in map {
        match k.as_text() {
            Some("name") => name = v.as_text().map(str::to_string),
            Some("size") => size = v.as_integer().and_then(|i| u64::try_from(i128::from(i)).ok()),
            Some("blob") => blob = v.as_bytes().and_then(|b| Hash::from_bytes(b).ok()),
            Some("from") => from = v.as_text().map(str::to_string),
            Some(SOURCE_KEY) => source = decode_source(v),
            _ => {}
        }
    }
    Some(FileOffer {
        name: name?,
        size: size?,
        blob: blob?,
        from: from.unwrap_or_default(),
        source,
    })
}

/// The `app` field of a manifest. **An unreadable one drops to `None` and keeps
/// the offer**: the source is a label about the file, and a stranger's malformed
/// label must not hide a file whose name, size and bytes are all fine. The cost
/// is stated — a malformed source reads as "a person offered this" — and it is
/// the cheaper mistake, since nothing branches on it but the words beside a row.
fn decode_source(v: &Value) -> Option<OfferSource> {
    let (mut set, mut app, mut name) = (None, None, None);
    for (k, v) in v.as_map()? {
        match k.as_text() {
            Some("set") => set = v.as_text().map(str::to_string),
            Some("id") => app = v.as_text().map(str::to_string),
            Some("name") => name = v.as_text().map(str::to_string),
            _ => {}
        }
    }
    let (set, app) = (set?, app?);
    if set.is_empty() || app.is_empty() {
        return None;
    }
    // A missing display name is recoverable: the id is still a name.
    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| app.clone());
    Some(OfferSource { set, app, name })
}

// ---------------------------------------------------------------------------
// Wire shapes (`system/content` §6.2 / §6.3)
// ---------------------------------------------------------------------------

/// The namespace path both ops name in their `resource`. Fully qualified,
/// because the handler writes the §6.4.2 presence binding at
/// `{namespace}/{hex(H)}` — a bare `system/content/files` would bind outside
/// any peer's tree.
pub fn namespace_resource(peer_id: &str) -> String {
    namespace_resource_for(peer_id, NAMESPACE)
}

/// [`namespace_resource`] for another `system/content` namespace.
pub fn namespace_resource_for(peer_id: &str, namespace: &str) -> String {
    format!("/{peer_id}/system/content/{namespace}")
}

// `resource_opts`, `empty_params` and `remote_execute` used to live here, and
// every line of them was about a **remote read** rather than about a file. They
// moved to [`crate::remote_read`] the moment a second consumer appeared
// (`feed_peer`, reading a followed publisher's feed off their live tree) — C15's
// rule, applied at the point a second caller proved the boundary rather than as
// a standalone rename.

/// An entity in the inline `core/entity` shape the content handler decodes
/// (`{type, data}`). The handler re-encodes `data` through ECF and re-hashes,
/// so the value we hand it must be the entity's own canonical body decoded —
/// not a re-serialization of something else.
fn inline_entity(entity: &Entity) -> Result<Value, String> {
    let data: Value = ciborium::from_reader(entity.data.as_slice())
        .map_err(|e| format!("entity body is not CBOR: {e}"))?;
    Ok(Value::Map(vec![
        (text("type"), text(entity.entity_type.clone())),
        (text("data"), data),
    ]))
}

/// `ingest` params in **envelope** mode: `root` is the blob manifest, `included`
/// carries every chunk keyed by its own content hash (the handler validates
/// `content_hash(entity) == key` and rejects a mismatch).
pub fn ingest_params(blob: &Entity, chunks: &[Entity]) -> Result<Entity, String> {
    let included: Vec<(Value, Value)> = chunks
        .iter()
        .map(|c| Ok((ecf_bytes(c.content_hash.to_bytes()), inline_entity(c)?)))
        .collect::<Result<_, String>>()?;
    let envelope = Value::Map(vec![
        (text("root"), inline_entity(blob)?),
        (text("included"), Value::Map(included)),
    ]);
    let data = to_ecf(&Value::Map(vec![(text("envelope"), envelope)]));
    Entity::new("system/content/ingest-request", data).map_err(|e| format!("ingest params: {e}"))
}

/// `get` params: the hashes to fetch, each as a bstr hash record.
pub fn get_params(hashes: &[Hash]) -> Result<Entity, String> {
    let data = to_ecf(&Value::Map(vec![(
        text("hashes"),
        Value::Array(hashes.iter().map(|h| ecf_bytes(h.to_bytes())).collect()),
    )]));
    Entity::new("system/content/get-request", data).map_err(|e| format!("get params: {e}"))
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

/// A byte count in human terms. Lives here rather than in the DOM because the
/// first thing that needed it was a *refusal message* from the model
/// (`MAX_OFFER_BYTES`), and a size the window prints must read identically to a
/// size the model prints — `dom::file_transfer::human_size` delegates here.
pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B") // i18n-ignore — byte unit, language-neutral
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Why a file of `size` bytes is too big to offer — **one expression of the
/// refusal**, for the same reason `human_bytes` lives here: the wording a
/// window shows must read identically to the wording the model produces.
///
/// It has two callers on purpose. [`offer_file`] refuses at the point of work,
/// which is the authority; the file picker refuses at the point of *choice*,
/// **before** `array_buffer()` — a 200 MB video should not be pulled into wasm
/// memory (where it costs roughly four copies) just to be turned down, and on a
/// phone that read is exactly where the tab dies. Neither refusal is redundant:
/// the picker's is a courtesy the Shell verb does not get, and the model's is
/// the one that cannot be bypassed.
/// Below this, a file chooser's `cancel` cannot have come from a human hand —
/// nobody opens and dismisses a dialog in a quarter of a second. Above it, a
/// cancel is an ordinary "changed my mind" and deserves no message at all.
pub const PICKER_AUTO_DISMISS_MS: f64 = 300.0;

/// A file chooser that the engine opened **and closed itself**, in `ms`.
///
/// **Why this is a message and a plain cancel is not.** A `cancel` event means
/// "no file was chosen", and it covers two completely different situations: a
/// person backing out of a chooser, and an engine that accepted the request and
/// then dismissed it without ever showing anything. The first needs no comment;
/// the second is a failure, and it is **the exact shape of the Android/Firefox
/// report that this offer button "just doesn't do anything"** — no dialog, no
/// error, nothing in the console, because the app listened only for `change`
/// and `cancel` went nowhere.
///
/// The elapsed time is the only discriminator available: nothing in the event
/// says whether a chooser was ever painted. So the number is quoted rather than
/// hidden — if this message ever appears with a plausible human interval, the
/// threshold is wrong and the reader can see that for themselves.
///
/// **It names no cause, and that is deliberate — it hands over a TEST instead.**
/// The first version said "the browser has no permission to reach files", which
/// is very likely wrong: on Android the chooser is the Storage Access Framework,
/// which requires no permission at all. A confident wrong cause sends someone
/// into the wrong settings screen, which is worse than saying less — the same
/// rule this repo applies to the WebRTC banner and the insecure-origin row.
///
/// What the page genuinely knows is: the request was accepted, no chooser was
/// shown, and nothing here can retry it.
///
/// **Measured 2026-08-24 on one Android device: Firefox auto-dismisses, Chrome
/// opens the chooser normally — same page, same phone, same file.** So the
/// message names the workaround that was actually observed to work rather than
/// a settings screen nobody has confirmed matters. It stays hedged about the
/// cause (one device is not a survey) while being concrete about the remedy,
/// which is the split this repo keeps re-learning: state the consequence you
/// measured, not the explanation you inferred.
pub fn picker_auto_dismissed_message(ms: f64) -> String {
    format!(
        "the browser closed the file chooser itself after {ms:.0}ms, without showing it — \
         so no file could be picked, and this is not something the page can retry. \
         The cause is in the browser, not this app: the same page can open a file \
         chooser in a different browser on the same device. If you are on Firefox for \
         Android, try Chrome."
    )
}

pub fn too_large_message(name: &str, size: u64) -> String {
    format!(
        "{name} is {} — this browser offers files up to {} \
         (it holds the whole file, its chunks and one CBOR envelope in memory \
         at once; larger files need a streaming ingest, not a larger limit)",
        human_bytes(size),
        human_bytes(MAX_OFFER_BYTES),
    )
}

/// Why an offer could not start at all: this device's own peer has no dispatch
/// route, so there is nothing to ingest into.
///
/// Lives beside [`too_large_message`] for the same reason — the refusal wording
/// has one home in the model tier — and because that keeps the whole
/// `OfferOutcome::Failed` channel a single, consistent class of message. (Model
/// English inside a localized frame is a known, recorded shape; a translated
/// "it failed" with the reason dropped would be worse.)
pub fn not_routed_message(local_pid: &str) -> String {
    format!("this device's peer ({local_pid}) is not routed — reload and try again")
}

/// How many §3.2 chunks a file of `size` bytes becomes. The window states this
/// *before* the work starts — a count is the only honest progress an ingest can
/// offer, since chunking is one synchronous pass with nothing to report from
/// inside it.
pub fn chunk_count(size: u64) -> u64 {
    size.div_ceil(CHUNK_SIZE as u64).max(1)
}

/// Chunk `raw` into the §3.2 fixed-size entity shape. Returns the blob manifest
/// entity plus its chunk entities, in blob-declared order.
///
/// The scratch store is local and private: chunking is a pure function of the
/// bytes, and routing it through the peer would need a `ContentStore` the
/// Worker arm cannot hand out.
pub fn chunk_bytes(raw: &[u8]) -> Result<(Entity, Vec<Entity>), String> {
    let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let blob_hash =
        create_blob_fixed(&store, raw, CHUNK_SIZE).map_err(|e| format!("chunking failed: {e}"))?;
    let blob = store
        .get(&blob_hash)
        .ok_or_else(|| "chunker did not store its own blob".to_string())?;
    let (_total, chunk_hashes) =
        blob_chunk_hashes(&store, &blob_hash).map_err(|e| format!("blob decode: {e}"))?;
    let chunks = chunk_hashes
        .into_iter()
        .map(|h| {
            store
                .get(&h)
                .ok_or_else(|| format!("chunker did not store chunk {}", h.to_hex()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((blob, chunks))
}

// ---------------------------------------------------------------------------
// Offer (sender side)
// ---------------------------------------------------------------------------

/// Ingest `raw` into our own content namespace and publish its manifest.
///
/// Both halves are dispatched (L1), so this is one code path on both arms. The
/// manifest write is awaited rather than fire-and-forget: an offer whose bytes
/// are ingested but whose manifest never lands is a file nobody can name.
pub async fn offer_file(
    dispatch: &DispatchHandle,
    name: &str,
    raw: &[u8],
) -> Result<FileOffer, String> {
    offer_file_from(dispatch, name, raw, None).await
}

/// [`offer_file`], recording the app the file came from (see [`OfferSource`]).
pub async fn offer_file_from(
    dispatch: &DispatchHandle,
    name: &str,
    raw: &[u8],
    source: Option<OfferSource>,
) -> Result<FileOffer, String> {
    let local_pid = dispatch.local_peer_id();
    // Refuse **before** allocating anything: the whole point of a stated limit
    // is that the user is told, in a sentence, instead of watching the tab die
    // partway through an ingest they cannot see. See [`MAX_OFFER_BYTES`].
    if raw.len() as u64 > MAX_OFFER_BYTES {
        return Err(too_large_message(name, raw.len() as u64));
    }
    let blob = ingest_into(dispatch, NAMESPACE, raw).await?;

    let offer = FileOffer {
        name: name.to_string(),
        size: raw.len() as u64,
        blob: blob.content_hash,
        from: local_pid.clone(),
        source,
    };
    dispatch
        .put(
            offer_path(APP_ID, &local_pid, &offer.id()),
            manifest_entity(&offer)?,
        )
        .await?;
    Ok(offer)
}

/// Chunk `raw` and ingest it into this peer's own `system/content` `namespace`;
/// returns the blob manifest (its content hash is the file's id). No size check:
/// callers state their own ceiling first.
pub async fn ingest_into(dispatch: &DispatchHandle, namespace: &str, raw: &[u8]) -> Result<Entity, String> {
    let local_pid = dispatch.local_peer_id();
    let (blob, chunks) = chunk_bytes(raw)?;
    let params = ingest_params(&blob, &chunks)?;
    let result = dispatch
        .execute(
            "system/content".to_string(),
            "ingest".to_string(),
            params,
            resource_opts(&namespace_resource_for(&local_pid, namespace)),
        )
        .await?;
    if result.status != 200 {
        return Err(format!("ingest refused: status {}", result.status));
    }
    Ok(blob)
}

// ---------------------------------------------------------------------------
// Pull (receiver side)
// ---------------------------------------------------------------------------

/// List what `remote_pid` is offering, newest-listing-order irrelevant (the
/// caller sorts). One `system/tree` listing plus one read per manifest — the
/// same cross-peer read shape chat's delivery uses, so it works over any
/// transport and on either arm.
///
/// A manifest that fails to decode is **skipped, not fatal**: one bad row in a
/// stranger's tree must not hide the rest of their offers.
pub async fn list_offers(
    dispatch: &DispatchHandle,
    remote_pid: &str,
) -> Result<Vec<FileOffer>, String> {
    let prefix = offers_prefix(APP_ID, remote_pid);
    let listing = remote_execute(
        dispatch,
        format!("entity://{remote_pid}/system/tree"),
        "get".to_string(),
        empty_params(),
        resource_opts(&prefix),
    )
    .await?;
    if listing.status == 404 {
        return Ok(Vec::new()); // nothing offered yet — not an error
    }
    if listing.status != 200 {
        return Err(format!("offer listing refused: status {}", listing.status));
    }

    let mut out = Vec::new();
    for id in crate::backend_auth::parse_listing_keys(&listing.result) {
        let leaf = remote_execute(
            dispatch,
            format!("entity://{remote_pid}/system/tree"),
            "get".to_string(),
            empty_params(),
            resource_opts(&format!("{prefix}{id}")),
        )
        .await?;
        if leaf.status == 200 {
            if let Some(offer) = decode_manifest(&leaf.result) {
                out.push(offer);
            }
        }
    }
    Ok(out)
}

/// Pull an offered file's bytes from `remote_pid` — the closure walk.
///
/// Blob first, then its chunks in `GET_BATCH_SIZE` batches, each response's
/// `included` map folded into a local scratch store until `reassemble` can
/// resolve the whole blob. A batch that returns **nothing new** is fatal rather
/// than an infinite loop: the two ways that happens (the peer does not hold the
/// chunk, or the frame budget cannot fit even one) are both the caller's
/// problem to see, and a silent spin would present as a hang.
///
/// The bytes are verified by construction — `reassemble` walks content hashes,
/// so a corrupted or substituted chunk cannot reassemble into the named blob.
pub async fn pull_offer(
    dispatch: &DispatchHandle,
    remote_pid: &str,
    blob: &Hash,
) -> Result<Vec<u8>, String> {
    pull_offer_with(dispatch, remote_pid, blob, |_, _| {}).await
}

/// [`pull_offer`], reporting progress as `(chunks_held, chunks_total)` after
/// every batch.
///
/// The callback exists because a pull is the one part of a transfer with a
/// **genuinely reportable** interior: the closure walk is N round trips, and on
/// a slow link the whole of it happens between "↓ pulling" and "✓ saved" with
/// nothing in between — which is indistinguishable from a hang. It fires per
/// *batch* (16 chunks ≈ 4 MiB), not per chunk: a per-chunk callback would write
/// more event-log lines than there are bytes worth reporting.
///
/// The first call is `(held, total)` as soon as the blob is decoded, so a caller
/// can state the size of the job before the first chunk arrives.
///
/// **Generic, not `&dyn Fn`, and that is load-bearing on native.** The future
/// this returns captures the callback, so a trait object would make the future
/// `!Send` for every caller — including the Shell's `spawn_task`, which requires
/// `Send`. Monomorphizing keeps the no-op case exactly as `Send` as it was.
pub async fn pull_offer_with<P: Fn(usize, usize)>(
    dispatch: &DispatchHandle,
    remote_pid: &str,
    blob: &Hash,
    progress: P,
) -> Result<Vec<u8>, String> {
    walk_closure(dispatch, Holder::Peer(remote_pid), NAMESPACE, blob, progress).await
}

/// The bytes of one of **our own** offers, read out of our own `system/content`.
///
/// This is what "save to this device" needs for a file an app handed the host:
/// the bytes are already here, so the walk is local — a bare `system/content`
/// dispatch, not `entity://{self}/…`, for the reason [`read_own_offers`] gives
/// (the remote shape's failure path is ten one-second retries against ourselves).
/// Same walk as [`pull_offer`], so a large file is reassembled from its chunks
/// exactly as a pulled one is, and verified by the same hash walk.
pub async fn read_own_offer(dispatch: &DispatchHandle, blob: &Hash) -> Result<Vec<u8>, String> {
    read_own_in(dispatch, NAMESPACE, blob).await
}

/// [`read_own_offer`] from another of our own `system/content` namespaces.
pub async fn read_own_in(dispatch: &DispatchHandle, namespace: &str, blob: &Hash) -> Result<Vec<u8>, String> {
    walk_closure(dispatch, Holder::Local, namespace, blob, |_, _| {}).await
}

/// Whose `system/content` a closure walk reads.
#[derive(Clone, Copy)]
enum Holder<'a> {
    /// This peer's own store, dispatched locally.
    Local,
    /// A remote peer, dispatched through the connection pool.
    Peer(&'a str),
}

async fn walk_closure<P: Fn(usize, usize)>(
    dispatch: &DispatchHandle,
    holder: Holder<'_>,
    namespace: &str,
    blob: &Hash,
    progress: P,
) -> Result<Vec<u8>, String> {
    let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let holder_pid = match holder {
        Holder::Local => dispatch.local_peer_id(),
        Holder::Peer(pid) => pid.to_string(),
    };
    let namespace = namespace_resource_for(&holder_pid, namespace);

    fetch_into(dispatch, holder, &namespace, &store, &[*blob]).await?;
    if store.get(blob).is_none() {
        return Err(match holder {
            Holder::Peer(pid) => format!(
                "peer {pid} did not return the offered blob {} — the offer is stale",
                blob.to_hex()
            ),
            Holder::Local => format!(
                "this browser no longer holds the file's contents ({})",
                blob.to_hex()
            ),
        });
    }

    let (_total, chunk_hashes) =
        blob_chunk_hashes(&store, blob).map_err(|e| format!("blob decode: {e}"))?;
    let total = chunk_hashes.len();
    let mut missing: Vec<Hash> = chunk_hashes
        .iter()
        .filter(|h| store.get(h).is_none())
        .copied()
        .collect();
    progress(total - missing.len(), total);
    while !missing.is_empty() {
        let batch: Vec<Hash> = missing.iter().take(GET_BATCH_SIZE).copied().collect();
        let before = missing.len();
        fetch_into(dispatch, holder, &namespace, &store, &batch).await?;
        missing.retain(|h| store.get(h).is_none());
        progress(total - missing.len(), total);
        if missing.len() == before {
            return Err(format!(
                "closure stalled: {} chunk(s) still missing after a full batch \
                 (the peer no longer holds them, or one chunk exceeds the frame budget)",
                missing.len()
            ));
        }
    }

    reassemble(&store, blob).map_err(|e| format!("reassemble failed: {e}"))
}

/// One `system/content:get` against `remote_pid`, folding every returned entity
/// into `store`. Hash-keyed, so an entity the peer sent that we did not ask for
/// is harmless — and one it withheld shows up as a still-missing hash rather
/// than as a partial file.
async fn fetch_into(
    dispatch: &DispatchHandle,
    holder: Holder<'_>,
    namespace: &str,
    store: &Arc<dyn ContentStore>,
    hashes: &[Hash],
) -> Result<(), String> {
    let params = get_params(hashes)?;
    let opts = resource_opts(namespace);
    let result = match holder {
        Holder::Peer(pid) => {
            remote_execute(
                dispatch,
                format!("entity://{pid}/system/content"),
                "get".to_string(),
                params,
                opts,
            )
            .await?
        }
        Holder::Local => {
            dispatch
                .execute("system/content".to_string(), "get".to_string(), params, opts)
                .await?
        }
    };
    if result.status != 200 {
        return Err(format!("content get refused: status {}", result.status));
    }
    for entity in result.included.values() {
        // A put that fails (hash mismatch, encoding) simply leaves that hash
        // missing, which the caller's loop reports — never a partial file.
        let _ = store.put(entity.clone());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// What *we* are offering (the sender's own view)
// ---------------------------------------------------------------------------

/// The offers this peer is publishing, read from its **own** tree.
///
/// Deliberately a plain local read (`tree_listing` + `get_entity`) rather than
/// [`list_offers`] against our own id: the two look interchangeable and are not.
/// `list_offers` dispatches `entity://{pid}/system/tree` — a *remote* shape
/// whose failure path is ten one-second retries — where the answer is sitting in
/// our own store. A caller must subscribe the prefix
/// ([`crate::app_paths::offers_prefix`]) for this to be populated on the Worker
/// arm, where a tree read is a cache mirror seeded only for subscribed prefixes.
///
/// Sorted by name so the list does not reshuffle under the cursor when an offer
/// is added (the tree order is by content hash, which is effectively random).
pub fn read_own_offers(peers: &crate::peers::Peers, peer_id: &str) -> Vec<FileOffer> {
    let prefix = offers_prefix(APP_ID, peer_id);
    let mut out: Vec<FileOffer> = peers
        .tree_listing(peer_id, &prefix)
        .into_iter()
        .filter_map(|entry| {
            let id = entry.path.strip_prefix(&prefix)?;
            if id.is_empty() || id.contains('/') {
                return None; // one level; defensive — it never nests today
            }
            decode_manifest(&peers.get_entity(peer_id, &entry.path)?)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.blob.to_hex().cmp(&b.blob.to_hex())));
    out
}

/// Stop listing an offer: remove its manifest from our tree.
///
/// **Say what this does and does not do.** It withdraws the *name* — the row
/// disappears from every peer's listing, and nobody can discover the file from
/// us again. It does **not** unpublish the bytes: content is hash-addressed, the
/// ingest left a §6.4.2 presence binding in our `system/content` namespace, and
/// a peer that already pulled (or merely saw) the content id can still `get` it.
/// Reclaiming the content itself is a separate, binding-aware operation
/// (`WriterHandle::content_remove` refuses while a live path binds the blob, and
/// the presence binding is exactly such a path), so the honest surface says
/// "stop offering", never "delete".
pub fn withdraw_offer(writer: &crate::writer_handle::WriterHandle, peer_id: &str, offer_id: &str) {
    writer.remove(offer_path(APP_ID, peer_id, offer_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    // `.get(field)` on a CBOR map — used only by the wire-shape assertions.
    use entity_ecf::ValueExt;

    #[test]
    fn a_manifest_round_trips() {
        let (blob, _) = chunk_bytes(b"hello").unwrap();
        let offer = FileOffer {
            name: "notes.txt".into(),
            size: 5,
            blob: blob.content_hash,
            from: "PEER_A".into(),
            source: None,
        };
        let decoded = decode_manifest(&manifest_entity(&offer).unwrap()).unwrap();
        assert_eq!(decoded, offer);
        assert_eq!(decoded.id(), blob.content_hash.to_hex());
    }

    #[test]
    fn a_manifest_without_an_app_keeps_the_bytes_it_had_before_the_field_existed() {
        // Built by hand in the pre-field layout, NOT through `manifest_entity`:
        // a fixture from the encoder under test would move with it.
        let (blob, _) = chunk_bytes(b"hello").unwrap();
        let legacy = Entity::new(
            OFFER_TYPE,
            to_ecf(&Value::Map(vec![
                (text("name"), text("notes.txt")),
                (text("size"), integer(5)),
                (text("blob"), ecf_bytes(blob.content_hash.to_bytes())),
                (text("from"), text("PEER_A")),
            ])),
        )
        .unwrap();
        let offer = FileOffer {
            name: "notes.txt".into(),
            size: 5,
            blob: blob.content_hash,
            from: "PEER_A".into(),
            source: None,
        };
        let now = manifest_entity(&offer).unwrap();
        assert_eq!(now.data, legacy.data, "a person's own offer must not re-encode");
        assert_eq!(now.content_hash, legacy.content_hash);
        // And the old manifest still decodes, as a person's offer.
        assert_eq!(decode_manifest(&legacy).unwrap(), offer);
    }

    #[test]
    fn an_offer_records_the_app_it_came_from() {
        let (blob, _) = chunk_bytes(b"report").unwrap();
        let offer = FileOffer {
            name: "report.txt".into(),
            size: 6,
            blob: blob.content_hash,
            from: "PEER_A".into(),
            source: Some(OfferSource {
                set: "apps".into(),
                app: "alpine".into(),
                name: "Alpine Linux".into(),
            }),
        };
        let entity = manifest_entity(&offer).unwrap();
        assert_eq!(decode_manifest(&entity).unwrap(), offer);
        // Pinned by literal: other peers read this key.
        let value: Value = ciborium::from_reader(entity.data.as_slice()).unwrap();
        let src = value.get("app").expect("the source is encoded under `app`");
        assert_eq!(src.get("id").unwrap().as_text(), Some("alpine"));
        assert_eq!(src.get("set").unwrap().as_text(), Some("apps"));
    }

    #[test]
    fn an_unreadable_app_field_keeps_the_file_and_drops_only_the_label() {
        let (blob, _) = chunk_bytes(b"x").unwrap();
        let body = |app: Value| {
            Entity::new(
                OFFER_TYPE,
                to_ecf(&Value::Map(vec![
                    (text("name"), text("x.bin")),
                    (text("size"), integer(1)),
                    (text("blob"), ecf_bytes(blob.content_hash.to_bytes())),
                    (text("from"), text("P")),
                    (text("app"), app),
                ])),
            )
            .unwrap()
        };
        for bad in [
            text("alpine"),
            Value::Map(vec![(text("id"), text("alpine"))]),
            Value::Map(vec![(text("set"), text("")), (text("id"), text("alpine"))]),
        ] {
            let offer = decode_manifest(&body(bad.clone())).expect("the file must still list");
            assert_eq!(offer.name, "x.bin");
            assert_eq!(offer.source, None, "{bad:?} is not a usable source");
        }
        // No display name: the id stands in, rather than dropping the source.
        let named = decode_manifest(&body(Value::Map(vec![
            (text("set"), text("apps")),
            (text("id"), text("alpine")),
        ])))
        .unwrap();
        assert_eq!(named.source.unwrap().name, "alpine");
    }

    #[test]
    fn a_foreign_or_malformed_manifest_decodes_to_none() {
        // Wrong type: a stranger's tree holds whatever they put there.
        let wrong = Entity::new("app/other/thing", to_ecf(&Value::Null)).unwrap();
        assert!(decode_manifest(&wrong).is_none());
        // Right type, missing fields → None rather than a half-filled offer.
        let partial = Entity::new(OFFER_TYPE, to_ecf(&Value::Map(vec![(text("name"), text("x"))])))
            .unwrap();
        assert!(decode_manifest(&partial).is_none());
    }

    #[test]
    fn chunking_is_content_addressed_and_complete() {
        // Two chunks' worth plus a tail, so the blob really lists several.
        let raw: Vec<u8> = (0..(CHUNK_SIZE * 2 + 7)).map(|i| (i % 251) as u8).collect();
        let (blob, chunks) = chunk_bytes(&raw).unwrap();
        assert_eq!(chunks.len(), 3, "two full chunks and a tail");

        // The blob names exactly these chunks, in order — and reassembling
        // from them alone returns the original bytes.
        let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        store.put(blob.clone()).unwrap();
        for c in &chunks {
            store.put(c.clone()).unwrap();
        }
        let (total, listed) = blob_chunk_hashes(&store, &blob.content_hash).unwrap();
        assert_eq!(total, raw.len() as u64);
        assert_eq!(
            listed,
            chunks.iter().map(|c| c.content_hash).collect::<Vec<_>>()
        );
        assert_eq!(reassemble(&store, &blob.content_hash).unwrap(), raw);

        // Identical bytes chunk to an identical blob — this is what makes the
        // blob hash usable as the offer id (re-offering overwrites one row).
        let (again, _) = chunk_bytes(&raw).unwrap();
        assert_eq!(again.content_hash, blob.content_hash);
    }

    #[test]
    fn ingest_params_carry_the_blob_as_root_and_chunks_keyed_by_hash() {
        let raw: Vec<u8> = (0..(CHUNK_SIZE + 1)).map(|i| (i % 97) as u8).collect();
        let (blob, chunks) = chunk_bytes(&raw).unwrap();
        let params = ingest_params(&blob, &chunks).unwrap();
        let value: Value = ciborium::from_reader(params.data.as_slice()).unwrap();
        let envelope = value.get("envelope").expect("envelope mode");

        // Root is the blob, in the `{type, data}` shape the handler decodes.
        let root = envelope.get("root").unwrap();
        assert_eq!(root.get("type").unwrap().as_text(), Some("system/content/blob"));

        // Every included key is its entity's own content hash — the handler
        // rejects a mismatch, so getting this wrong is a 400 at the far end.
        let included = envelope.get("included").unwrap().as_map().unwrap().clone();
        assert_eq!(included.len(), chunks.len());
        for (key, ent) in &included {
            let h = Hash::from_bytes(key.as_bytes().unwrap()).unwrap();
            let round = Entity::new(
                ent.get("type").unwrap().as_text().unwrap(),
                to_ecf(ent.get("data").unwrap()),
            )
            .unwrap();
            assert_eq!(round.content_hash, h, "included entity must hash to its key");
        }
    }

    /// A user can pick a 0-byte file, so the chunker has to have an answer for
    /// one. Whatever it is, it must be an answer and not a panic — and the
    /// count we *state* before the work starts has to agree with it.
    #[test]
    fn an_empty_file_chunks_without_panicking() {
        match chunk_bytes(b"") {
            Ok((blob, chunks)) => {
                // Round-trips to nothing, which is the honest result.
                let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
                store.put(blob.clone()).unwrap();
                for c in &chunks {
                    store.put(c.clone()).unwrap();
                }
                assert_eq!(reassemble(&store, &blob.content_hash).unwrap(), Vec::<u8>::new());
                assert!(
                    chunks.len() as u64 <= chunk_count(0),
                    "an empty file must not claim more chunks than the count the \
                     window prints ({} stated, {} produced)",
                    chunk_count(0),
                    chunks.len()
                );
            }
            // A refusal is equally acceptable — it reaches the user as
            // "offer failed: …" rather than as a frozen tab.
            Err(e) => assert!(!e.is_empty(), "a refusal must say something"),
        }
    }

    #[test]
    fn get_params_encode_hashes_as_bstr_records() {
        let (blob, _) = chunk_bytes(b"x").unwrap();
        let params = get_params(&[blob.content_hash]).unwrap();
        let value: Value = ciborium::from_reader(params.data.as_slice()).unwrap();
        let arr = value.get("hashes").unwrap().as_array().unwrap().clone();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            Hash::from_bytes(arr[0].as_bytes().unwrap()).unwrap(),
            blob.content_hash
        );
    }

    #[test]
    fn the_namespace_resource_is_peer_qualified() {
        // The handler writes the §6.4.2 presence binding at
        // `{resource}/{hex(H)}`; an unqualified namespace would bind outside
        // any peer's tree.
        assert_eq!(
            namespace_resource("PEER_A"),
            "/PEER_A/system/content/files"
        );
    }
}
