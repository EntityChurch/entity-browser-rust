//! `file_kinds` — which entities in the tree are **files**, and what a person may
//! do with each. The Files window is a query over this table.
//!
//! **A file is an entity that names a run of bytes a person would recognise as
//! one thing** (`DESIGN-2026-09-14-b` §2). The line is drawn **by type, never by
//! path**: a path is a place, a type is a meaning — the same reason AP42's guard
//! reads `entity_type`. Everything else in the tree is an entity, not a file.
//!
//! Two things make this a table rather than a match in the window:
//!
//! - **One row per file-carrying type** ([`KINDS`]), each naming its place and
//!   the actions it permits. Actions are per kind, not per place, because
//!   *remove* is right for a kept file and wrong for an offer — withdrawing an
//!   offer is a different act with a different consequence.
//! - **A census** (`every_module_that_encodes_a_blob_pointer_is_classified`):
//!   a module that starts encoding a `blob` pointer must say here whether what
//!   it writes is a file. AP44 — *if the rule needs "every", the structure has to
//!   enforce it* — because a new kind of file nobody classified is a file the
//!   Files window silently does not show.

use crate::app_paths::{self, APP_ID};
use crate::apps::format::APP_SAVE_TYPE;
use crate::apps::workspace::{self, WorkFile};
use crate::file_offer;
use crate::kept_files;
use crate::peers::Peers;
use crate::user_files;

/// Where a file lives, as a person thinks of it. The Files window's places, in
/// display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// Files a person brought onto this device (`crate::user_files`) — private.
    MyFiles,
    /// Files apps handed the host (`x-file`) — private, beside each app.
    AppFiles,
    /// Apps' live saves and their backups.
    Saves,
    /// An app's working files (`x-work-*`).
    Workspace,
    /// What this device offers to peers — the one listed place.
    Offered,
}

impl Place {
    pub const ALL: [Place; 5] = [Place::MyFiles, Place::AppFiles, Place::Saves, Place::Workspace, Place::Offered];

    /// Stable key — carried in DOM event values and `data-place` hooks.
    pub fn key(self) -> &'static str {
        match self {
            Place::MyFiles => "my-files",
            Place::AppFiles => "app-files",
            Place::Saves => "saves",
            Place::Workspace => "workspace",
            Place::Offered => "offered",
        }
    }

    pub fn from_key(key: &str) -> Option<Place> {
        Place::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// What a person may do with a file. Each has exactly one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileAction {
    /// Download the bytes to this device.
    SaveToDevice,
    /// Package a save as a `.entitysave` file and download it — the half of
    /// the USB flow that leaves the device.
    DownloadSaveFile,
    /// Offer it to peers — the deliberate act that shares a private file.
    OfferToPeers,
    /// Stop listing an offer. The bytes stay (they cannot be reclaimed today).
    StopOffering,
    /// Remove a private file or a backup from this device's tree.
    Remove,
}

/// A row of [`KINDS`].
#[derive(Debug, Clone, Copy)]
pub struct FileKind {
    pub type_tag: &'static str,
    pub place: Place,
    /// The most a file of this kind permits; a row may permit fewer
    /// ([`FileRef::actions`]).
    pub actions: &'static [FileAction],
}

/// Every entity type that is a file. See the module note.
pub const KINDS: &[FileKind] = &[
    FileKind {
        type_tag: user_files::USER_FILE_TYPE,
        place: Place::MyFiles,
        actions: &[FileAction::SaveToDevice, FileAction::OfferToPeers, FileAction::Remove],
    },
    FileKind {
        type_tag: kept_files::KEPT_TYPE,
        place: Place::AppFiles,
        actions: &[FileAction::SaveToDevice, FileAction::OfferToPeers, FileAction::Remove],
    },
    FileKind {
        type_tag: APP_SAVE_TYPE,
        place: Place::Saves,
        actions: &[FileAction::DownloadSaveFile, FileAction::Remove],
    },
    FileKind {
        type_tag: workspace::FILE_TYPE,
        place: Place::Workspace,
        actions: &[FileAction::SaveToDevice],
    },
    FileKind {
        type_tag: file_offer::OFFER_TYPE,
        place: Place::Offered,
        actions: &[FileAction::SaveToDevice, FileAction::StopOffering],
    },
];

/// The kind for an entity type, or `None` — *that entity is not a file*.
pub fn kind_for(type_tag: &str) -> Option<&'static FileKind> {
    KINDS.iter().find(|k| k.type_tag == type_tag)
}

