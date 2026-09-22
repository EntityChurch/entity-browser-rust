//! Save-state management — back it up, restore it, hand it to another peer.
//!
//! An entity app persists opaque save state through the host loop
//! (`dom::games` → [`crate::app_paths::app_save_path`]). That was the whole
//! story: one live save per app, overwritten in place, reachable only by
//! running the app. This module is the other half — the save as something a
//! person **owns**: they can snapshot it before a risky move, roll back to a
//! snapshot, and carry it to another install.
//!
//! Three rules the shapes here exist to enforce:
//!
//! **A save outlives its catalog.** [`list_saves`] enumerates the *save*
//! prefix, not the catalog — so a save for an app that is no longer published
//! still lists, still backs up, and can still be sent somewhere it does run.
//! Joining against the catalog first would make an origin going away look like
//! the user's data going away.
//!
//! **A backup is a distinct entity, never a second live save.** It lives under
//! `…/backups/{id}/{stamp}` and nothing reads it but this module; the app only
//! ever sees `…/state/{id}`. Restore *copies* backwards. That keeps "which save
//! is the app playing" a question with one answer.
//!
//! **A save crossing to another peer carries what it is.** [`SaveBundle`] wraps
//! the opaque state with the set and id it belongs to, so the receiving side
//! files it against the right app instead of trusting a filename. The bytes
//! ride the ordinary file-offer path ([`crate::file_offer`]) — pull, never
//! push: a stranger does not write into your tree.

use entity_entity::Entity;
use entity_hash::Hash;

use super::format::{AppSave, APP_SAVE_TYPE};
use crate::app_paths::{self, APP_ID};
use crate::peers::Peers;

/// Entity type for a save packaged for another peer. Distinct from
/// [`APP_SAVE_TYPE`] on purpose: a bundle is a *transfer* envelope, and a
/// consumer that confuses the two would write the envelope where the app
/// expects bare state.
pub const SAVE_BUNDLE_TYPE: &str = "app/app-save-bundle";

/// The file-name suffix a bundle is offered under. Cosmetic — the receiver
/// identifies a bundle by decoding it, never by its name — but it is what the
/// File Transfer window shows, so it should say what the thing is.
pub const BUNDLE_SUFFIX: &str = ".entitysave";

/// One app's live save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRow {
    pub set: String,
    pub id: String,
    /// Size of the opaque state, in bytes of its serialized form.
    pub bytes: usize,
    /// Content hash of the save entity — the identity a backup dedups against.
    pub hash: Hash,
}

/// One snapshot of an app's save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRow {
    /// Epoch milliseconds, decoded from the path segment.
    pub stamp_ms: u64,
    pub bytes: usize,
    pub hash: Hash,
}

