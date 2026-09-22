//! `workspace` — an app's **working files**, kept by the host in this profile's
//! tree, as a **local extension**.
//!
//! `DESIGN-2026-09-13-FILES-ACROSS-THE-APP-BOUNDARY` §3 names four roles for a
//! file crossing the app boundary. Two already exist: **state** (the `state`
//! message, one opaque value) and **handoff** (`x-file`, one file out to the
//! person's files). [`crate::apps::assets`] is the **resource** role, read-only
//! and owned by a publisher. This is the fourth, **workspace**: files the
//! *person* makes inside an app, read and written one at a time, and still there
//! next launch.
//!
//! ## The contract does not know what a file is for
//!
//! - the catalog opts in with [`MANIFEST_KEY`] (`"x-workspace": true`);
//! - `init` carries [`INIT_KEY`] so the app knows the host will answer;
//! - the app **lists** ([`MSG_LIST`]), **reads** one path ([`MSG_GET`]) and
//!   **saves** a batch of changed and removed paths ([`MSG_SAVE`]).
//!
//! Paths are relative to the app's workspace and validated exactly like an
//! asset key ([`crate::apps::assets::valid_key`]): no absolute path, no `..`,
//! no empty segment, no control character. The host never learns that
//! `.bash_history` is a shell's history.
//!
//! ## One entity per file, and why that differs from asset bundles
//!
//! A bundle is one index entity because a publisher's files are fetched over
//! HTTP, where one binding per file would be one freshness check per file.
//! A workspace is **ours**: it lives in this profile's own tree and is never
//! fetched, so that cost does not exist — and per-file entities buy what a
//! workspace wants: a save touches only the files that changed, and the files
//! are **visible in the entity tree** where a person can find them, at
//! [`crate::app_paths::app_workspace_prefix`]. (Design §13 said the same.)
//!
//! Each file is a [`WorkFile`] (`size`, `mode`, `mtime`, `blob`) at
//! `{prefix}{path}`, with its bytes as a `system/content/blob` closure, chunked
//! canonically.
//!
//! ## What a save may never do
//!
//! **Delete a file the app did not name.** The host removes exactly the paths
//! in `remove`. An app that could not read something back is not a reason to
//! lose it, so the rule that decides what was deleted lives in the app, which
//! is the only side that knows what the person actually removed.
//!
//! ## Two windows, one workspace
//!
//! Every window of an app shares its workspace, and a window saves what it
//! changed against what it last read. Without a check, the last save wins: a
//! file edited in two windows keeps only the later edit, and a file deleted in
//! one window is removed even after another saved a newer copy (field report
//! 2026-09-15, BACKLOG B-10). So a save may say, per path, which **version** it
//! expects the workspace to hold (`expect`; a version is the file's blob hash,
//! reported by the listing and by each save) — `null` for *I expect no file
//! here*. A path whose current version differs is refused as [`Refusal::Conflict`]
//! and left exactly as it is; the app decides what to do with its own copy.
//! Paths the save does not name in `expect` are not checked, which keeps an app
//! that never sends one working as before.
//!
//! ## Stated bounds
//!
//! - **Direct/IDB arm only.** The Worker/OPFS proxy has no content verb (see
//!   [`crate::writer_handle::WriterHandle::content_put`]), so on `?worker=1`
//!   every operation answers `unavailable` rather than an empty workspace.
//! - **Overwritten and removed files leave their blobs behind.** Reclaiming a
//!   blob safely needs to know no other file shares it, and the binding-safe
//!   reclaim only sees tree bindings, not a hash named inside an entity. Growth
//!   is bounded by what the person writes; kernel GC is the real answer.

use std::collections::BTreeSet;
use std::sync::Arc;

use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_store::{ContentStore, MemoryContentStore};

use crate::apps::assets::{self, AssetEntry};

/// The catalog key an app sets to `true` to get a workspace.
pub const MANIFEST_KEY: &str = "x-workspace";
/// The `init` field: `true` when this host answers the workspace verbs.
pub const INIT_KEY: &str = "x-workspace";

/// One working file. App-tier vocabulary in the `app/app-*` family.
pub const FILE_TYPE: &str = "app/app-work-file";

