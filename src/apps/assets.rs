//! `assets` — an app's **asset bundles**: named sets of files the app ships
//! with and reads by key, host side, as a **local extension**.
//!
//! An entity-app is one self-contained `.html` file (entity-apps' `build.py`
//! concatenates it), and that is the right shape for code. It is the wrong
//! shape for what some apps need *beside* their code — a kernel, a root
//! filesystem, a package repository, textures, a model. Those are large, and
//! most of them are never read on a given run, so they must be fetched **on
//! demand** rather than inlined.
//!
//! ## The contract does not know what a file is
//!
//! `DESIGN-2026-09-13-FILES-ACROSS-THE-APP-BOUNDARY` §3 calls this the
//! **resource** role. The host's whole vocabulary is:
//!
//! - the catalog declares bundle **names** ([`MANIFEST_KEY`], `x-assets`);
//! - the app asks for a **key** in a bundle ([`MSG_GET`]);
//! - the host answers with the bytes or a refusal ([`MSG_ASSET`]).
//!
//! The host never learns that `blobs/8711…` is a 9p inode body, and the app
//! never learns where the bytes came from — its own tree, a publisher's
//! origin, a peer. Nothing here is VM-shaped, and a test pins that
//! ([`the_vocabulary_names_no_kind_of_content`](tests)).
//!
//! ## One index per bundle, and it is the only thing that can go stale
//!
//! A bundle is **one entity**, an [`AssetIndex`] (`key → {size, blob}`), plus a
//! `system/content/blob` closure per file. That is deliberate, and it is what
//! makes a dynamic bundle cheap:
//!
//! - **The index is the bundle's root.** Republishing a bundle — adding a
//!   package, removing one — moves exactly one pointer. The app is not told
//!   and does not need to be; it asks for keys, and the next launch reads the
//!   new index.
//! - **Everything under the index is content-addressed**, so a file body can
//!   never be stale and needs no currency check (D24 applies to the index
//!   alone). A per-file tree binding would have cost one freshness request per
//!   file over HTTP; this costs one per bundle per launch.
//! - **Identical files across bundles and apps dedup in the store**, because
//!   the blob is keyed by its bytes.
//!
//! ## Why every name here starts with `x-`
//!
//! For the reason [`crate::app_files`] gives: the message contract and the
//! catalog belong to entity-apps, and we have said we will not mint vocabulary
//! in them unilaterally. If they rule on a shape, the rename is this module's
//! constants and one catalog key.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_store::{ContentStore, MemoryContentStore};

use crate::content_site::asset_store::{self, ResolveError};
use crate::content_site::format::CANONICAL_CHUNK_SIZE;

/// The catalog key an app sets to the list of bundle names it reads.
pub const MANIFEST_KEY: &str = "x-assets";

/// The bundle index entity type. App-tier vocabulary in the existing
/// `app/app-*` family (`app-catalog`, `app-bundle`, `app-save`).
pub const INDEX_TYPE: &str = "app/app-asset-index";

/// app → host: `{id, bundle, key}` — "give me this key".
pub const MSG_GET: &str = "x-asset-get";
/// host → app: `{id, bundle, key, ok, size?, data?, reason?}` — the answer.
pub const MSG_ASSET: &str = "x-asset";
/// The `init` field listing the bundles this host will answer for, so an app
/// can choose host mode before its first request instead of timing one out.
pub const INIT_KEY: &str = "x-assets";

/// Longest key accepted, in bytes. Generous for a path, and a bound on what a
/// sandboxed page can make the host allocate before it refuses.
pub const MAX_KEY_BYTES: usize = 1024;
/// Longest bundle name accepted.
pub const MAX_BUNDLE_NAME_BYTES: usize = 64;

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// A bundle name is one path segment of `[a-z0-9._-]`, not starting with `.`.
///
/// Narrow on purpose: it becomes a tree segment and a directory name at ingest
/// (`<id>.assets/<bundle>/`), and nothing about a bundle needs more.
pub fn valid_bundle_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_BUNDLE_NAME_BYTES
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// Validate a key: a relative `/`-separated path with no empty, `.` or `..`
/// segment, no backslash and no control character.
///
/// **Refused rather than normalised.** `a//b` and `./a` name the same file as
/// `a/b` and `a` on a filesystem, but a key is an index lookup, and silently
/// rewriting one would let two spellings of a request succeed where the index
/// holds one. A refusal tells the app author exactly which spelling to use.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && !key.starts_with('/')
        && !key.contains('\\')
        && !key.chars().any(char::is_control)
        && key.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// One file in a bundle: its length and the hash of its `system/content/blob`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetEntry {
    pub size: u64,
    pub blob: Hash,
}