/// Every live save under one app-set, in listing (id) order.
///
/// Reads the save prefix directly — see the module note on why this is not
/// joined against the catalog. A row whose entity is missing or is not a save
/// is skipped rather than surfaced as an empty save.
pub fn list_saves(peers: &Peers, peer_id: &str, set: &str) -> Vec<SaveRow> {
    let prefix = app_paths::app_saves_prefix(APP_ID, peer_id, set);
    let mut out = Vec::new();
    for entry in peers.tree_listing(peer_id, &prefix) {
        let Some(id) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        // The prefix is one level deep; anything below it is not a save.
        if id.is_empty() || id.contains('/') {
            continue;
        }
        let Some(ent) = peers.get_entity(peer_id, &entry.path) else {
            continue;
        };
        if ent.entity_type != APP_SAVE_TYPE {
            continue;
        }
        out.push(SaveRow {
            set: set.to_string(),
            id: id.to_string(),
            bytes: AppSave::from_entity(&ent).state.len(),
            hash: ent.content_hash,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Every live save across every set, in [`super::paths::APP_SETS`] order.
pub fn list_all_saves(peers: &Peers, peer_id: &str) -> Vec<SaveRow> {
    super::paths::APP_SETS
        .iter()
        .flat_map(|set| list_saves(peers, peer_id, set))
        .collect()
}

/// One app's backups, **newest first**.
///
/// A path segment that is not a number is skipped: the stamp is the ordering
/// key, and a row that cannot be placed in time is worse than absent — it would
/// sort somewhere arbitrary and read as a backup from 1970.
pub fn list_backups(peers: &Peers, peer_id: &str, set: &str, id: &str) -> Vec<BackupRow> {
    let prefix = app_paths::app_backups_for(APP_ID, peer_id, set, id);
    let mut out = Vec::new();
    for entry in peers.tree_listing(peer_id, &prefix) {
        let Some(stamp) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(stamp_ms) = stamp.parse::<u64>() else {
            continue;
        };
        let Some(ent) = peers.get_entity(peer_id, &entry.path) else {
            continue;
        };
        if ent.entity_type != APP_SAVE_TYPE {
            continue;
        }
        out.push(BackupRow {
            stamp_ms,
            bytes: AppSave::from_entity(&ent).state.len(),
            hash: ent.content_hash,
        });
    }
    out.sort_by(|a, b| b.stamp_ms.cmp(&a.stamp_ms));
    out
}

/// Whether this app's live save is already byte-identical to its newest backup.
///
/// Backing up an unchanged save twice would bind a second presence entity for
/// content the store already holds, and give the user two rows that differ only
/// by a timestamp. Content addressing makes the *blob* free; the binding is
/// not, and nothing in the app reclaims one (buildout item 21).
pub fn already_backed_up(peers: &Peers, peer_id: &str, set: &str, id: &str) -> bool {
    let Some(live) = peers.get_entity(peer_id, &app_paths::app_save_path(APP_ID, peer_id, set, id))
    else {
        return false;
    };
    list_backups(peers, peer_id, set, id)
        .first()
        .map(|b| b.hash == live.content_hash)
        .unwrap_or(false)
}

/// A save packaged for another peer: the opaque state plus enough to file it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveBundle {
    pub set: String,
    pub id: String,
    /// The app's display name at the sending end — shown while choosing what to
    /// import, and the only human-readable thing in the envelope. Advisory: the
    /// receiver files by `set`/`id`, never by this.
    pub app_name: String,
    pub state: String,
    /// When the sender packaged it, epoch milliseconds. `0` = not stated.
    pub saved_at_ms: u64,
}

impl SaveBundle {
    /// The name this bundle is offered under. Includes the set, because two
    /// apps in different sets may share an id and a person choosing between two
    /// offered files should be able to tell them apart.
    pub fn file_name(&self) -> String {
        format!("{}-{}{}", self.set, self.id, BUNDLE_SUFFIX)
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
            (
                entity_ecf::Value::Text("set".into()),
                entity_ecf::text(&self.set),
            ),
            (
                entity_ecf::Value::Text("id".into()),
                entity_ecf::text(&self.id),
            ),
            (
                entity_ecf::Value::Text("app_name".into()),
                entity_ecf::text(&self.app_name),
            ),
            (
                entity_ecf::Value::Text("state".into()),
                entity_ecf::text(&self.state),
            ),
            (
                entity_ecf::Value::Text("saved_at_ms".into()),
                entity_ecf::uinteger(self.saved_at_ms),
            ),
        ]));
        Entity::new(SAVE_BUNDLE_TYPE, data).unwrap()
    }

    /// The bytes to hand to [`crate::file_offer::offer_file`].
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_entity().data
    }

    /// Decode a bundle from the bytes a pull returned.
    ///
    /// Returns `None` for anything that is not a bundle — including an ordinary
    /// file someone happened to offer. The import surface pulls from a list the
    /// user chose from, so it WILL meet non-bundles; treating a decode failure
    /// as an error rather than as "that is not a save" would make an unrelated
    /// offer look like a corrupt one.
    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        let value: ciborium::Value = ciborium::from_reader(raw).ok()?;
        let map = value.as_map()?;
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("set") => out.set = v.as_text()?.to_string(),
                Some("id") => out.id = v.as_text()?.to_string(),
                Some("app_name") => out.app_name = v.as_text().unwrap_or("").to_string(),
                Some("state") => out.state = v.as_text()?.to_string(),
                Some("saved_at_ms") => {
                    out.saved_at_ms = v
                        .as_integer()
                        .and_then(|i| u64::try_from(i).ok())
                        .unwrap_or(0)
                }
                _ => {}
            }
        }
        // A bundle that names no app cannot be filed, which is the one thing a
        // bundle exists to make possible.
        (!out.set.is_empty() && !out.id.is_empty()).then_some(out)
    }
}