/// app → host: `{id}` — "what files do I have?"
pub const MSG_LIST: &str = "x-work-list";
/// host → app: `{id, ok, files: [{path, size, mode, mtime, version}], unreadable: [path], reason?}`
pub const MSG_LISTING: &str = "x-work-listing";
/// app → host: `{id, path}` — "give me this file".
pub const MSG_GET: &str = "x-work-get";
/// host → app: `{id, path, ok, data?, reason?}`
pub const MSG_FILE: &str = "x-work-file";
/// app → host: `{id, put: [{path, mode, mtime, data}], remove: [path], expect?: {path: version | null}}`
pub const MSG_SAVE: &str = "x-work-save";
/// host → app: `{id, ok, saved, removed, bytes, failed: [{path, reason}], versions: {path: version}, reason?}`
pub const MSG_SAVED: &str = "x-work-saved";

/// Largest single file a save accepts. The app holds it in memory to send it,
/// and so does the host to chunk it.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest total a single save accepts. An app with more to save sends it in
/// more than one batch.
pub const MAX_SAVE_BYTES: u64 = 64 * 1024 * 1024;
/// Most paths a single save may name (put + remove).
pub const MAX_SAVE_PATHS: usize = 4096;
/// Largest mode accepted: permission and set-id/sticky bits, never a file type.
pub const MAX_MODE: u32 = 0o7777;

// ---------------------------------------------------------------------------
// The file entity
// ---------------------------------------------------------------------------

/// One working file: its length, permission bits, modification time (seconds
/// since the epoch, as the app reported it) and the hash of its blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkFile {
    pub size: u64,
    pub mode: u32,
    pub mtime: u64,
    pub blob: Hash,
}

/// Why a stored file entity could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    WrongType(String),
    Malformed(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::WrongType(t) => write!(f, "expected {FILE_TYPE}, found {t}"),
            FileError::Malformed(why) => write!(f, "work file does not decode: {why}"),
        }
    }
}

impl WorkFile {
    pub fn to_entity(&self) -> Entity {
        let data = to_ecf(&Value::Map(vec![
            (text("size"), integer(self.size as i64)),
            (text("mode"), integer(self.mode as i64)),
            (text("mtime"), integer(self.mtime as i64)),
            (text("blob"), ecf_bytes(self.blob.to_bytes())),
        ]));
        Entity::new(FILE_TYPE, data).expect("a work file always encodes")
    }

    pub fn from_entity(entity: &Entity) -> Result<Self, FileError> {
        if entity.entity_type != FILE_TYPE {
            return Err(FileError::WrongType(entity.entity_type.clone()));
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|e| FileError::Malformed(e.to_string()))?;
        let map = value
            .as_map()
            .ok_or_else(|| FileError::Malformed("body is not a map".into()))?;
        let uint = |name: &str| {
            map.iter()
                .find(|(k, _)| k.as_text() == Some(name))
                .and_then(|(_, v)| v.as_integer())
                .and_then(|i| u64::try_from(i128::from(i)).ok())
        };
        let blob = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("blob"))
            .and_then(|(_, v)| v.as_bytes())
            .and_then(|b| Hash::from_bytes(b).ok());
        // All four are required, and an out-of-range mode is malformed rather
        // than clamped: a wrong mode on restore is a silently broken script.
        let (Some(size), Some(mode), Some(mtime), Some(blob)) = (uint("size"), uint("mode"), uint("mtime"), blob)
        else {
            return Err(FileError::Malformed("lacks size, mode, mtime or blob".into()));
        };
        let mode = u32::try_from(mode)
            .ok()
            .filter(|m| *m <= MAX_MODE)
            .ok_or_else(|| FileError::Malformed(format!("mode {mode:o} is out of range")))?;
        Ok(WorkFile { size, mode, mtime, blob })
    }

    fn as_asset_entry(&self) -> AssetEntry {
        AssetEntry { size: self.size, blob: self.blob }
    }
}

// ---------------------------------------------------------------------------
// Where the files are kept
// ---------------------------------------------------------------------------

