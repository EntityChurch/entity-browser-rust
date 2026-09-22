//! Staging and resolving a site asset's bytes across the payload union.
//!
//! [`format::SiteAsset`] is the **wire type** — media type plus EMBED §3's
//! tagged payload. This module is the pair of operations that cross the
//! boundary between bytes and that wire type, and it is deliberately separate
//! from `format.rs` so the codec keeps no content-store dependency:
//!
//! - [`stage`] — bytes in, `(SiteAsset, content entities)` out. Consults
//!   `INLINE_PAYLOAD_MAX` and chunks the overflow at the canonical size.
//! - [`resolve`] — a `SiteAsset` plus a content lookup, bytes out.
//!
//! ## Why the ceiling is a conformance rule and not a size preference
//!
//! `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §4:
//!
//! > **[MUST]** An asset whose bytes exceed `inline-payload`'s
//! > `.size (1..16384)` ceiling **MUST** use a **`pointer`** payload, placing
//! > the bytes in the content store **where §6.1's canonical chunking governs
//! > them.** An implementation **MUST NOT** inline unbounded bytes in a site
//! > asset.
//!
//! §6.1 makes *"same image → same site root"* depend on every v1 publisher
//! chunking at the canonical 1 MiB FastCDC default. **Bytes that never enter
//! the content store never meet that rule** — so on an all-inline asset path
//! the MUST is not failing, it is *unreachable*, and no reproducible-publish
//! check can observe it. That is the whole reason our `G-PIN-4` asset row was
//! incomparable and `EXPECTED-INGEST.json` carried two roots.
//!
//! ## What `stage` hands back, and why it is not a side effect
//!
//! Chunking writes blob + chunk entities into a content store, and **every
//! caller then has a second obligation** — the publisher must project them
//! beside the asset entity, the browser must persist them beside the cached
//! one. A function that only wrote them into the store it was handed would
//! leave that obligation to memory, which is the shape AP44 names. So the
//! produced entities are a **return value**: a caller that ignores them gets
//! an unused-variable warning, not a site whose figures 404.

use std::sync::Arc;

use entity_entity::Entity;
use entity_hash::Hash;
use entity_store::{ContentStore, MemoryContentStore};

use super::format::{AssetPayload, SiteAsset, CANONICAL_CHUNK_SIZE, INLINE_PAYLOAD_MAX};
use crate::embed::EmbedPayload;

/// One staged asset: the wire entity's contents, plus the content-store
/// entities that must travel with it.
///
/// `content` is empty for an inline asset — the bytes are in the asset
/// itself — and holds the `system/content/blob` plus every
/// `system/content/chunk` for a pointer one.
#[derive(Debug, Clone)]
pub struct StagedAsset {
    pub asset: SiteAsset,
    /// The blob and its chunks, blob **first**. Order is not load-bearing for
    /// correctness (they are content-addressed) but it makes a report of what
    /// a publish emitted readable.
    pub content: Vec<Entity>,
}

/// Stage `bytes` as a site asset: the **inline arm inside
/// `1..=`[`INLINE_PAYLOAD_MAX`]**, the pointer arm everywhere else.
///
/// **A range, not a ceiling.** `inline-payload` is `bstr .size (1..16384)`, so
/// 16,384 is the last conformant inline size *and* zero is not a conformant
/// one — an empty file has no inline representation and takes the pointer
/// arm like an oversized one. Every edge is gated
/// (`the_inline_ceiling_is_asserted_from_both_sides`,
/// `an_empty_asset_is_outside_the_inline_range_and_takes_the_pointer_arm`),
/// because `F-5` already cost us the lesson that a bound asserted from one
/// side passes for an implementation that only ever went one way — and here
/// each direction has a *different* wrong implementation.
///
/// Chunking is [`CANONICAL_CHUNK_SIZE`] FastCDC, §6.1's locked default.
/// **Do not make this a parameter** — a per-caller chunk size is precisely
/// the divergence §6.1 exists to prevent, and the one legitimate consumer of
/// a *different* size (`blob_chunk_size`, re-chunking someone else's blob
/// against its own recorded parameters) is a read path, not this one.
pub fn stage(
    media_type: impl Into<String>,
    bytes: Vec<u8>,
    store: &Arc<dyn ContentStore>,
) -> Result<StagedAsset, String> {
    let media_type = media_type.into();
    // **`1..=MAX`, not `<= MAX` — the range is what the CDDL says.**
    // `inline-payload` is `bstr .size (1..16384)`, so **zero is outside it**,
    // and an empty file staged as `Inline(vec![])` would encode a payload no
    // conformant decoder should accept. It is a small case and a real one (a
    // truncated download, a placeholder someone `touch`ed), and the previous
    // `<=` spelling produced it silently.
    //
    // The pointer arm handles it exactly: a zero-byte blob is a `total_size:
    // 0` manifest with an empty chunk list, which is representable, verifiable
    // and reassembles to the empty vec. So *outside the inline range* — above
    // **or** below — takes the pointer, and there is no third case.
    if EmbedPayload::inline_is_conformant(bytes.len()) {
        return Ok(StagedAsset {
            asset: SiteAsset::inline(media_type, bytes),
            content: Vec::new(),
        });
    }
    let blob_hash = entity_content::create_blob_fastcdc(store, &bytes, CANONICAL_CHUNK_SIZE)
        .map_err(|e| format!("chunk asset ({} bytes): {e}", bytes.len()))?;
    let content = blob_closure(store, &blob_hash)?;
    Ok(StagedAsset {
        asset: SiteAsset {
            media_type,
            payload: AssetPayload::Pointer(blob_hash),
            from_legacy_encoding: false,
        },
        content,
    })
}