// ---------------------------------------------------------------------------
// The save operations — ONE implementation, two surfaces.
//
// The Apps window's Saves panel and the Files window both back up, restore,
// drop and import saves. They used to be methods on the Apps window only; a
// second surface would have had to copy them, and the copy of "back up what an
// import replaces" is exactly the line a copy forgets. Each returns what it did
// as an enum, so each surface words it its own way without re-deciding it.
// ---------------------------------------------------------------------------

/// What [`backup_live`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupOutcome {
    BackedUp,
    /// The newest backup already holds these exact bytes — a second row that
    /// differs only by a clock would be noise and one more binding nothing
    /// reclaims.
    Unchanged,
    /// There is no live save to back up.
    NoSave,
}

/// Snapshot one app's live save under `now_ms`.
pub fn backup_live(
    peers: &Peers,
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    set: &str,
    id: &str,
    now_ms: u64,
) -> BackupOutcome {
    let live = app_paths::app_save_path(APP_ID, peer_id, set, id);
    let Some(ent) = peers.get_entity(peer_id, &live) else {
        return BackupOutcome::NoSave;
    };
    if already_backed_up(peers, peer_id, set, id) {
        return BackupOutcome::Unchanged;
    }
    writer.put(app_paths::app_backup_path(APP_ID, peer_id, set, id, now_ms), ent);
    BackupOutcome::BackedUp
}

/// Copy a backup back over the live save. The backup entity is written
/// unchanged, so the two share one content blob. `false` = no such backup.
pub fn restore_backup(
    peers: &Peers,
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    set: &str,
    id: &str,
    stamp: u64,
) -> bool {
    let from = app_paths::app_backup_path(APP_ID, peer_id, set, id, stamp);
    let Some(ent) = peers.get_entity(peer_id, &from) else {
        return false;
    };
    writer.put(app_paths::app_save_path(APP_ID, peer_id, set, id), ent);
    true
}

/// Forget a backup, and reclaim its bytes if nothing else binds them.
/// Reclaim is binding-safe, so a live save restored from this very backup
/// keeps the blob alive.
pub fn drop_backup(
    peers: &Peers,
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    set: &str,
    id: &str,
    stamp: u64,
) {
    let path = app_paths::app_backup_path(APP_ID, peer_id, set, id, stamp);
    let hash = peers.get_entity(peer_id, &path).map(|e| e.content_hash);
    writer.remove(path);
    if let Some(h) = hash {
        writer.content_remove(h);
    }
}

/// What [`import_bundle`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// There was no save for that app; this is now it.
    Imported,
    /// A save was replaced — and backed up first, so it is in the backup list.
    ImportedReplacing,
}

/// File a bundle as this peer's save for the app it names.
///
/// **It backs up whatever it replaces first.** Importing is the one save
/// operation that destroys a save without naming it — the person is thinking
/// about the incoming one — so the outgoing one is snapshotted on the way past.
/// Where the bundle came from (a peer's offer, a file off a USB stick) is not
/// this function's business; the bundle carries what it is.
pub fn import_bundle(
    peers: &Peers,
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    bundle: &SaveBundle,
    now_ms: u64,
) -> ImportOutcome {
    let live = app_paths::app_save_path(APP_ID, peer_id, &bundle.set, &bundle.id);
    let replaced = peers.get_entity(peer_id, &live).is_some();
    if replaced {
        backup_live(peers, writer, peer_id, &bundle.set, &bundle.id, now_ms);
    }
    writer.put(live, AppSave::new(&bundle.state).to_entity());
    if replaced {
        ImportOutcome::ImportedReplacing
    } else {
        ImportOutcome::Imported
    }
}