/// The store a workspace is kept in. A trait so every rule below is gated
/// natively; the player implements it over [`crate::writer_handle::WriterHandle`].
pub trait WorkStore {
    /// `Err` when this store cannot hold a workspace at all (the Worker arm).
    /// Checked before anything else, so "cannot look" never reads as "empty".
    fn usable(&self) -> Result<(), String>;
    fn get(&self, path: &str) -> Option<Entity>;
    /// Full tree paths bound under `prefix`.
    fn list(&self, prefix: &str) -> Vec<String>;
    fn put(&self, path: &str, entity: Entity);
    fn remove(&self, path: &str);
    fn content_put(&self, entity: Entity);
    fn content_get(&self, hash: &Hash) -> Option<Entity>;
}

impl WorkStore for crate::writer_handle::WriterHandle {
    fn usable(&self) -> Result<(), String> {
        match self {
            crate::writer_handle::WriterHandle::Direct(_) => Ok(()),
            #[cfg(target_arch = "wasm32")]
            crate::writer_handle::WriterHandle::Worker { .. } => {
                Err("this storage mode cannot hold working files (worker mode has no content store)".into())
            }
        }
    }
    fn get(&self, path: &str) -> Option<Entity> {
        match self {
            crate::writer_handle::WriterHandle::Direct(shared) => shared.tree.get(path),
            #[cfg(target_arch = "wasm32")]
            crate::writer_handle::WriterHandle::Worker { .. } => None,
        }
    }
    fn list(&self, prefix: &str) -> Vec<String> {
        match self {
            crate::writer_handle::WriterHandle::Direct(shared) => {
                shared.tree.list(prefix).into_iter().map(|e| e.path).collect()
            }
            #[cfg(target_arch = "wasm32")]
            crate::writer_handle::WriterHandle::Worker { .. } => Vec::new(),
        }
    }
    fn put(&self, path: &str, entity: Entity) {
        crate::writer_handle::WriterHandle::put(self, path.to_string(), entity)
    }
    fn remove(&self, path: &str) {
        crate::writer_handle::WriterHandle::remove(self, path.to_string())
    }
    fn content_put(&self, entity: Entity) {
        crate::writer_handle::WriterHandle::content_put(self, entity)
    }
    fn content_get(&self, hash: &Hash) -> Option<Entity> {
        crate::writer_handle::WriterHandle::content_get(self, hash)
    }
}

/// Where a running app's workspace is kept — built by the Apps window, handed
/// to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSource {
    pub app_id: String,
    /// The tree prefix, ending in `/`. See [`crate::app_paths::app_workspace_prefix`].
    pub prefix: String,
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why the host did not do what was asked. Each names whose problem it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The app did not declare [`MANIFEST_KEY`]. Dropped without a reply.
    NotDeclared,
    /// The message is missing a field or has one of the wrong shape. The app's defect.
    BadRequest(String),
    /// A path [`assets::valid_key`] refuses. The app's defect.
    BadPath,
    /// A mode over [`MAX_MODE`]. The app's defect.
    BadMode,
    /// Over a size or count limit of this host.
    TooLarge(String),
    /// No file at that path. Either side, which is why it is its own code.
    NotFound,
    /// This store cannot hold a workspace, or the bytes are not held.
    Unavailable(String),
    /// A stored file does not decode or reassemble. Ours.
    Unreadable(String),
    /// The file is not the version the save expected: another window of this
    /// app changed, created or removed it since this one read it.
    Conflict,
}

impl Refusal {
    /// Stable wire code for `reason`. An app branches on this, never on prose.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NotDeclared => "not-declared",
            Refusal::BadRequest(_) => "bad-request",
            Refusal::BadPath => "bad-path",
            Refusal::BadMode => "bad-mode",
            Refusal::TooLarge(_) => "too-large",
            Refusal::NotFound => "not-found",
            Refusal::Unavailable(_) => "unavailable",
            Refusal::Unreadable(_) => "unreadable",
            Refusal::Conflict => "conflict",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotDeclared => write!(f, "the app does not declare a workspace"),
            Refusal::BadRequest(why) => write!(f, "malformed request: {why}"),
            Refusal::BadPath => write!(f, "not a valid workspace path"),
            Refusal::BadMode => write!(f, "mode is out of range"),
            Refusal::TooLarge(why) => write!(f, "over this host's limit: {why}"),
            Refusal::NotFound => write!(f, "no such file in the workspace"),
            Refusal::Unavailable(why) => write!(f, "the workspace is unavailable: {why}"),
            Refusal::Unreadable(why) => write!(f, "a stored file is unreadable: {why}"),
            Refusal::Conflict => write!(f, "another window changed this file since it was read"),
        }
    }
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// A workspace path that passed validation — the only thing a store is ever
/// addressed with.
fn tree_path(prefix: &str, path: &str) -> Result<String, Refusal> {
    if !assets::valid_key(path) {
        return Err(Refusal::BadPath);
    }
    Ok(format!("{prefix}{path}"))
}