/// A bundle: every key it carries. A `BTreeMap` so encoding is deterministic
/// and an unchanged bundle republishes to the same hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetIndex {
    pub entries: BTreeMap<String, AssetEntry>,
}

/// Why an index entity could not be read. Kept apart from "no index", which is
/// a `None` one layer up: *this bundle does not exist* and *it exists and we
/// cannot read it* send an app author to different places (AP40).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexError {
    /// Some other entity is at the index path.
    WrongType(String),
    /// The right type, and the body does not decode as an index.
    Malformed(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::WrongType(t) => write!(f, "expected {INDEX_TYPE}, found {t}"),
            IndexError::Malformed(why) => write!(f, "asset index does not decode: {why}"),
        }
    }
}

impl AssetIndex {
    /// Total bytes across every file — what a full mirror of the bundle costs.
    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.size).sum()
    }

    /// Every distinct blob the index names. Several keys may share one.
    pub fn blobs(&self) -> BTreeSet<Hash> {
        self.entries.values().map(|e| e.blob).collect()
    }

    pub fn to_entity(&self) -> Entity {
        let entries = self
            .entries
            .iter()
            .map(|(key, e)| {
                (
                    text(key.clone()),
                    Value::Map(vec![
                        (text("size"), integer(e.size as i64)),
                        (text("blob"), ecf_bytes(e.blob.to_bytes())),
                    ]),
                )
            })
            .collect();
        let data = to_ecf(&Value::Map(vec![(text("entries"), Value::Map(entries))]));
        Entity::new(INDEX_TYPE, data).expect("an asset index always encodes")
    }

    /// Decode an index. **All or nothing**: one malformed row fails the whole
    /// index, because a short index would answer "not found" for a key the
    /// publisher shipped — a wrong answer on the path that looks like success.
    pub fn from_entity(entity: &Entity) -> Result<Self, IndexError> {
        if entity.entity_type != INDEX_TYPE {
            return Err(IndexError::WrongType(entity.entity_type.clone()));
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|e| IndexError::Malformed(e.to_string()))?;
        let map = value
            .as_map()
            .ok_or_else(|| IndexError::Malformed("body is not a map".into()))?;
        let entries = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("entries"))
            .and_then(|(_, v)| v.as_map())
            .ok_or_else(|| IndexError::Malformed("no `entries` map".into()))?;
        let mut out = AssetIndex::default();
        for (k, v) in entries {
            let key = k
                .as_text()
                .ok_or_else(|| IndexError::Malformed("a key is not text".into()))?;
            if !valid_key(key) {
                return Err(IndexError::Malformed(format!("invalid key {key:?}")));
            }
            let row = v
                .as_map()
                .ok_or_else(|| IndexError::Malformed(format!("row {key:?} is not a map")))?;
            let (mut size, mut blob) = (None, None);
            for (rk, rv) in row {
                match rk.as_text() {
                    Some("size") => {
                        size = rv.as_integer().and_then(|i| u64::try_from(i128::from(i)).ok())
                    }
                    Some("blob") => blob = rv.as_bytes().and_then(|b| Hash::from_bytes(b).ok()),
                    _ => {}
                }
            }
            let (Some(size), Some(blob)) = (size, blob) else {
                return Err(IndexError::Malformed(format!("row {key:?} lacks size or blob")));
            };
            out.entries.insert(key.to_string(), AssetEntry { size, blob });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Ingest (authoring): a directory → an index + its content closure
// ---------------------------------------------------------------------------

/// One app's bundle as ingested: which app, which bundle, the index, and every
/// `system/content` entity the index needs.
///
/// The closure is a **return value**, for `asset_store::stage`'s reason: the
/// publisher must project it and the seed must store it, and a function that
/// only wrote into a scratch store would leave that to memory (AP44).
#[derive(Debug, Clone)]
pub struct IngestedBundle {
    pub app_id: String,
    pub bundle: String,
    pub index: AssetIndex,
    /// Blobs and chunks, deduplicated by hash.
    pub content: Vec<Entity>,
}

/// Read every file under `dir` into an index keyed by its path relative to
/// `dir`, chunked at the canonical size.
///
/// Symlinks are **followed** — a bundle staged by linking a build output is
/// the ordinary case (the run-env rig does exactly that). A key that would be
/// invalid ([`valid_key`]) is an error, not a skip: a file the publisher put in
/// the directory and the app cannot ask for is a packaging mistake worth
/// stopping on.
pub fn read_bundle_dir(dir: &Path) -> Result<(AssetIndex, Vec<Entity>), String> {
    if !dir.is_dir() {
        return Err(format!("asset bundle {} is not a directory", dir.display()));
    }
    let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let mut index = AssetIndex::default();
    let mut content: BTreeMap<Hash, Entity> = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut listing: Vec<_> = std::fs::read_dir(&d)
            .map_err(|e| format!("read {}: {e}", d.display()))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("read {}: {e}", d.display()))?;
        listing.sort_by_key(|e| e.file_name());
        for item in listing {
            let path = item.path();
            // `metadata` follows symlinks; `file_type` on the entry would not.
            let meta = std::fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !valid_key(&rel) {
                return Err(format!("{} cannot be an asset key ({rel:?})", path.display()));
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let (entry, closure) = stage_bytes(&bytes, &store)?;
            for e in closure {
                content.entry(e.content_hash).or_insert(e);
            }
            index.entries.insert(rel, entry);
        }
    }
    Ok((index, content.into_values().collect()))
}