/// The blob entity plus every chunk entity it names, read back out of the
/// store that was just chunked into.
///
/// Read back rather than collected during chunking, deliberately: the
/// chunker is the kernel's and its return value is the blob hash alone, so
/// re-deriving the set from the blob is the only expression that cannot drift
/// from what was actually written.
pub fn blob_closure(
    store: &Arc<dyn ContentStore>,
    blob_hash: &Hash,
) -> Result<Vec<Entity>, String> {
    blob_closure_via(blob_hash, |h| store.get(h))
}

/// [`blob_closure`] against an arbitrary lookup — the same walk where the
/// caller holds something that is not an `Arc<dyn ContentStore>` (the app
/// tier's `Peers::content_by_hash`, or a map an HTTP consumer just fetched
/// into).
///
/// One expression, two entry points: `blob_closure` is this with a store
/// closed over. Two hand-written walks of a blob's chunk list is exactly the
/// C15 shape, and the half nobody reads is the half that drifts.
pub fn blob_closure_via<F>(blob_hash: &Hash, lookup: F) -> Result<Vec<Entity>, String>
where
    F: Fn(&Hash) -> Option<Entity>,
{
    let blob = lookup(blob_hash)
        .ok_or_else(|| format!("blob {} is not held", blob_hash.to_hex()))?;
    let chunks = chunk_hashes_of(&blob)?;
    let mut out = Vec::with_capacity(chunks.len() + 1);
    out.push(blob);
    for ch in chunks {
        let e = lookup(&ch)
            .ok_or_else(|| format!("chunk {} named by the blob is absent", ch.to_hex()))?;
        out.push(e);
    }
    Ok(out)
}

/// The chunk hashes a `system/content/blob` entity names, in order.
///
/// **The one place that reads the blob wire shape, and it does not read it.**
/// The entity is staged into a scratch store and handed to the kernel's
/// `blob_chunk_hashes`, whose doc comment exists so downstream handlers can
/// enumerate a blob *"without re-implementing the wire decode"*. Every arm
/// here — the publisher, the browser store, the HTTP consumer — goes through
/// this, so there is one expression of that schema in this crate rather than
/// three that agree until one of them does not (C15).
pub fn chunk_hashes_of(blob: &Entity) -> Result<Vec<Hash>, String> {
    let hash = blob.content_hash;
    let scratch = MemoryContentStore::new();
    scratch.put(blob.clone()).map_err(|e| format!("stage blob: {e}"))?;
    let scratch: Arc<dyn ContentStore> = Arc::new(scratch);
    let (_total, chunks) = entity_content::blob_chunk_hashes(&scratch, &hash)
        .map_err(|e| format!("blob {} decodes: {e}", hash.to_hex()))?;
    Ok(chunks)
}

/// Resolve an asset against a **slice of content entities** — the shape a
/// self-contained site carries (`read::OwnedSite::content`, and what an
/// `.entsite` bundle will hold).
pub fn resolve_from(asset: &SiteAsset, content: &[Entity]) -> Result<Vec<u8>, ResolveError> {
    resolve(asset, |h| content.iter().find(|e| &e.content_hash == h).cloned())
}