/// Every file in the workspace, sorted by path, plus the paths whose entity
/// could not be read.
///
/// **One unreadable file does not hide the rest**, unlike an asset index: a
/// workspace is the person's work, and refusing to list twenty good files over
/// one bad one would lose all twenty for this session. The bad paths are
/// reported so they are not mistaken for absent ones.
pub fn list(store: &dyn WorkStore, prefix: &str) -> Result<(Vec<(String, WorkFile)>, Vec<String>), Refusal> {
    store.usable().map_err(Refusal::Unavailable)?;
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    let mut paths = store.list(prefix);
    paths.sort();
    for full in paths {
        let Some(rel) = full.strip_prefix(prefix) else { continue };
        if !assets::valid_key(rel) {
            unreadable.push(rel.to_string());
            continue;
        }
        match store.get(&full).map(|e| WorkFile::from_entity(&e)) {
            Some(Ok(f)) => files.push((rel.to_string(), f)),
            // Listed and then gone: a concurrent remove. Not a fault.
            None => {}
            Some(Err(_)) => unreadable.push(rel.to_string()),
        }
    }
    Ok((files, unreadable))
}

/// One file's bytes, checked against the length the entity claims.
pub fn read(store: &dyn WorkStore, prefix: &str, path: &str) -> Result<Vec<u8>, Refusal> {
    store.usable().map_err(Refusal::Unavailable)?;
    let full = tree_path(prefix, path)?;
    let entity = store.get(&full).ok_or(Refusal::NotFound)?;
    let file = WorkFile::from_entity(&entity).map_err(|e| Refusal::Unreadable(e.to_string()))?;
    assets::resolve_entry(&file.as_asset_entry(), |h| store.content_get(h)).map_err(|r| match r {
        assets::Refusal::Unavailable(why) => Refusal::Unavailable(why),
        other => Refusal::Unreadable(other.to_string()),
    })
}

/// One file in a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutFile {
    pub path: String,
    pub mode: u32,
    pub mtime: u64,
    pub bytes: Vec<u8>,
}

/// What a save did. `failed` is per path, so one bad file is reported without
/// abandoning the rest of the batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveReport {
    pub saved: usize,
    pub removed: usize,
    pub bytes: u64,
    pub failed: Vec<(String, Refusal)>,
    /// The version each saved path now holds, so the app can expect it next time.
    pub versions: Vec<(String, String)>,
}

/// Per path, the version a save expects the workspace to hold (`None`: no file).
pub type Expect = std::collections::BTreeMap<String, Option<String>>;

/// A file's version: its blob hash, hex. Two saves of the same bytes are one
/// version, so a window that wrote what another already wrote is not a conflict.
pub fn version_of(file: &WorkFile) -> String {
    file.blob.to_hex()
}

/// The version at `full` now, or `None` for no (readable) file.
fn current_version(store: &dyn WorkStore, full: &str) -> Option<String> {
    store.get(full).and_then(|e| WorkFile::from_entity(&e).ok()).map(|f| version_of(&f))
}