/// Chunk `bytes` into `store` and return the entry plus the blob closure.
///
/// **Always the pointer shape, never inline.** A bundle's index is fetched
/// whole on every launch; inlining small files into it would put every byte of
/// every small file into the one request that is not content-addressed.
pub fn stage_bytes(
    bytes: &[u8],
    store: &Arc<dyn ContentStore>,
) -> Result<(AssetEntry, Vec<Entity>), String> {
    let blob = entity_content::create_blob_fastcdc(store, bytes, CANONICAL_CHUNK_SIZE)
        .map_err(|e| format!("chunk {} bytes: {e}", bytes.len()))?;
    let closure = asset_store::blob_closure(store, &blob)?;
    Ok((AssetEntry { size: bytes.len() as u64, blob }, closure))
}

/// Where a running app's bundles come from — built by the Apps window from the
/// catalog it resolved, handed to the player. Native so the window's
/// construction of it stays testable; the serving half is `dom::app_assets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetSource {
    /// The peer whose catalog listed the app — the publisher, whose tree the
    /// index lives in (cached in mine at that peer's natural path).
    pub apps_peer: String,
    pub set: String,
    pub app_id: String,
    /// The publisher's origin, when it is not this profile's own tree. `None`
    /// means answer from the local tree only.
    pub origin: Option<String>,
    /// The bundle names the catalog entry declares.
    pub bundles: Vec<String>,
}

// ---------------------------------------------------------------------------
// Serving (the host's decision, pure)
// ---------------------------------------------------------------------------

/// Why the host did not answer a request with bytes. **Each names whose
/// problem it is**, because an app author and a publisher go to different
/// places for each — [`crate::app_files::Refusal`]'s rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The app's catalog entry does not declare this bundle. The app's defect,
    /// or the catalog's.
    NotDeclared,
    /// A request with no usable `bundle` name. The app's defect.
    BadBundle,
    /// A key [`valid_key`] refuses. The app's defect.
    BadKey,
    /// The bundle's index was read and does not carry this key. Could be
    /// either side — which is exactly why it is not folded into a transport
    /// failure.
    NotFound,
    /// We could not obtain the index or the bytes: nothing held and the source
    /// did not answer, or this arm cannot hold content. **Not** "not found".
    Unavailable(String),
    /// Bytes or an index are present and do not decode, or reassemble to a
    /// length the index does not claim. The publisher's defect.
    Unreadable(String),
}