/// Package one app's live save as a bundle, ready to leave the device. `None` =
/// no live save. `app_name` is advisory (see [`SaveBundle::app_name`]).
pub fn bundle_live(
    peers: &Peers,
    peer_id: &str,
    set: &str,
    id: &str,
    app_name: &str,
    now_ms: u64,
) -> Option<SaveBundle> {
    let ent = peers.get_entity(peer_id, &app_paths::app_save_path(APP_ID, peer_id, set, id))?;
    if ent.entity_type != APP_SAVE_TYPE {
        return None;
    }
    Some(SaveBundle {
        set: set.to_string(),
        id: id.to_string(),
        app_name: app_name.to_string(),
        state: AppSave::from_entity(&ent).state,
        saved_at_ms: now_ms,
    })
}

/// Package one backup as a bundle. `None` = no such backup. The bundle's
/// `saved_at_ms` is the backup's own stamp — when the state was captured, which
/// is what a person importing it later wants to know.
pub fn bundle_backup(
    peers: &Peers,
    peer_id: &str,
    set: &str,
    id: &str,
    app_name: &str,
    stamp_ms: u64,
) -> Option<SaveBundle> {
    let ent = peers.get_entity(peer_id, &app_paths::app_backup_path(APP_ID, peer_id, set, id, stamp_ms))?;
    if ent.entity_type != APP_SAVE_TYPE {
        return None;
    }
    Some(SaveBundle {
        set: set.to_string(),
        id: id.to_string(),
        app_name: app_name.to_string(),
        state: AppSave::from_entity(&ent).state,
        saved_at_ms: stamp_ms,
    })
}

/// What reading a picked file as a save did — the file-picker half of the USB
/// flow, shared by the Files window and the Apps window's Saves panel so both
/// word the same three facts the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileImport {
    /// The file is not a save at all. Carries the file's own name: an ordinary
    /// file picked by mistake is the common case, and "corrupt save" would send
    /// the person looking for a problem that is not there.
    NotASave { file_name: String },
    Imported { app: String },
    ImportedReplacing { app: String },
}

impl FileImport {
    /// The sentence for a person.
    pub fn message(&self) -> String {
        match self {
            FileImport::NotASave { file_name } => crate::i18n::t("files.not_a_save", &[("name", file_name)]),
            FileImport::Imported { app } => crate::i18n::t("saves.imported", &[("app", app)]),
            FileImport::ImportedReplacing { app } => crate::i18n::t("saves.imported_replacing", &[("app", app)]),
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, FileImport::NotASave { .. })
    }
}