/// The reason an asset's bytes could not be produced.
///
/// **Five outcomes, kept apart on purpose (AP40), and each licenses a
/// different sentence:**
///
/// | variant | what it means | what would fix it |
/// |---|---|---|
/// | [`NoPayload`](Self::NoPayload) | we cannot read this asset entity at all | nothing to fetch — the entity itself is the problem |
/// | [`BlobMissing`](Self::BlobMissing) | we hold the pointer, not the blob | a fetch we have not made yet; the ordinary cold state on the +content tier |
/// | [`ChunkMissing`](Self::ChunkMissing) | the closure is incomplete | the rest of the fetch |
/// | [`UnsupportedPayload`](Self::UnsupportedPayload) | a payload tag EMBED §3 does not yet define | us building it — their bytes are fine |
/// | [`InvalidForType`](Self::InvalidForType) | a legal embed arm this type refuses (`child`, A-30) | **the publisher** — a schema violation, actionable at the origin |
/// | [`Malformed`](Self::Malformed) | it is there and does not decode | nothing — corrupt or truncated |
///
/// **There is deliberately no "the publisher shipped an empty asset"**, because
/// the wire cannot say that: `inline-payload`'s `bstr .size (1..16384)`
/// excludes zero. Anything that looks like an empty asset is one of the four
/// above wearing a success's clothes, which is precisely the bug
/// `NoPayload` was added to end.
///
/// Collapsing these into `Option<Vec<u8>>` at a *boundary* is fine — a
/// renderer has one thing to draw — but the boundary is where that collapse
/// belongs, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// **The entity carried no payload this decoder could read** — see
    /// [`AssetPayload::Unreadable`]. Distinct from every arm below because it
    /// is a fault in the *asset*, not in the closure behind it: there is
    /// nothing to fetch that would fix it.
    ///
    /// This variant is the other half of the defect
    /// [`AssetPayload::Unreadable`] records. With the old
    /// `Inline(Vec::new())` default this case returned **`Ok(vec![])`**, so
    /// every caller treated "we cannot read this" as "the publisher shipped
    /// an empty file" — a wrong answer on the *success* path, which no
    /// amount of care at a call site can recover.
    NoPayload,
    /// **The publisher used a payload arm this build does not implement.**
    /// Their bytes are fine and our reader is short — the opposite
    /// attribution from [`NoPayload`](Self::NoPayload), which is why it is a
    /// separate variant rather than a shared "cannot read it".
    ///
    /// **No longer reachable via `child`** — see
    /// [`InvalidForType`](Self::InvalidForType). This is now a tag EMBED §3
    /// does not yet define: a genuinely newer producer against an older
    /// reader, which is the one case where naming our own gap is right.
    UnsupportedPayload { tag: String },
    /// **The publisher used a legal `embed-payload` arm that
    /// `app/site-asset` does not admit** — today exactly `child`.
    ///
    /// A-30, ruled 2026-09-10 (SITE v0.5.1 §4). **Their schema violation, not
    /// our gap and not a malformed byte** — the three attributions stay apart
    /// because they route to different people. See
    /// [`AssetPayload::InvalidForType`](super::format::AssetPayload::InvalidForType)
    /// for why the obvious fix (fold it into [`NoPayload`](Self::NoPayload))
    /// was the wrong one.
    InvalidForType { tag: String },
    /// The pointer's `system/content/blob` is not in the lookup's reach.
    /// **Not an error state for a cold consumer** — it is what a pointer
    /// asset looks like before its closure has been fetched.
    BlobMissing(Hash),
    /// A chunk the blob names is absent. The closure is incomplete.
    ChunkMissing(Hash),
    /// The blob or a chunk is present and does not decode as one.
    Malformed(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::UnsupportedPayload { tag } => write!(
                f,
                "the asset uses the `{tag}` payload arm, which this build does not implement"
            ),
            // Names the publisher and names the rule, because this one is
            // actionable at the origin rather than here.
            ResolveError::InvalidForType { tag } => write!(
                f,
                "the asset carries a `{tag}` payload, which an app/site-asset must not \
                 (APP-CONVENTION-SEMANTIC-CONTENT-SITE §4) — the publisher must emit \
                 an inline or pointer payload"
            ),
            ResolveError::NoPayload => write!(
                f,
                "the asset entity carries no payload this build can read (no `payload` key, and \
                 no retired top-level `bytes`)"
            ),
            ResolveError::BlobMissing(h) => {
                write!(f, "asset blob {} is not held locally", h.to_hex())
            }
            ResolveError::ChunkMissing(h) => {
                write!(f, "asset chunk {} is missing from the blob's closure", h.to_hex())
            }
            ResolveError::Malformed(why) => write!(f, "asset content does not decode: {why}"),
        }
    }
}