impl Refusal {
    /// Stable wire code for `reason`. An app branches on this, never on prose.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NotDeclared => "not-declared",
            Refusal::BadBundle => "bad-bundle",
            Refusal::BadKey => "bad-key",
            Refusal::NotFound => "not-found",
            Refusal::Unavailable(_) => "unavailable",
            Refusal::Unreadable(_) => "unreadable",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotDeclared => write!(f, "the app does not declare this asset bundle"),
            Refusal::BadBundle => write!(f, "the request names no usable bundle"),
            Refusal::BadKey => write!(f, "the request's key is not a valid asset key"),
            Refusal::NotFound => write!(f, "the bundle does not carry this key"),
            Refusal::Unavailable(why) => write!(f, "the asset could not be obtained: {why}"),
            Refusal::Unreadable(why) => write!(f, "the asset is present and unreadable: {why}"),
        }
    }
}

/// Admit a request before any lookup: the bundle is declared and the key is a
/// key. Cheap and synchronous, so a malformed request never starts a fetch.
pub fn admit(declared: &[String], bundle: Option<&str>, key: Option<&str>) -> Result<(), Refusal> {
    let bundle = bundle.filter(|b| valid_bundle_name(b)).ok_or(Refusal::BadBundle)?;
    if !declared.iter().any(|d| d == bundle) {
        return Err(Refusal::NotDeclared);
    }
    let key = key.ok_or(Refusal::BadKey)?;
    if !valid_key(key) {
        return Err(Refusal::BadKey);
    }
    Ok(())
}

/// The content hashes an entry still needs before it can be reassembled:
/// the blob alone if the blob is not held, else whichever chunks are missing.
/// Empty means [`resolve_entry`] will not report a missing piece.
pub fn missing_content<F>(entry: &AssetEntry, lookup: F) -> Result<Vec<Hash>, Refusal>
where
    F: Fn(&Hash) -> Option<Entity>,
{
    let Some(blob) = lookup(&entry.blob) else {
        return Ok(vec![entry.blob]);
    };
    let chunks = asset_store::chunk_hashes_of(&blob).map_err(Refusal::Unreadable)?;
    Ok(chunks.into_iter().filter(|h| lookup(h).is_none()).collect())
}