/// Which file a row is — enough to act on it, and encodable into a DOM event
/// value. Every variant is addressed by what identifies it in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRef {
    Mine { blob_hex: String },
    Kept { set: String, app: String, blob_hex: String },
    Save { set: String, id: String },
    Backup { set: String, id: String, stamp: u64 },
    Work { set: String, app: String, path: String },
    Offer { blob_hex: String },
}

impl FileRef {
    /// The entity type this ref points at — the join back to [`KINDS`].
    pub fn type_tag(&self) -> &'static str {
        match self {
            FileRef::Mine { .. } => user_files::USER_FILE_TYPE,
            FileRef::Kept { .. } => kept_files::KEPT_TYPE,
            FileRef::Save { .. } | FileRef::Backup { .. } => APP_SAVE_TYPE,
            FileRef::Work { .. } => workspace::FILE_TYPE,
            FileRef::Offer { .. } => file_offer::OFFER_TYPE,
        }
    }

    /// Where this file is listed — decided by its kind's row, so the table and
    /// the listing cannot disagree about a place.
    pub fn place(&self) -> Place {
        kind_for(self.type_tag()).expect("every FileRef variant has a kind").place
    }

    /// The actions this particular file permits: its kind's, narrowed where
    /// the row says more than the type does. A live save is the app's — the app
    /// writes it on every move — so it is downloaded, never removed from here.
    pub fn actions(&self) -> Vec<FileAction> {
        let kind = kind_for(self.type_tag()).expect("every FileRef variant has a kind");
        kind.actions
            .iter()
            .copied()
            .filter(|a| !(matches!(self, FileRef::Save { .. }) && *a == FileAction::Remove))
            .collect()
    }

    /// The tree path of the entity this file is — where the Entity Tree shows
    /// it. A file the File Manager lists is always an entity at a path; this
    /// is that path, from the same builders that write it.
    pub fn tree_path(&self, peer_id: &str) -> String {
        match self {
            FileRef::Mine { blob_hex } => app_paths::user_file_path(APP_ID, peer_id, blob_hex),
            FileRef::Kept { set, app, blob_hex } => app_paths::app_file_path(APP_ID, peer_id, set, app, blob_hex),
            FileRef::Save { set, id } => app_paths::app_save_path(APP_ID, peer_id, set, id),
            FileRef::Backup { set, id, stamp } => format!("{}{stamp}", app_paths::app_backups_for(APP_ID, peer_id, set, id)),
            FileRef::Work { set, app, path } => format!("{}{path}", app_paths::app_workspace_prefix(APP_ID, peer_id, set, app)),
            FileRef::Offer { blob_hex } => app_paths::offer_path(APP_ID, peer_id, blob_hex),
        }
    }

    /// `tag|field|field…` — for a DOM event value. Only the LAST field may
    /// contain `|`: sets and app ids are catalog slugs, blob ids are hex, stamps
    /// are digits — but a workspace path is whatever `assets::valid_key` admits,
    /// and that admits `|`. So a decode splits at most as many times as the tag
    /// has fields, and the path keeps the rest.
    pub fn encode(&self) -> String {
        match self {
            FileRef::Mine { blob_hex } => format!("mine|{blob_hex}"),
            FileRef::Kept { set, app, blob_hex } => format!("kept|{set}|{app}|{blob_hex}"),
            FileRef::Save { set, id } => format!("save|{set}|{id}"),
            FileRef::Backup { set, id, stamp } => format!("backup|{set}|{id}|{stamp}"),
            FileRef::Work { set, app, path } => format!("work|{set}|{app}|{path}"),
            FileRef::Offer { blob_hex } => format!("offer|{blob_hex}"),
        }
    }

    pub fn decode(value: &str) -> Option<FileRef> {
        let (tag, rest) = value.split_once('|')?;
        let fields = |n: usize| -> Option<Vec<String>> {
            let parts: Vec<String> = rest.splitn(n, '|').map(str::to_string).collect();
            (parts.len() == n && parts.iter().all(|p| !p.is_empty())).then_some(parts)
        };
        match tag {
            "mine" => fields(1).and_then(|f| (!f[0].contains('|')).then(|| FileRef::Mine { blob_hex: f[0].clone() })),
            "kept" => fields(3).and_then(|f| {
                (!f[2].contains('|')).then(|| FileRef::Kept { set: f[0].clone(), app: f[1].clone(), blob_hex: f[2].clone() })
            }),
            "save" => fields(2).and_then(|f| {
                (!f[1].contains('|')).then(|| FileRef::Save { set: f[0].clone(), id: f[1].clone() })
            }),
            "backup" => fields(3).and_then(|f| {
                Some(FileRef::Backup { set: f[0].clone(), id: f[1].clone(), stamp: f[2].parse().ok()? })
            }),
            "work" => fields(3).map(|f| FileRef::Work { set: f[0].clone(), app: f[1].clone(), path: f[2].clone() }),
            "offer" => fields(1).and_then(|f| (!f[0].contains('|')).then(|| FileRef::Offer { blob_hex: f[0].clone() })),
            _ => None,
        }
    }
}