/// Check a whole save before writing any of it. **Batch limits refuse the
/// batch**; a bad path or mode fails only that path (reported by [`save`]).
pub fn admit_save(put: &[PutFile], remove: &[String]) -> Result<(), Refusal> {
    let count = put.len() + remove.len();
    if count > MAX_SAVE_PATHS {
        return Err(Refusal::TooLarge(format!("{count} paths in one save, limit {MAX_SAVE_PATHS}")));
    }
    let total: u64 = put.iter().map(|p| p.bytes.len() as u64).sum();
    if total > MAX_SAVE_BYTES {
        return Err(Refusal::TooLarge(format!("{total} bytes in one save, limit {MAX_SAVE_BYTES}")));
    }
    // A path both written and removed is ambiguous about which the app meant.
    let written: BTreeSet<&str> = put.iter().map(|p| p.path.as_str()).collect();
    if let Some(both) = remove.iter().find(|r| written.contains(r.as_str())) {
        return Err(Refusal::BadRequest(format!("{both:?} is both saved and removed")));
    }
    if written.len() != put.len() {
        return Err(Refusal::BadRequest("a path is saved twice".into()));
    }
    Ok(())
}

/// Write `put`, remove `remove`. Removes only the paths named (module doc).
pub fn save(store: &dyn WorkStore, prefix: &str, put: &[PutFile], remove: &[String]) -> Result<SaveReport, Refusal> {
    save_expecting(store, prefix, put, remove, &Expect::new())
}

/// [`save`], refusing any path named in `expect` whose current version is not
/// the expected one (module doc, *Two windows, one workspace*).
///
/// - a **put** conflicts when the file changed, or when it appeared where the
///   app expected none. A put where the app expected a file that another window
///   has since removed is **not** a conflict: writing it back loses nothing.
/// - a **remove** conflicts when the file changed. Removing a file that is
///   already gone is not.
pub fn save_expecting(
    store: &dyn WorkStore,
    prefix: &str,
    put: &[PutFile],
    remove: &[String],
    expect: &Expect,
) -> Result<SaveReport, Refusal> {
    store.usable().map_err(Refusal::Unavailable)?;
    admit_save(put, remove)?;
    let mut report = SaveReport::default();
    for p in put {
        if let (Some(expected), Ok(full)) = (expect.get(&p.path), tree_path(prefix, &p.path)) {
            let now = current_version(store, &full);
            let conflict = match (expected, &now) {
                (Some(want), Some(have)) => want != have,
                (None, Some(_)) => true,
                (_, None) => false,
            };
            if conflict {
                report.failed.push((p.path.clone(), Refusal::Conflict));
                continue;
            }
        }
        match save_one(store, prefix, p) {
            Ok(version) => {
                report.saved += 1;
                report.bytes += p.bytes.len() as u64;
                report.versions.push((p.path.clone(), version));
            }
            Err(r) => report.failed.push((p.path.clone(), r)),
        }
    }
    for path in remove {
        match tree_path(prefix, path) {
            Ok(full) => {
                if let Some(Some(want)) = expect.get(path) {
                    if current_version(store, &full).is_some_and(|have| &have != want) {
                        report.failed.push((path.clone(), Refusal::Conflict));
                        continue;
                    }
                }
                if store.get(&full).is_some() {
                    store.remove(&full);
                    report.removed += 1;
                }
            }
            Err(r) => report.failed.push((path.clone(), r)),
        }
    }
    Ok(report)
}