/// Resolve an asset to its bytes, given a way to look a content entity up by
/// hash.
///
/// Inline is a clone. A pointer walks the blob's chunk list through the
/// **kernel's own decode** — the blob and its chunks are copied into a
/// scratch [`MemoryContentStore`] and handed to `entity_content::reassemble`,
/// rather than this module re-implementing the `system/content/blob` wire
/// shape. Re-implementing it is how two expressions of one rule get created
/// (C15), and this one has a kernel-side owner already.
///
/// `lookup` is a closure rather than a `&Arc<dyn ContentStore>` because the
/// three arms that call this hold three different things: the publisher has a
/// real store, the browser has `ctx.store().get_by_hash`, and the HTTP
/// consumer has a map it just fetched into.
pub fn resolve<F>(asset: &SiteAsset, lookup: F) -> Result<Vec<u8>, ResolveError>
where
    F: Fn(&Hash) -> Option<Entity>,
{
    let blob_hash = match &asset.payload {
        AssetPayload::Inline(bytes) => return Ok(bytes.clone()),
        AssetPayload::Pointer(h) => *h,
        // Never `Ok(vec![])`. See `ResolveError::NoPayload`.
        AssetPayload::Unreadable => return Err(ResolveError::NoPayload),
        AssetPayload::Unsupported { tag } => {
            return Err(ResolveError::UnsupportedPayload { tag: tag.clone() })
        }
        // A-30: theirs, not ours, and not malformed. Three attributions.
        AssetPayload::InvalidForType { tag } => {
            return Err(ResolveError::InvalidForType { tag: tag.clone() })
        }
    };
    reassemble_blob(blob_hash, lookup)
}

/// A `system/content/blob`'s bytes, reassembled from a content lookup — the
/// pointer arm of [`resolve`], with no asset around it.
///
/// Public because a site asset is not the only thing that points at a blob: an
/// app's asset bundle (`crate::apps::assets`) names one per file. One walk of
/// the blob wire shape, two callers — C15, the same reason
/// [`blob_closure_via`] is shared.
pub fn reassemble_blob<F>(blob_hash: Hash, lookup: F) -> Result<Vec<u8>, ResolveError>
where
    F: Fn(&Hash) -> Option<Entity>,
{
    let blob = lookup(&blob_hash).ok_or(ResolveError::BlobMissing(blob_hash))?;
    let chunks = chunk_hashes_of(&blob).map_err(ResolveError::Malformed)?;
    let scratch = MemoryContentStore::new();
    scratch
        .put(blob)
        .map_err(|e| ResolveError::Malformed(format!("stage blob: {e}")))?;
    let scratch: Arc<dyn ContentStore> = Arc::new(scratch);
    for ch in &chunks {
        let e = lookup(ch).ok_or(ResolveError::ChunkMissing(*ch))?;
        scratch
            .put(e)
            .map_err(|e| ResolveError::Malformed(format!("stage chunk: {e}")))?;
    }
    entity_content::reassemble(&scratch, &blob_hash)
        .map_err(|e| ResolveError::Malformed(e.to_string()))
}