/// One file, as the Files window lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub name: String,
    pub size: u64,
    /// Who the file belongs to, in a person's words: the app's name for a kept
    /// file, `set/id` for a save or a workspace file. Empty for an offer.
    pub owner: String,
    /// Epoch ms when that is known (a backup's stamp, a workspace file's mtime).
    pub when_ms: Option<u64>,
    pub file: FileRef,
}

impl FileRow {
    pub fn place(&self) -> Place {
        self.file.place()
    }
}

/// Every file this peer holds, in [`Place::ALL`] order, each place sorted so
/// rows do not reshuffle as files arrive. The caller subscribes the prefixes
/// ([`watch_prefixes`]) for the Worker arm.
pub fn list(peers: &Peers, peer_id: &str) -> Vec<FileRow> {
    let mut rows = Vec::new();

    for f in user_files::read_mine(peers, peer_id) {
        rows.push(FileRow {
            name: f.name.clone(),
            size: f.size,
            owner: String::new(),
            when_ms: None,
            file: FileRef::Mine { blob_hex: f.id() },
        });
    }

    for f in kept_files::read_kept(peers, peer_id) {
        let Some(src) = f.source.clone() else { continue };
        rows.push(FileRow {
                        name: f.name.clone(),
            size: f.size,
            owner: src.name.clone(),
            when_ms: None,
            file: FileRef::Kept { set: src.set, app: src.app, blob_hex: f.id() },
        });
    }

    for save in crate::apps::saves::list_all_saves(peers, peer_id) {
        rows.push(FileRow {
                        name: format!("{}.json", save.id), // i18n-ignore — a file name
            size: save.bytes as u64,
            owner: format!("{}/{}", save.set, save.id), // i18n-ignore — an address
            when_ms: None,
            file: FileRef::Save { set: save.set.clone(), id: save.id.clone() },
        });
        for b in crate::apps::saves::list_backups(peers, peer_id, &save.set, &save.id) {
            rows.push(FileRow {
                                name: format!("{}.json", save.id), // i18n-ignore — a file name
                size: b.bytes as u64,
                owner: format!("{}/{}", save.set, save.id), // i18n-ignore — an address
                when_ms: Some(b.stamp_ms),
                file: FileRef::Backup { set: save.set.clone(), id: save.id.clone(), stamp: b.stamp_ms },
            });
        }
    }

    for set in crate::apps::paths::APP_SETS {
        let prefix = format!("/{peer_id}/app/{APP_ID}/apps/{set}/work/");
        let mut work: Vec<FileRow> = peers
            .tree_listing(peer_id, &prefix)
            .into_iter()
            .filter_map(|entry| {
                let rest = entry.path.strip_prefix(&prefix)?;
                let (app, path) = rest.split_once('/')?;
                if app.is_empty() || path.is_empty() {
                    return None;
                }
                let file = WorkFile::from_entity(&peers.get_entity(peer_id, &entry.path)?).ok()?;
                Some(FileRow {
                                        name: path.to_string(),
                    size: file.size,
                    owner: format!("{set}/{app}"), // i18n-ignore — an address
                    // `mtime` is Unix SECONDS (the guest's clock, as a FAT/POSIX
                    // stamp); a row's `when_ms` is milliseconds.
                    when_ms: (file.mtime > 0).then(|| file.mtime.saturating_mul(1000)),
                    file: FileRef::Work { set: set.to_string(), app: app.to_string(), path: path.to_string() },
                })
            })
            .collect();
        work.sort_by(|a, b| a.owner.cmp(&b.owner).then(a.name.cmp(&b.name)));
        rows.extend(work);
    }

    let mut offered: Vec<FileRow> = file_offer::read_own_offers(peers, peer_id)
        .into_iter()
        .map(|o| FileRow {
                        name: o.name.clone(),
            size: o.size,
            owner: o.source.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
            when_ms: None,
            file: FileRef::Offer { blob_hex: o.id() },
        })
        .collect();
    offered.sort_by(|a, b| a.name.cmp(&b.name));
    rows.extend(offered);

    rows
}