fn save_one(store: &dyn WorkStore, prefix: &str, p: &PutFile) -> Result<String, Refusal> {
    let full = tree_path(prefix, &p.path)?;
    if p.mode > MAX_MODE {
        return Err(Refusal::BadMode);
    }
    if p.bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(Refusal::TooLarge(format!("{} bytes, limit {MAX_FILE_BYTES}", p.bytes.len())));
    }
    let scratch: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let (entry, closure) = assets::stage_bytes(&p.bytes, &scratch).map_err(Refusal::Unavailable)?;
    // Content before the binding that names it, so a reader never holds a file
    // whose bytes have not landed.
    for e in closure {
        store.content_put(e);
    }
    let file = WorkFile { size: entry.size, mode: p.mode, mtime: p.mtime, blob: entry.blob };
    store.put(&full, file.to_entity());
    Ok(version_of(&file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemStore {
        tree: RefCell<BTreeMap<String, Entity>>,
        content: RefCell<BTreeMap<Hash, Entity>>,
        unusable: Option<String>,
    }

    impl WorkStore for MemStore {
        fn usable(&self) -> Result<(), String> {
            self.unusable.clone().map_or(Ok(()), Err)
        }
        fn get(&self, path: &str) -> Option<Entity> {
            self.tree.borrow().get(path).cloned()
        }
        fn list(&self, prefix: &str) -> Vec<String> {
            self.tree.borrow().keys().filter(|k| k.starts_with(prefix)).cloned().collect()
        }
        fn put(&self, path: &str, entity: Entity) {
            self.tree.borrow_mut().insert(path.to_string(), entity);
        }
        fn remove(&self, path: &str) {
            self.tree.borrow_mut().remove(path);
        }
        fn content_put(&self, entity: Entity) {
            self.content.borrow_mut().insert(entity.content_hash, entity);
        }
        fn content_get(&self, hash: &Hash) -> Option<Entity> {
            self.content.borrow().get(hash).cloned()
        }
    }

    const P: &str = "/me/app/entity-browser/apps/apps/work/alpine/";

    fn file(path: &str, mode: u32, bytes: &[u8]) -> PutFile {
        PutFile { path: path.into(), mode, mtime: 1_789_000_000, bytes: bytes.to_vec() }
    }

    #[test]
    fn a_saved_file_lists_and_reads_back_with_its_mode_and_mtime() {
        let s = MemStore::default();
        let big = vec![7u8; 200_000]; // several chunks
        let r = save(&s, P, &[file("notes.txt", 0o644, b"hello"), file("bin/run.sh", 0o755, &big)], &[]).unwrap();
        assert_eq!((r.saved, r.removed, r.bytes, r.failed.len()), (2, 0, 200_005, 0));

        let (files, unreadable) = list(&s, P).unwrap();
        assert!(unreadable.is_empty());
        let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["bin/run.sh", "notes.txt"]);
        assert_eq!(files[0].1.mode, 0o755, "an executable must come back executable");
        assert_eq!(files[1].1.mtime, 1_789_000_000);

        assert_eq!(read(&s, P, "notes.txt").unwrap(), b"hello");
        assert_eq!(read(&s, P, "bin/run.sh").unwrap(), big);
    }

    #[test]
    fn a_file_lives_at_the_prefix_plus_its_path_so_it_is_visible_in_the_tree() {
        let s = MemStore::default();
        save(&s, P, &[file("project/main.c", 0o644, b"int main;")], &[]).unwrap();
        let e = s.get(&format!("{P}project/main.c")).expect("bound at prefix + path");
        assert_eq!(e.entity_type, FILE_TYPE);
    }

    #[test]
    fn a_save_removes_exactly_the_paths_it_names_and_nothing_else() {
        let s = MemStore::default();
        save(&s, P, &[file("a", 0o644, b"a"), file("b", 0o644, b"b"), file("c", 0o644, b"c")], &[]).unwrap();
        let r = save(&s, P, &[], &["b".into(), "never-existed".into()]).unwrap();
        assert_eq!(r.removed, 1, "removing a path that is not there is not a removal");
        let (files, _) = list(&s, P).unwrap();
        let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["a", "c"], "a save must never delete a file it did not name");
    }

    #[test]
    fn bad_paths_fail_that_path_and_the_rest_of_the_batch_lands() {
        let s = MemStore::default();
        let r = save(&s, P, &[file("../escape", 0o644, b"x"), file("ok", 0o644, b"y"), file("/abs", 0o644, b"z")], &["a//b".into()]).unwrap();
        assert_eq!(r.saved, 1);
        let codes: Vec<_> = r.failed.iter().map(|(p, why)| (p.as_str(), why.code())).collect();
        assert_eq!(codes, [("../escape", "bad-path"), ("/abs", "bad-path"), ("a//b", "bad-path")]);
        assert!(s.tree.borrow().keys().all(|k| k.starts_with(P)), "nothing may be written outside the prefix");
    }

    #[test]
    fn an_out_of_range_mode_is_refused_not_clamped() {
        let s = MemStore::default();
        let r = save(&s, P, &[file("f", 0o100644, b"x")], &[]).unwrap();
        assert_eq!(r.failed[0].1, Refusal::BadMode);
        assert!(list(&s, P).unwrap().0.is_empty());
    }

    #[test]
    fn batch_limits_refuse_the_whole_save_before_anything_is_written() {
        let s = MemStore::default();
        let huge = vec![0u8; (MAX_SAVE_BYTES / 2 + 1) as usize];
        let e = save(&s, P, &[file("a", 0o644, &huge), file("b", 0o644, &huge)], &[]).unwrap_err();
        assert_eq!(e.code(), "too-large");
        assert!(s.tree.borrow().is_empty(), "a refused batch writes nothing");

        let e = save(&s, P, &[file("a", 0o644, b"x")], &["a".into()]).unwrap_err();
        assert_eq!(e.code(), "bad-request");
        let e = save(&s, P, &[file("a", 0o644, b"x"), file("a", 0o644, b"y")], &[]).unwrap_err();
        assert_eq!(e.code(), "bad-request");
    }

    #[test]
    fn a_store_that_cannot_hold_a_workspace_is_unavailable_never_empty() {
        let s = MemStore { unusable: Some("worker arm".into()), ..Default::default() };
        assert_eq!(list(&s, P).unwrap_err().code(), "unavailable");
        assert_eq!(read(&s, P, "a").unwrap_err().code(), "unavailable");
        assert_eq!(save(&s, P, &[], &[]).unwrap_err().code(), "unavailable");
    }

    #[test]
    fn one_unreadable_file_is_reported_and_does_not_hide_the_others() {
        let s = MemStore::default();
        save(&s, P, &[file("good", 0o644, b"ok")], &[]).unwrap();
        s.put(&format!("{P}bad"), Entity::new("app/something-else", to_ecf(&Value::Map(vec![]))).unwrap());
        let (files, unreadable) = list(&s, P).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(unreadable, ["bad"]);
    }

    #[test]
    fn a_missing_file_and_missing_bytes_are_different_answers() {
        let s = MemStore::default();
        assert_eq!(read(&s, P, "nope").unwrap_err().code(), "not-found");
        save(&s, P, &[file("f", 0o644, b"bytes")], &[]).unwrap();
        s.content.borrow_mut().clear();
        assert_eq!(read(&s, P, "f").unwrap_err().code(), "unavailable");
    }

    #[test]
    fn the_file_entity_round_trips_and_rejects_what_it_cannot_trust() {
        let f = WorkFile { size: 5, mode: 0o755, mtime: 42, blob: Hash::compute("test/blob", b"x") };
        assert_eq!(WorkFile::from_entity(&f.to_entity()).unwrap(), f);
        let wrong = Entity::new("app/app-save", to_ecf(&Value::Map(vec![]))).unwrap();
        assert!(matches!(WorkFile::from_entity(&wrong), Err(FileError::WrongType(_))));
        let no_mode = Entity::new(
            FILE_TYPE,
            to_ecf(&Value::Map(vec![(text("size"), integer(1)), (text("blob"), ecf_bytes(f.blob.to_bytes()))])),
        )
        .unwrap();
        assert!(matches!(WorkFile::from_entity(&no_mode), Err(FileError::Malformed(_))));
    }

    /// The extension's names stay prefixed until entity-apps rules, like
    /// `x-files` and `x-assets`.
    #[test]
    fn the_extension_names_are_x_prefixed() {
        for n in [MANIFEST_KEY, INIT_KEY, MSG_LIST, MSG_LISTING, MSG_GET, MSG_FILE, MSG_SAVE, MSG_SAVED] {
            assert!(n.starts_with("x-"), "{n} is not x-prefixed");
        }
    }

    // ── two windows, one workspace (BACKLOG B-10) ──────────────────────────────

    fn version_at(s: &MemStore, path: &str) -> Option<String> {
        list(s, P).unwrap().0.into_iter().find(|(p, _)| p == path).map(|(_, f)| version_of(&f))
    }

    fn expect(rows: &[(&str, Option<&str>)]) -> Expect {
        rows.iter().map(|(p, v)| (p.to_string(), v.map(str::to_string))).collect()
    }

    #[test]
    fn a_file_edited_in_two_windows_keeps_the_first_save_and_refuses_the_stale_one() {
        let s = MemStore::default();
        save(&s, P, &[file("notes.txt", 0o644, b"original")], &[]).unwrap();
        let read_by_both = version_at(&s, "notes.txt").unwrap();
        // Window B saves its edit, against what it read.
        let b = save_expecting(&s, P, &[file("notes.txt", 0o644, b"edited in B")], &[], &expect(&[("notes.txt", Some(&read_by_both))])).unwrap();
        assert!(b.failed.is_empty(), "{b:?}");
        // Window A, still holding the original, saves its own edit.
        let a = save_expecting(&s, P, &[file("notes.txt", 0o644, b"edited in A")], &[], &expect(&[("notes.txt", Some(&read_by_both))])).unwrap();
        assert_eq!(a.failed, vec![("notes.txt".to_string(), Refusal::Conflict)], "the stale save must be refused");
        assert_eq!(read(&s, P, "notes.txt").unwrap(), b"edited in B", "and B's edit is still there");
        assert_eq!(a.saved, 0);
    }

    #[test]
    fn a_new_file_where_another_window_already_made_one_is_a_conflict() {
        let s = MemStore::default();
        save(&s, P, &[file("new.txt", 0o644, b"from B")], &[]).unwrap();
        let a = save_expecting(&s, P, &[file("new.txt", 0o644, b"from A")], &[], &expect(&[("new.txt", None)])).unwrap();
        assert_eq!(a.failed, vec![("new.txt".to_string(), Refusal::Conflict)]);
        assert_eq!(read(&s, P, "new.txt").unwrap(), b"from B");
    }

    #[test]
    fn removing_a_file_another_window_has_since_updated_is_refused() {
        let s = MemStore::default();
        save(&s, P, &[file("keep.txt", 0o644, b"v1")], &[]).unwrap();
        let v1 = version_at(&s, "keep.txt").unwrap();
        save(&s, P, &[file("keep.txt", 0o644, b"v2, saved by A")], &[]).unwrap();
        let b = save_expecting(&s, P, &[], &["keep.txt".into()], &expect(&[("keep.txt", Some(&v1))])).unwrap();
        assert_eq!(b.failed, vec![("keep.txt".to_string(), Refusal::Conflict)]);
        assert_eq!(b.removed, 0);
        assert_eq!(read(&s, P, "keep.txt").unwrap(), b"v2, saved by A", "the newer copy survives B's delete");
        // Removing what B actually read still works when nothing moved.
        let v2 = version_at(&s, "keep.txt").unwrap();
        let ok = save_expecting(&s, P, &[], &["keep.txt".into()], &expect(&[("keep.txt", Some(&v2))])).unwrap();
        assert_eq!((ok.removed, ok.failed.len()), (1, 0));
    }

    #[test]
    fn writing_back_a_file_another_window_removed_loses_nothing_and_is_allowed() {
        let s = MemStore::default();
        save(&s, P, &[file("gone.txt", 0o644, b"v1")], &[]).unwrap();
        let v1 = version_at(&s, "gone.txt").unwrap();
        save(&s, P, &[], &["gone.txt".into()]).unwrap();
        let a = save_expecting(&s, P, &[file("gone.txt", 0o644, b"v1 edited")], &[], &expect(&[("gone.txt", Some(&v1))])).unwrap();
        assert!(a.failed.is_empty(), "{a:?}");
        assert_eq!(read(&s, P, "gone.txt").unwrap(), b"v1 edited");
    }

    #[test]
    fn a_save_that_expects_nothing_is_unchecked_and_versions_are_reported() {
        let s = MemStore::default();
        save(&s, P, &[file("f", 0o644, b"one")], &[]).unwrap();
        let r = save(&s, P, &[file("f", 0o644, b"two")], &[]).unwrap();
        assert!(r.failed.is_empty(), "an app that sends no `expect` saves as before");
        assert_eq!(r.versions, vec![("f".to_string(), version_at(&s, "f").unwrap())], "each save reports the version it wrote");
        // The same bytes are the same version: a window writing what another wrote is not a conflict.
        let same = version_at(&s, "f").unwrap();
        let again = save_expecting(&s, P, &[file("f", 0o600, b"two")], &[], &expect(&[("f", Some(&same))])).unwrap();
        assert!(again.failed.is_empty());
    }
}