/// [`resolve`] against a real content store — the publisher and Direct-arm
/// browser shape.
pub fn resolve_in(asset: &SiteAsset, store: &Arc<dyn ContentStore>) -> Result<Vec<u8>, ResolveError> {
    resolve(asset, |h| store.get(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<dyn ContentStore> {
        Arc::new(MemoryContentStore::new())
    }

    /// **Both sides of the ceiling, and this is the gate the `[MUST]` asks
    /// for.** `F-5` cost us the lesson directly: a bound asserted from one
    /// side passes for a walker that went one level, so a test that only
    /// checks "16,385 is a pointer" passes for a publisher that pointers
    /// everything, and one that only checks "16,384 is inline" passes for the
    /// all-inline publisher this change exists to retire.
    #[test]
    fn the_inline_ceiling_is_asserted_from_both_sides() {
        let s = store();
        let at = stage("image/png", vec![7u8; INLINE_PAYLOAD_MAX], &s).unwrap();
        assert!(
            at.asset.inline_bytes().is_some(),
            "{INLINE_PAYLOAD_MAX} is the LAST conformant inline size — \
             `.size (1..16384)` is inclusive"
        );
        assert!(at.content.is_empty(), "an inline asset stages no content entities");

        let over = stage("image/png", vec![7u8; INLINE_PAYLOAD_MAX + 1], &s).unwrap();
        assert!(
            over.asset.pointer().is_some(),
            "one byte over the ceiling MUST take the pointer arm (content-site §4)"
        );
        assert!(
            !over.content.is_empty(),
            "a pointer asset carries its blob closure — otherwise the publish \
             emits an asset whose bytes nothing can reach"
        );
    }

    /// **Zero is OUTSIDE the inline range, so an empty asset takes the
    /// pointer arm.** `inline-payload` is `bstr .size (1..16384)` — the lower
    /// bound is as normative as the upper one, and a `<=` spelling silently
    /// emits a payload no conformant decoder should accept.
    ///
    /// Asserted rather than refused: an empty figure is a publisher's
    /// problem, not a reason to fail their whole publish, and the pointer arm
    /// represents it exactly (`total_size: 0`, empty chunk list).
    #[test]
    fn an_empty_asset_is_outside_the_inline_range_and_takes_the_pointer_arm() {
        let s = store();
        let staged = stage("image/png", Vec::new(), &s).expect("an empty asset stages");
        assert!(
            staged.asset.pointer().is_some(),
            "0 is not in `.size (1..16384)`, so inline cannot represent it"
        );
        assert_eq!(
            resolve_in(&staged.asset, &s).expect("resolves"),
            Vec::<u8>::new(),
            "and it round-trips to the empty vec rather than to an error"
        );
        // One byte IS in range — the other side of the lower bound, for the
        // same reason both sides of the upper one are asserted.
        let one = stage("image/png", vec![7], &s).unwrap();
        assert_eq!(one.asset.inline_bytes(), Some(&[7u8][..]), "1 is the FIRST inline size");
    }

    #[test]
    fn a_pointer_asset_round_trips_to_the_same_bytes() {
        let s = store();
        // Deliberately non-uniform: an all-zero buffer makes every chunker
        // agree by accident, which is the wrong fixture for a chunk walk.
        let bytes: Vec<u8> = (0..40_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let staged = stage("application/octet-stream", bytes.clone(), &s).unwrap();
        assert!(staged.asset.pointer().is_some());
        let back = resolve_in(&staged.asset, &s).expect("resolves");
        assert_eq!(back, bytes, "reassembly is byte-exact");
    }

    #[test]
    fn a_pointer_whose_blob_we_do_not_hold_says_so_rather_than_yielding_nothing() {
        let s = store();
        let bytes: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        let staged = stage("image/png", bytes, &s).unwrap();
        // An empty store: we hold the asset entity, not its closure. That is
        // the ordinary cold state of a +content-tier consumer, and it must be
        // distinguishable from "the publisher shipped an empty asset".
        let empty = store();
        match resolve_in(&staged.asset, &empty) {
            Err(ResolveError::BlobMissing(h)) => {
                assert_eq!(h, staged.asset.pointer().unwrap())
            }
            other => panic!("expected BlobMissing, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_chunk_is_not_a_missing_blob() {
        let s = store();
        let bytes: Vec<u8> = (0..30_000u32).map(|i| (i ^ 0x5a) as u8).collect();
        let staged = stage("image/png", bytes, &s).unwrap();
        // Hold the blob, withhold every chunk.
        let blob = staged.content[0].clone();
        let partial = MemoryContentStore::new();
        partial.put(blob).unwrap();
        let partial: Arc<dyn ContentStore> = Arc::new(partial);
        match resolve_in(&staged.asset, &partial) {
            Err(ResolveError::ChunkMissing(_)) => {}
            other => panic!("expected ChunkMissing, got {other:?}"),
        }
    }

    /// **THE REGRESSION THIS WHOLE ARM EXISTS FOR — an already-published
    /// asset must still resolve to its bytes.**
    ///
    /// The pointer change moved the wire shape from `{media_type, bytes}` to
    /// `{media_type, payload}`. Every asset on every already-published origin,
    /// and every asset already cached in a visitor's own tree, carries the
    /// old one. **Measured on a real 30,778-byte published figure before this
    /// test existed:** it decoded to `Inline(0 bytes)` and `resolve()`
    /// returned `Ok(0)` — a success carrying nothing, rendered as
    /// `data:image/png;base64,`.
    ///
    /// **And it is not self-healing, which is what makes it a blocker rather
    /// than a migration note.** `MultiResolver::resolve_page` tries the
    /// durable cache *first*, with no network, so a returning visitor never
    /// issues the fetch that would replace the stale entity. Republishing the
    /// origin does not reach them.
    #[test]
    fn an_asset_published_before_the_pointer_arm_still_resolves_to_its_bytes() {
        // The pre-`A-27` encoding, built by hand rather than by an old
        // constructor: the point is to pin the BYTES a previous build emitted,
        // and a helper that could drift with the code would not do that.
        let legacy = entity_entity::Entity::new(
            crate::content_site::format::SITE_ASSET_TYPE,
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("media_type".into()), entity_ecf::text("image/png")),
                (
                    entity_ecf::Value::Text("bytes".into()),
                    entity_ecf::Value::Bytes(b"\x89PNG\r\n-the-real-figure".to_vec()),
                ),
            ])),
        )
        .unwrap();

        let asset = SiteAsset::from_entity(&legacy);
        assert_eq!(asset.media_type, "image/png");
        assert_eq!(
            resolve(&asset, |_| None).expect("a legacy asset resolves"),
            b"\x89PNG\r\n-the-real-figure",
            "an asset published before the pointer arm must still render"
        );
        assert!(
            asset.from_legacy_encoding,
            "and it must SAY it came from the retired encoding, or the concession has no \
             instrument and can never be retired"
        );
    }

    /// ⚠ **A legacy OVERSIZED asset heals into a NON-CONFORMANT entity, and
    /// this test is the instrument for that concession rather than its
    /// approval.**
    ///
    /// `SiteAsset::to_entity` re-encodes into the declared shape, which the
    /// cache write-through calls *the repair* — and it is, for the shape. It
    /// is not one for the **size**: `inline-payload` is
    /// `bstr .size (1..16384)`, the legacy encoding had no bound at all, and
    /// **47% of the assets on a real published tree are over 16 KiB**. So every
    /// visitor to a papers site published before the pointer arm writes an
    /// oversized `inline` payload into their own tree today.
    ///
    /// **Why it is pinned rather than fixed here, and the distinction is the
    /// transferable half: AUTHORING and TRANSCRIBING are different acts.**
    /// Refusing at [`SiteAsset::to_entity`] would make the cache drop a figure
    /// it can render perfectly — D24's *"a cache that drops what it cannot
    /// re-verify turns an outage into a missing app"*, for a payload we CAN
    /// verify. The real repair is to re-[`stage`] through the content store, and
    /// that needs a store the Worker arm does not have a content verb for.
    /// **So the bound is enforced on the authoring side** —
    /// [`EmbedPayload::inline_is_conformant`], which `stage` consults — **and
    /// this arm is a stated bound, not coverage.**
    #[test]
    fn a_legacy_oversized_asset_heals_into_a_shape_that_is_declared_and_a_size_that_is_not() {
        let big = vec![7u8; INLINE_PAYLOAD_MAX + 1];
        let legacy = entity_entity::Entity::new(
            crate::content_site::format::SITE_ASSET_TYPE,
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("media_type".into()), entity_ecf::text("image/png")),
                (entity_ecf::Value::Text("bytes".into()), entity_ecf::Value::Bytes(big.clone())),
            ])),
        )
        .unwrap();

        let asset = SiteAsset::from_entity(&legacy);
        assert!(asset.from_legacy_encoding);
        // It still renders — which is the whole reason refusing would be worse.
        assert_eq!(resolve(&asset, |_| None).unwrap().len(), big.len());

        // And the cache write-through's re-encode carries it inline, at a size
        // the CDDL excludes.
        let healed = SiteAsset::from_entity(&asset.to_entity());
        assert!(
            !healed.from_legacy_encoding,
            "the shape healed — that half of the claim is true"
        );
        assert_eq!(
            healed.inline_bytes().map(<[u8]>::len),
            Some(big.len()),
            "and the size did not: this is the stated bound, measured"
        );
        assert!(
            !EmbedPayload::inline_is_conformant(big.len()),
            "which is exactly the range the authoring side refuses"
        );
    }

    /// **An unreadable payload is an error, never `Ok(vec![])`.**
    ///
    /// The other half of the same defect: with `Inline(Vec::new())` as the
    /// default, *"we cannot read this"* and *"the publisher shipped an empty
    /// file"* were the same value **on the success path**, where no amount of
    /// care at a call site can recover it.
    ///
    /// The three shapes below are the ones that reach it, and the third is
    /// the one EMBED §3 names: a payload map with no `tag` is exactly the
    /// untagged/ambiguous case a decoder MUST reject.
    #[test]
    fn an_unreadable_payload_is_an_error_and_never_an_empty_success() {
        let build = |pairs: Vec<(entity_ecf::Value, entity_ecf::Value)>| {
            entity_entity::Entity::new(
                crate::content_site::format::SITE_ASSET_TYPE,
                entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs)),
            )
            .unwrap()
        };
        let mt =
            || (entity_ecf::Value::Text("media_type".into()), entity_ecf::text("image/png"));

        // (a) no payload and no legacy bytes.
        let bare = build(vec![mt()]);
        // (b) a payload with an unknown tag.
        let unknown = build(vec![
            mt(),
            (
                entity_ecf::Value::Text("payload".into()),
                entity_ecf::Value::Map(vec![(
                    entity_ecf::Value::Text("tag".into()),
                    entity_ecf::text("something-we-do-not-know"),
                )]),
            ),
        ]);
        // (c) an UNTAGGED payload carrying bytes — EMBED §3's named case.
        let untagged = build(vec![
            mt(),
            (
                entity_ecf::Value::Text("payload".into()),
                entity_ecf::Value::Map(vec![(
                    entity_ecf::Value::Text("bytes".into()),
                    entity_ecf::Value::Bytes(b"tempting".to_vec()),
                )]),
            ),
        ]);

        // (d) a `child` payload — correctly tagged, well-formed, unambiguous,
        // and NOT admitted by `app/site-asset` (SITE v0.5.1 §4's `[MUST NOT]`,
        // A-30). The `ref` shape does not matter: the tag alone decides.
        //
        // **Both `ref` shapes are exercised, and that became load-bearing when
        // the union moved to `crate::embed`.** The shared decoder now *parses*
        // the reference, so a `child` reaches this layer down two paths — as
        // `Ok(Child)` and as `Err(BadRef)` — and only one of them existed
        // before. They must land on the same answer: this field refuses the arm
        // whatever its reference says, and it never resolves one, so
        // adjudicating an atom we would not follow would report the publisher's
        // smaller mistake and hide the one that decides the outcome. Row (d)'s
        // hash is deliberately too short to be a content hash (the `BadRef`
        // path); row (e) carries a well-formed one.
        let child = build(vec![
            mt(),
            (
                entity_ecf::Value::Text("payload".into()),
                entity_ecf::Value::Map(vec![
                    (entity_ecf::Value::Text("tag".into()), entity_ecf::text("child")),
                    (
                        entity_ecf::Value::Text("ref".into()),
                        entity_ecf::Value::Map(vec![
                            (entity_ecf::Value::Text("tag".into()), entity_ecf::text("pin")),
                            (entity_ecf::Value::Text("peer".into()), entity_ecf::text("SOMEPEER")),
                            (
                                entity_ecf::Value::Text("hash".into()),
                                entity_ecf::Value::Bytes(vec![0x00, 0x01, 0x02]),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]);

        // (e) the same excluded arm carrying a REFERENCE THAT PARSES.
        let child_valid_ref = build(vec![
            mt(),
            (
                entity_ecf::Value::Text("payload".into()),
                entity_ecf::Value::Map(vec![
                    (entity_ecf::Value::Text("tag".into()), entity_ecf::text("child")),
                    (
                        entity_ecf::Value::Text("ref".into()),
                        crate::entity_ref::EntityRef::pin(
                            "QmSomePeer",
                            entity_hash::Hash::compute("test/note", b"sibling"),
                        )
                        .to_value(),
                    ),
                ]),
            ),
        ]);

        // **The FOUR do NOT collapse into one answer, and that is the second
        // half of this test — SITE §9's vector asserts the DISCRIMINATION, not
        // the refusal.** A bare entity and an untagged payload are malformed
        // (EMBED §3's MUST); an unknown *tag* is an arm we have not built,
        // which is our gap; and `child` is a legal embed arm this type refuses,
        // which is the publisher's schema violation. Three different people
        // fix those three.
        //
        // An earlier cut asserted `NoPayload` for the first three and had to be
        // corrected. **The `child` row was very nearly a fifth instance of the
        // same mistake**: our own plan proposed folding it into `NoPayload`
        // ("nothing else changes"), which arch caught — a run reporting the
        // child case as malformed OR as an unimplemented arm fails the vector.
        for (what, ent, want) in [
            ("bare", bare, ResolveError::NoPayload),
            (
                "unknown tag",
                unknown,
                ResolveError::UnsupportedPayload { tag: "something-we-do-not-know".into() },
            ),
            ("untagged", untagged, ResolveError::NoPayload),
            ("child", child, ResolveError::InvalidForType { tag: "child".into() }),
            (
                "child with a reference that parses",
                child_valid_ref,
                ResolveError::InvalidForType { tag: "child".into() },
            ),
        ] {
            let asset = SiteAsset::from_entity(&ent);
            assert_eq!(
                resolve(&asset, |_| None),
                Err(want),
                "{what}: must not resolve to an empty success, and must name the right fault"
            );
            assert!(!asset.from_legacy_encoding, "{what}: nothing legacy was read");
        }
    }

    /// **The legacy arm fires ONLY when `payload` is absent entirely.** A
    /// present-but-unreadable `payload` must not fall back to a top-level
    /// `bytes` — an open type may grow that key meaning something else, and
    /// the narrow rule is what stops this arm claiming entities it has no
    /// business reading.
    #[test]
    fn a_present_but_unreadable_payload_does_not_fall_back_to_the_legacy_key() {
        let ent = entity_entity::Entity::new(
            crate::content_site::format::SITE_ASSET_TYPE,
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("media_type".into()), entity_ecf::text("image/png")),
                (
                    entity_ecf::Value::Text("bytes".into()),
                    entity_ecf::Value::Bytes(b"not mine to take".to_vec()),
                ),
                (
                    entity_ecf::Value::Text("payload".into()),
                    entity_ecf::Value::Map(vec![(
                        entity_ecf::Value::Text("tag".into()),
                        entity_ecf::text("a-future-arm"),
                    )]),
                ),
            ])),
        )
        .unwrap();
        let asset = SiteAsset::from_entity(&ent);
        assert_eq!(
            resolve(&asset, |_| None),
            Err(ResolveError::UnsupportedPayload { tag: "a-future-arm".into() }),
            "a payload arm we do not implement is reported as such — and crucially the \
             top-level `bytes` sitting right beside it is NOT taken"
        );
        assert!(!asset.from_legacy_encoding);
    }

    /// Re-encoding heals: a legacy asset decoded and written back out carries
    /// the **declared** shape, and the flag falls to `false`. That is what
    /// makes the cache write-through a repair rather than a second producer
    /// of the retired encoding.
    #[test]
    fn re_encoding_a_legacy_asset_emits_the_declared_shape() {
        let legacy = entity_entity::Entity::new(
            crate::content_site::format::SITE_ASSET_TYPE,
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("media_type".into()), entity_ecf::text("image/svg+xml")),
                (
                    entity_ecf::Value::Text("bytes".into()),
                    entity_ecf::Value::Bytes(b"<svg/>".to_vec()),
                ),
            ])),
        )
        .unwrap();
        let healed = SiteAsset::from_entity(&legacy).to_entity();
        assert_ne!(healed.data, legacy.data, "the shape changed on the way out");
        let back = SiteAsset::from_entity(&healed);
        assert_eq!(back.inline_bytes(), Some(&b"<svg/>"[..]));
        assert!(!back.from_legacy_encoding, "the re-encoded copy is no longer legacy");
    }

    /// The staged closure is exactly what a projection has to carry: drop any
    /// member and the asset is unreachable. Pins that `blob_closure` returns
    /// the blob **and** the chunks, not one or the other.
    #[test]
    fn the_staged_closure_is_sufficient_on_its_own() {
        let s = store();
        let bytes: Vec<u8> = (0..50_000u32).map(|i| (i.wrapping_mul(31) >> 3) as u8).collect();
        let staged = stage("image/svg+xml", bytes.clone(), &s).unwrap();
        // Rebuild a store from ONLY the returned entities.
        let fresh = MemoryContentStore::new();
        for e in &staged.content {
            fresh.put(e.clone()).unwrap();
        }
        let fresh: Arc<dyn ContentStore> = Arc::new(fresh);
        assert_eq!(resolve_in(&staged.asset, &fresh).expect("resolves"), bytes);
    }
}