/// The prefixes [`list`] reads — what a window over it must subscribe.
pub fn watch_prefixes(peer_id: &str) -> Vec<String> {
    let mut out = vec![app_paths::offers_prefix(APP_ID, peer_id), app_paths::user_files_prefix(APP_ID, peer_id)];
    for set in crate::apps::paths::APP_SETS {
        out.push(format!("/{peer_id}/app/{APP_ID}/apps/{set}/"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::format::AppSave;
    use crate::apps::paths;

    #[test]
    fn every_ref_round_trips_through_its_encoding_and_joins_a_kind() {
        let refs = [
            FileRef::Mine { blob_hex: "00ef".into() },
            FileRef::Kept { set: "apps".into(), app: "kolibri".into(), blob_hex: "00ab".into() },
            FileRef::Save { set: "games".into(), id: "chess".into() },
            FileRef::Backup { set: "games".into(), id: "chess".into(), stamp: 1_700_000_000_000 },
            FileRef::Work { set: "apps".into(), app: "alpine".into(), path: "root/notes.md".into() },
            // `assets::valid_key` admits `|`, so a path may carry one.
            FileRef::Work { set: "apps".into(), app: "alpine".into(), path: "a|b/c|d".into() },
            FileRef::Offer { blob_hex: "00cd".into() },
        ];
        for r in refs {
            assert_eq!(FileRef::decode(&r.encode()), Some(r.clone()), "{r:?}");
            assert!(kind_for(r.type_tag()).is_some(), "{r:?} has no kind");
            assert!(!r.actions().is_empty(), "{r:?} permits nothing");
        }
        assert_eq!(FileRef::decode("save|games"), None, "short");
        assert_eq!(FileRef::decode("backup|games|chess|soon"), None, "stamp not a number");
        assert_eq!(FileRef::decode("save||chess"), None, "empty field");
        assert_eq!(FileRef::decode("elsewhere|x"), None, "unknown tag");
        assert_eq!(FileRef::decode("save|games|chess|extra"), None, "too many fields");
        assert_eq!(FileRef::decode("save"), None, "no fields");
    }

    /// A live save is the app's; removing it from a file list would pull the
    /// state out from under a running game. Only a backup is removable here.
    #[test]
    fn a_live_save_cannot_be_removed_and_a_backup_can() {
        let live = FileRef::Save { set: "games".into(), id: "chess".into() };
        let backup = FileRef::Backup { set: "games".into(), id: "chess".into(), stamp: 1 };
        assert!(!live.actions().contains(&FileAction::Remove));
        assert!(backup.actions().contains(&FileAction::Remove));
        assert!(live.actions().contains(&FileAction::DownloadSaveFile));
    }

    /// Sharing stays deliberate: no kind but a private one offers itself, and an
    /// offer is stopped, never "removed" (the bytes stay — the word must not
    /// promise otherwise).
    #[test]
    fn only_private_files_offer_and_offers_are_never_removed() {
        for k in KINDS {
            if k.actions.contains(&FileAction::OfferToPeers) {
                assert_ne!(k.place, Place::Offered);
            }
            if k.place == Place::Offered {
                assert!(!k.actions.contains(&FileAction::Remove));
                assert!(k.actions.contains(&FileAction::StopOffering));
            }
        }
        let tags: std::collections::HashSet<_> = KINDS.iter().map(|k| k.type_tag).collect();
        assert_eq!(tags.len(), KINDS.len(), "a type is one kind");
    }

    #[tokio::test]
    async fn list_finds_saves_and_backups_by_type_and_skips_what_is_not_a_file() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        peers.seed_write(&pid, app_paths::app_save_path(APP_ID, &pid, paths::GAMES_SET, "chess"), AppSave::new("{}").to_entity());
        peers.seed_write(
            &pid,
            app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 1_700_000_000_000),
            AppSave::new("{\"a\":1}").to_entity(),
        );
        // Under a place a file could live, but not a file type: not listed.
        peers.seed_write(
            &pid,
            format!("/{pid}/app/{APP_ID}/apps/{}/work/alpine/notes.md", paths::APPS_SET),
            entity_entity::Entity::new("app/something-else", entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![]))).unwrap(),
        );
        let rows = list(&peers, &pid);
        let saves: Vec<_> = rows.iter().filter(|r| r.place() == Place::Saves).collect();
        assert_eq!(saves.len(), 2, "{rows:?}");
        assert!(matches!(saves[0].file, FileRef::Save { .. }));
        assert_eq!(saves[1].when_ms, Some(1_700_000_000_000));
        assert!(rows.iter().all(|r| r.place() != Place::Workspace), "a non-file type is not a file: {rows:?}");
    }

    /// "Show in Entity Tree" is only honest if the path it opens is where the
    /// row's entity actually is: every listed row's `tree_path` resolves to an
    /// entity of the row's own type.
    #[tokio::test]
    async fn every_listed_file_names_the_tree_path_its_entity_is_at() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        peers.seed_write(&pid, app_paths::app_save_path(APP_ID, &pid, paths::GAMES_SET, "chess"), AppSave::new("{}").to_entity());
        peers.seed_write(
            &pid,
            app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 1_700_000_000_000),
            AppSave::new("{\"a\":1}").to_entity(),
        );
        let rows = list(&peers, &pid);
        assert_eq!(rows.len(), 2, "anti-vacuity: {rows:?}");
        for r in &rows {
            let path = r.file.tree_path(&pid);
            let e = peers.get_entity(&pid, &path).unwrap_or_else(|| panic!("{r:?} names {path}, which holds nothing"));
            assert_eq!(e.entity_type, r.file.type_tag(), "{path}");
        }
        // The rest are built by the same builders their writers call; pinned by shape.
        assert_eq!(FileRef::Mine { blob_hex: "00ef".into() }.tree_path(&pid), app_paths::user_file_path(APP_ID, &pid, "00ef"));
        assert_eq!(
            FileRef::Work { set: "apps".into(), app: "alpine".into(), path: "root/n.md".into() }.tree_path(&pid),
            format!("/{pid}/app/{APP_ID}/apps/apps/work/alpine/root/n.md")
        );
        assert_eq!(FileRef::Offer { blob_hex: "00cd".into() }.tree_path(&pid), app_paths::offer_path(APP_ID, &pid, "00cd"));
    }

    /// The census. A module whose code encodes a `"blob"` pointer writes
    /// something that names bytes; it must be classified here — as the module
    /// behind a [`KINDS`] row, or as not-a-file with its reason. Scans code, not
    /// comments. Stated bound: it sees the `blob` key spelling only; EMBED's
    /// `payload.hash` pointers are classified by their module by hand below.
    #[test]
    fn every_module_that_encodes_a_blob_pointer_is_classified() {
        const FILES: &[(&str, &str)] = &[
            ("src/file_offer.rs", "offers (OFFER_TYPE) and the manifest kept files reuse (KEPT_TYPE)"),
            ("src/apps/workspace.rs", "workspace files (FILE_TYPE)"),
        ];
        const NOT_FILES: &[(&str, &str)] = &[
            ("src/share.rs", "a share POINTS AT a file or prefix; the file is its offer"),
            ("src/apps/assets.rs", "an app bundle's asset index — publisher input, not the person's file"),
            ("src/embed.rs", "an embed payload inside a site asset or feed entry"),
            ("src/content_site/gpin4_joint_fixture.rs", "a cross-impl test fixture"),
            ("src/share_crossimpl_fixture.rs", "a cross-impl test fixture (app/share bodies for the other seat)"),
        ];
        let mut found = Vec::new();
        let mut stack = vec![std::path::PathBuf::from("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let code = text
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .any(|l| l.contains("\"blob\""));
                    if code {
                        found.push(path.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        assert!(found.len() >= 2, "anti-vacuity: the scan found {found:?}");
        let classified: Vec<&str> = FILES.iter().chain(NOT_FILES).map(|(p, _)| *p).collect();
        let missing: Vec<_> = found.iter().filter(|p| !classified.contains(&p.as_str())).collect();
        assert!(
            missing.is_empty(),
            "these modules encode a blob pointer and nobody said whether what they write is a file \
             — add a KINDS row or a NOT_FILES entry: {missing:?}"
        );
        let stale: Vec<_> = classified.iter().filter(|p| !found.iter().any(|f| f == *p)).collect();
        assert!(stale.is_empty(), "classified modules that no longer encode a blob pointer: {stale:?}");
    }
}