/// Import a picked file as a save ([`import_bundle`], which backs up first).
pub fn import_file(
    peers: &Peers,
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    file_name: &str,
    bytes: &[u8],
    now_ms: u64,
) -> FileImport {
    let Some(bundle) = SaveBundle::from_bytes(bytes) else {
        return FileImport::NotASave { file_name: file_name.to_string() };
    };
    // The bundle's advisory name, else its id: a save is filed by set/id.
    let app = if bundle.app_name.is_empty() { bundle.id.clone() } else { bundle.app_name.clone() };
    match import_bundle(peers, writer, peer_id, &bundle, now_ms) {
        ImportOutcome::ImportedReplacing => FileImport::ImportedReplacing { app },
        ImportOutcome::Imported => FileImport::Imported { app },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::paths;

    #[tokio::test]
    async fn a_backup_bundles_with_its_own_stamp_and_a_picked_non_save_says_so() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let writer = peers.writer_handle_for(&pid).expect("writer");
        peers.seed_write(&pid, app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 42), AppSave::new("old").to_entity());
        let b = bundle_backup(&peers, &pid, paths::GAMES_SET, "chess", "Chess", 42).expect("a backup bundles");
        assert_eq!((b.state.as_str(), b.saved_at_ms, b.app_name.as_str()), ("old", 42, "Chess"));
        assert!(bundle_backup(&peers, &pid, paths::GAMES_SET, "chess", "Chess", 43).is_none());

        assert_eq!(
            import_file(&peers, &writer, &pid, "photo.jpg", b"\xff\xd8", 1),
            FileImport::NotASave { file_name: "photo.jpg".into() }
        );
        assert_eq!(
            import_file(&peers, &writer, &pid, "x.entitysave", &b.to_bytes(), 1),
            FileImport::Imported { app: "Chess".into() }
        );
        assert_eq!(
            import_file(&peers, &writer, &pid, "x.entitysave", &b.to_bytes(), 2),
            FileImport::ImportedReplacing { app: "Chess".into() }
        );
    }

    /// The USB flow end to end at the data layer: a save leaves one profile as
    /// bytes, arrives in another, and the second profile's save is backed up on
    /// the way past rather than lost.
    #[tokio::test]
    async fn a_save_leaves_as_bytes_and_an_import_backs_up_what_it_replaces() {
        let from = Peers::new_direct();
        let from_pid = from.primary_peer_id().to_string();
        seed_save(&from, &from_pid, paths::GAMES_SET, "chess", "{\"board\":\"e4\"}");
        let bytes = bundle_live(&from, &from_pid, paths::GAMES_SET, "chess", "Chess", 7)
            .expect("a live save bundles")
            .to_bytes();

        let to = Peers::new_direct();
        let to_pid = to.primary_peer_id().to_string();
        let writer = to.writer_handle_for(&to_pid).expect("writer");
        let bundle = SaveBundle::from_bytes(&bytes).expect("the bytes are a bundle");

        assert_eq!(import_bundle(&to, &writer, &to_pid, &bundle, 1_000), ImportOutcome::Imported);
        assert!(list_backups(&to, &to_pid, paths::GAMES_SET, "chess").is_empty(), "nothing replaced, nothing backed up");

        seed_save(&to, &to_pid, paths::GAMES_SET, "chess", "{\"board\":\"d4\"}");
        assert_eq!(
            import_bundle(&to, &writer, &to_pid, &bundle, 2_000),
            ImportOutcome::ImportedReplacing
        );
        let backups = list_backups(&to, &to_pid, paths::GAMES_SET, "chess");
        assert_eq!(backups.len(), 1, "the replaced save was backed up first");
        let saves = list_saves(&to, &to_pid, paths::GAMES_SET);
        assert_eq!(saves[0].bytes, "{\"board\":\"e4\"}".len(), "the imported state is live");
    }

    #[tokio::test]
    async fn backup_reports_no_save_and_unchanged_as_their_own_outcomes() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let writer = peers.writer_handle_for(&pid).expect("writer");
        assert_eq!(backup_live(&peers, &writer, &pid, paths::GAMES_SET, "chess", 1), BackupOutcome::NoSave);
        seed_save(&peers, &pid, paths::GAMES_SET, "chess", "s");
        assert_eq!(backup_live(&peers, &writer, &pid, paths::GAMES_SET, "chess", 2), BackupOutcome::BackedUp);
        assert_eq!(backup_live(&peers, &writer, &pid, paths::GAMES_SET, "chess", 3), BackupOutcome::Unchanged);
    }

    fn seed_save(peers: &Peers, pid: &str, set: &str, id: &str, state: &str) {
        peers.seed_write(
            pid,
            app_paths::app_save_path(APP_ID, pid, set, id),
            AppSave::new(state).to_entity(),
        );
    }

    #[tokio::test]
    async fn saves_are_listed_from_the_save_prefix_not_the_catalog() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // No catalog is seeded anywhere here — that is the point. An app whose
        // publisher went away still has a save the user owns.
        seed_save(&peers, &pid, paths::GAMES_SET, "chess", "{\"board\":1}");
        seed_save(&peers, &pid, paths::APPS_SET, "synth", "{\"bpm\":120}");

        let all = list_all_saves(&peers, &pid);
        assert_eq!(all.len(), 2, "one per set: {all:?}");
        assert_eq!(all[0].set, paths::GAMES_SET);
        assert_eq!(all[0].id, "chess");
        assert_eq!(all[0].bytes, "{\"board\":1}".len());
        assert_eq!(all[1].set, paths::APPS_SET);
        assert_eq!(all[1].id, "synth");
    }

    /// Ids collide across sets — the reason `app_save_path` is set-keyed — so
    /// two saves for "the same" id must stay two rows with two states.
    #[tokio::test]
    async fn the_same_id_in_two_sets_is_two_independent_saves() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        seed_save(&peers, &pid, paths::GAMES_SET, "loom", "game-state");
        seed_save(&peers, &pid, paths::APPS_SET, "loom", "tool-state");

        let all = list_all_saves(&peers, &pid);
        assert_eq!(all.len(), 2);
        assert_ne!(all[0].hash, all[1].hash, "different states, different blobs");
        assert_eq!(all[0].bytes, "game-state".len());
        assert_eq!(all[1].bytes, "tool-state".len());
    }

    /// Backups are newest-first and keyed by a stamp that sorts chronologically
    /// as a STRING — the zero-padding in `app_backup_path` is what makes the
    /// tree's own listing order usable.
    #[tokio::test]
    async fn backups_come_back_newest_first_across_a_digit_boundary() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // 999_999_999_999 is 12 digits, 1_000_000_000_000 is 13: unpadded, the
        // shorter one sorts AFTER the longer and the panel lies about order.
        for (stamp, state) in [(999_999_999_999u64, "older"), (1_000_000_000_000u64, "newer")] {
            peers.seed_write(
                &pid,
                app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", stamp),
                AppSave::new(state).to_entity(),
            );
        }
        let backups = list_backups(&peers, &pid, paths::GAMES_SET, "chess");
        assert_eq!(backups.len(), 2);
        assert_eq!(backups[0].stamp_ms, 1_000_000_000_000);
        assert_eq!(backups[1].stamp_ms, 999_999_999_999);
    }

    /// A backup is not a live save: it must not appear in the app's own save
    /// list, or the launcher would show phantom apps named after timestamps.
    #[tokio::test]
    async fn a_backup_is_not_listed_as_a_live_save() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        seed_save(&peers, &pid, paths::GAMES_SET, "chess", "live");
        peers.seed_write(
            &pid,
            app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 1_700_000_000_000),
            AppSave::new("snapshot").to_entity(),
        );
        let saves = list_saves(&peers, &pid, paths::GAMES_SET);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].id, "chess");
        assert_eq!(saves[0].bytes, "live".len());
    }

    #[tokio::test]
    async fn a_second_backup_of_an_unchanged_save_is_recognised_as_redundant() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        seed_save(&peers, &pid, paths::GAMES_SET, "chess", "same");
        assert!(
            !already_backed_up(&peers, &pid, paths::GAMES_SET, "chess"),
            "no backups yet"
        );
        peers.seed_write(
            &pid,
            app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 1_700_000_000_000),
            AppSave::new("same").to_entity(),
        );
        assert!(already_backed_up(&peers, &pid, paths::GAMES_SET, "chess"));
        // …and a move since the backup makes it worth taking another.
        seed_save(&peers, &pid, paths::GAMES_SET, "chess", "moved");
        assert!(!already_backed_up(&peers, &pid, paths::GAMES_SET, "chess"));
    }

    #[test]
    fn a_bundle_round_trips_through_the_bytes_that_cross_the_wire() {
        let b = SaveBundle {
            set: paths::GAMES_SET.into(),
            id: "chess".into(),
            app_name: "Chess".into(),
            state: "{\"board\":\"e4\"}".into(),
            saved_at_ms: 1_755_630_000_000,
        };
        assert_eq!(SaveBundle::from_bytes(&b.to_bytes()), Some(b.clone()));
        assert_eq!(b.file_name(), "games-chess.entitysave");
        assert_eq!(b.to_entity().entity_type, SAVE_BUNDLE_TYPE);
    }

    /// The import surface pulls from a list of everything a peer offers, so it
    /// meets ordinary files. Those are "not a save", not "a corrupt save".
    #[test]
    fn anything_that_is_not_a_bundle_decodes_to_none() {
        assert_eq!(SaveBundle::from_bytes(b"hello, world"), None);
        assert_eq!(SaveBundle::from_bytes(&[]), None);
        // Well-formed CBOR that simply is not a bundle — an app save's own
        // encoding, which names no app and so cannot be filed.
        let bare = AppSave::new("just state").to_entity().data;
        assert_eq!(SaveBundle::from_bytes(&bare), None);
    }
}