/// Reassemble an entry's bytes and **check the length the index claims**.
///
/// A blob that reassembles to a different length than its index row is a
/// publisher fault that would otherwise surface inside the app as a corrupt
/// file — for a VM, a guest binary that segfaults with nothing pointing here.
pub fn resolve_entry<F>(entry: &AssetEntry, lookup: F) -> Result<Vec<u8>, Refusal>
where
    F: Fn(&Hash) -> Option<Entity>,
{
    let bytes = asset_store::reassemble_blob(entry.blob, lookup).map_err(|e| match e {
        ResolveError::BlobMissing(h) | ResolveError::ChunkMissing(h) => {
            Refusal::Unavailable(format!("content {} is not held", h.to_hex()))
        }
        other => Refusal::Unreadable(other.to_string()),
    })?;
    if bytes.len() as u64 != entry.size {
        return Err(Refusal::Unreadable(format!(
            "reassembled {} bytes where the index claims {}",
            bytes.len(),
            entry.size
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<dyn ContentStore> {
        Arc::new(MemoryContentStore::new())
    }

    fn lookup_in(content: &[Entity]) -> impl Fn(&Hash) -> Option<Entity> + '_ {
        move |h| content.iter().find(|e| &e.content_hash == h).cloned()
    }

    #[test]
    fn keys_are_relative_paths_and_every_other_spelling_is_refused() {
        for ok in ["fs.json", "blobs/8711abcd", "a/b/c.txt", "x86/APKINDEX.tar.gz", "..hidden"] {
            assert!(valid_key(ok), "{ok:?} should be a key");
        }
        for bad in ["", "/abs", "a//b", "./a", "a/./b", "../x", "a/..", "a\\b", "a\nb", "a/"] {
            assert!(!valid_key(bad), "{bad:?} must be refused");
        }
        assert!(!valid_key(&"k".repeat(MAX_KEY_BYTES + 1)));
        assert!(valid_key(&"k".repeat(MAX_KEY_BYTES)));
    }

    #[test]
    fn bundle_names_are_one_narrow_segment() {
        for ok in ["guest", "alpine-3.21", "pkgs_x86", "a"] {
            assert!(valid_bundle_name(ok), "{ok:?}");
        }
        for bad in ["", ".git", "Guest", "a/b", "a b", "..", &"b".repeat(MAX_BUNDLE_NAME_BYTES + 1)] {
            assert!(!valid_bundle_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn an_index_round_trips_and_encodes_deterministically() {
        let s = store();
        let mut idx = AssetIndex::default();
        for (k, body) in [("b/two", b"two".as_slice()), ("a", b"one"), ("c/d/e", b"")] {
            let (entry, _) = stage_bytes(body, &s).unwrap();
            idx.entries.insert(k.to_string(), entry);
        }
        let ent = idx.to_entity();
        assert_eq!(ent.entity_type, INDEX_TYPE);
        assert_eq!(AssetIndex::from_entity(&ent).unwrap(), idx);
        // Same content, built in another order → same bytes, same hash.
        let mut again = AssetIndex::default();
        for (k, v) in idx.entries.iter().rev() {
            again.entries.insert(k.clone(), *v);
        }
        assert_eq!(again.to_entity().content_hash, ent.content_hash);
    }

    #[test]
    fn an_index_that_does_not_decode_is_an_error_and_never_a_short_index() {
        let other = Entity::new("app/app-bundle", to_ecf(&Value::Map(vec![]))).unwrap();
        assert!(matches!(AssetIndex::from_entity(&other), Err(IndexError::WrongType(_))));

        // One good row and one bad row: the whole index fails.
        let s = store();
        let (good, _) = stage_bytes(b"x", &s).unwrap();
        let data = to_ecf(&Value::Map(vec![(
            text("entries"),
            Value::Map(vec![
                (
                    text("good"),
                    Value::Map(vec![
                        (text("size"), integer(1)),
                        (text("blob"), ecf_bytes(good.blob.to_bytes())),
                    ]),
                ),
                (text("bad"), Value::Map(vec![(text("size"), integer(1))])),
            ]),
        )]));
        let ent = Entity::new(INDEX_TYPE, data).unwrap();
        assert!(matches!(AssetIndex::from_entity(&ent), Err(IndexError::Malformed(_))));

        // A key that could never be requested is malformed, not skipped.
        let data = to_ecf(&Value::Map(vec![(
            text("entries"),
            Value::Map(vec![(
                text("../escape"),
                Value::Map(vec![
                    (text("size"), integer(1)),
                    (text("blob"), ecf_bytes(good.blob.to_bytes())),
                ]),
            )]),
        )]));
        let ent = Entity::new(INDEX_TYPE, data).unwrap();
        assert!(matches!(AssetIndex::from_entity(&ent), Err(IndexError::Malformed(_))));
    }

    #[test]
    fn a_directory_becomes_an_index_keyed_by_relative_path_and_its_bytes_come_back() {
        let dir = std::env::temp_dir().join(format!("assets-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::create_dir_all(dir.join("nested/deeper")).unwrap();
        std::fs::write(dir.join("fs.json"), br#"{"version":3}"#).unwrap();
        std::fs::write(dir.join("blobs/aa"), b"same bytes").unwrap();
        std::fs::write(dir.join("nested/deeper/bb"), b"same bytes").unwrap();
        // Big enough to take more than one canonical chunk.
        let big: Vec<u8> = (0..(CANONICAL_CHUNK_SIZE * 3 + 7)).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(dir.join("big.bin"), &big).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("fs.json"), dir.join("linked.json")).unwrap();

        let (index, content) = read_bundle_dir(&dir).unwrap();
        let keys: Vec<&str> = index.entries.keys().map(String::as_str).collect();
        let mut want = vec!["big.bin", "blobs/aa", "fs.json", "nested/deeper/bb"];
        #[cfg(unix)]
        want.push("linked.json");
        want.sort();
        assert_eq!(keys, want);

        // Identical bytes under two keys share one blob, stored once.
        assert_eq!(index.entries["blobs/aa"].blob, index.entries["nested/deeper/bb"].blob);
        let distinct: BTreeSet<Hash> = content.iter().map(|e| e.content_hash).collect();
        assert_eq!(distinct.len(), content.len(), "content must be deduplicated");

        let lookup = lookup_in(&content);
        assert_eq!(resolve_entry(&index.entries["big.bin"], &lookup).unwrap(), big);
        assert_eq!(resolve_entry(&index.entries["fs.json"], &lookup).unwrap(), br#"{"version":3}"#);
        assert!(missing_content(&index.entries["big.bin"], &lookup).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_whose_name_cannot_be_a_key_stops_the_ingest() {
        let dir = std::env::temp_dir().join(format!("assets-badkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok"), b"1").unwrap();
        std::fs::write(dir.join("bad\u{7}name"), b"2").unwrap();
        let err = read_bundle_dir(&dir).unwrap_err();
        assert!(err.contains("cannot be an asset key"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_missing_is_the_blob_first_and_then_only_the_chunks_not_held() {
        let s = store();
        let big: Vec<u8> = (0..(CANONICAL_CHUNK_SIZE * 2 + 1)).map(|i| (i % 253) as u8).collect();
        let (entry, closure) = stage_bytes(&big, &s).unwrap();
        assert!(closure.len() >= 3, "blob + at least two chunks");

        let nothing = |_: &Hash| None;
        assert_eq!(missing_content(&entry, nothing).unwrap(), vec![entry.blob]);

        // Hold the blob and one chunk: exactly the other chunks are missing.
        let partial: Vec<Entity> = closure[..2].to_vec();
        let missing = missing_content(&entry, lookup_in(&partial)).unwrap();
        let expected: Vec<Hash> = closure[2..].iter().map(|e| e.content_hash).collect();
        assert_eq!(missing, expected);
        assert!(matches!(
            resolve_entry(&entry, lookup_in(&partial)),
            Err(Refusal::Unavailable(_))
        ));
    }

    #[test]
    fn a_length_the_index_does_not_claim_is_unreadable_not_a_success() {
        let s = store();
        let (mut entry, closure) = stage_bytes(b"twelve bytes", &s).unwrap();
        entry.size = 13;
        let got = resolve_entry(&entry, lookup_in(&closure));
        assert!(matches!(got, Err(Refusal::Unreadable(_))), "{got:?}");
    }

    #[test]
    fn admission_refuses_before_any_lookup_and_names_whose_problem_it_is() {
        let declared = vec!["guest".to_string()];
        assert_eq!(admit(&declared, Some("guest"), Some("fs.json")), Ok(()));
        assert_eq!(admit(&declared, Some("other"), Some("fs.json")), Err(Refusal::NotDeclared));
        assert_eq!(admit(&declared, None, Some("fs.json")), Err(Refusal::BadBundle));
        assert_eq!(admit(&declared, Some("../guest"), Some("fs.json")), Err(Refusal::BadBundle));
        assert_eq!(admit(&declared, Some("guest"), Some("../etc/passwd")), Err(Refusal::BadKey));
        assert_eq!(admit(&declared, Some("guest"), None), Err(Refusal::BadKey));
        assert_eq!(admit(&[], Some("guest"), Some("fs.json")), Err(Refusal::NotDeclared));
    }

    #[test]
    fn every_refusal_has_its_own_wire_code() {
        let all = [
            Refusal::NotDeclared,
            Refusal::BadBundle,
            Refusal::BadKey,
            Refusal::NotFound,
            Refusal::Unavailable(String::new()),
            Refusal::Unreadable(String::new()),
        ];
        let codes: BTreeSet<&str> = all.iter().map(Refusal::code).collect();
        assert_eq!(codes.len(), all.len(), "two refusals share a code");
    }

    /// The contract is about files, and this is the tripwire for it growing a
    /// concept of what they are. If a constant here ever needs to say `vm`,
    /// `kernel` or `package`, it has stopped being a generic host verb.
    #[test]
    fn the_vocabulary_names_no_kind_of_content() {
        for name in [MANIFEST_KEY, INDEX_TYPE, MSG_GET, MSG_ASSET, INIT_KEY] {
            for word in ["vm", "kernel", "image", "package", "guest", "alpine", "v86"] {
                assert!(!name.contains(word), "{name} names a kind of content ({word})");
            }
            if name != INDEX_TYPE {
                assert!(name.starts_with("x-"), "{name} must stay x-prefixed until entity-apps rules");
            }
        }
    }
}
