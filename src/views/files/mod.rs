//! Files window — one home for working with this device's own files.
//!
//! `DESIGN-2026-09-14-b`. File Transfer is about **peers** (what a device
//! offers, what I pull); Files is about **my files**: what apps kept, saves and
//! their backups, apps' working files, and what I offer. It is a query over
//! [`crate::file_kinds`], which decides what counts as a file, and every action
//! it offers has exactly one implementation somewhere else:
//!
//! - save / offer / stop offering a kept file or an offer → the app-level
//!   actions File Transfer already raises (`Action::SaveKeptFile`, …), which wake
//!   this window too (`app::shows_own_files`);
//! - backups, save files, imports → `crate::apps::saves`, shared with the Apps
//!   window's Saves panel;
//! - a working file's bytes → `crate::apps::workspace::read`.
//!
//! **No persisted window state**, deliberately: which place someone is looking
//! at is not a fact about the profile, so the window is clear of AP42's reused
//! slot by construction. The selected place and the last notice are session-only.

pub mod output;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[allow(unused_imports)]
use crate::action::Action;
use crate::file_kinds::{self, FileRef, Place};
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};
use crate::window_watch::WindowWatch;
use output::{FilesNotice, FilesOutput};

pub const TYPE_NAME: &str = "Files"; // i18n-ignore — identity key; display via window.files

/// Show a place (`value` = [`Place::key`]).
pub const PLACE_EVENT: &str = "files_place";
/// Remove a private file or a backup (`value` = [`FileRef::encode`]).
pub const REMOVE_EVENT: &str = "files_remove";
/// Download a save or a backup as a `.entitysave` file.
pub const DOWNLOAD_SAVE_EVENT: &str = "files_download_save";
/// Download an app's working file.
pub const DOWNLOAD_WORK_EVENT: &str = "files_download_work";
/// A save file was picked; its bytes are in the window's inbox.
pub const IMPORT_EVENT: &str = "files_import";
/// Files were picked or dropped for My files; they are in [`AddInbox`].
pub const ADD_EVENT: &str = "files_add";
/// Download every file in a place as one `.tar.gz` (`value` = [`Place::key`]).
pub const EXPORT_TAR_EVENT: &str = "files_export_tar";
/// Download every file in a place as one `.zip` (`value` = [`Place::key`]).
pub const EXPORT_ZIP_EVENT: &str = "files_export_zip";

/// Which plain archive a *Download all* writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// Opens by double-click on Windows, macOS and Linux desktops.
    Zip,
    /// What Unix tooling expects; a plain `.tar` where the engine has no gzip.
    TarGz,
}

/// Where one exported file's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportSource {
    /// Already in hand (a save's state, a working file read from the store).
    Inline(Vec<u8>),
    /// In one of our own `system/content` namespaces, read by closure walk.
    Stored { namespace: &'static str, blob: entity_hash::Hash },
    /// Could not be read; the reason is reported, the file left out.
    Unreadable(String),
}

/// The archive plan for a place: `(path inside the archive, unix seconds, source)`.
pub type ExportPlan = Vec<(String, u64, ExportSource)>;

/// Bytes a file picker read, waiting for the window to act on them. The DOM
/// callback holds a clone; `handle_action` takes it. An inbox rather than an
/// `Action` carrying bytes, because the only consumer is this window.
pub type Inbox = Rc<RefCell<Option<(String, Vec<u8>)>>>;

/// Files on their way into My files: each a name and its bytes, or why the
/// bytes could not be had (too large to read, a read that failed).
pub type AddInbox = Rc<RefCell<Vec<(String, Result<Vec<u8>, String>)>>>;

pub struct FilesWindow {
    peer_id: String,
    watch: WindowWatch,
    /// `None` until the window first lists: then the first place holding
    /// anything, else My files — so the window opens where the person's files
    /// are (File Transfer's "Open File Manager" is about files apps kept), and
    /// on an empty device, where a person puts things.
    place: Cell<Option<Place>>,
    /// Shared with the async keep of an added file, which reports when it lands.
    notice: Rc<RefCell<Option<FilesNotice>>>,
    #[allow(dead_code)] // read on the wasm render path
    inbox: Inbox,
    #[allow(dead_code)] // filled on the wasm render path
    adds: AddInbox,
}

impl FilesWindow {
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            watch: WindowWatch::new(),
            place: Cell::new(None),
            notice: Rc::new(RefCell::new(None)),
            inbox: Rc::new(RefCell::new(None)),
            adds: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: TYPE_NAME,
            description: "This device's own files: kept by apps, saves, working files, offers", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |_id, peer_id, pm| {
                let mut window = FilesWindow::new(peer_id.to_string());
                for prefix in file_kinds::watch_prefixes(peer_id) {
                    pm.watch_prefix(&mut window.watch, peer_id, prefix);
                }
                Box::new(window)
            },
        }
    }

    fn say(&self, notice: FilesNotice) {
        *self.notice.borrow_mut() = Some(notice);
        self.watch.mark_dirty();
    }

    pub fn output(&self, peers: &Peers) -> FilesOutput {
        let rows = file_kinds::list(peers, &self.peer_id);
        let counts = Place::ALL
            .iter()
            .map(|p| (*p, rows.iter().filter(|r| r.place() == *p).count()))
            .collect();
        let place = self.place.get().unwrap_or_else(|| {
            let first = Place::ALL.into_iter().find(|p| rows.iter().any(|r| r.place() == *p)).unwrap_or(Place::MyFiles);
            self.place.set(Some(first));
            first
        });
        FilesOutput {
            peer_id: self.peer_id.clone(),
            place,
            counts,
            rows: rows.into_iter().filter(|r| r.place() == place).collect(),
            notice: self.notice.borrow().clone(),
        }
    }

    fn remove(&self, peers: &Peers, file: &FileRef) {
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else { return };
        match file {
            // The manifest goes; the bytes stay in this device's content store,
            // unlisted, until storage reclaim exists (buildout item 21) — the
            // notice says so rather than promising a delete.
            FileRef::Kept { set, app, blob_hex } => {
                writer.remove(app_paths_kept(&self.peer_id, set, app, blob_hex));
                self.say(FilesNotice::Info(crate::i18n::t("files.removed_kept", &[])));
            }
            FileRef::Mine { blob_hex } => {
                writer.remove(crate::app_paths::user_file_path(crate::app_paths::APP_ID, &self.peer_id, blob_hex));
                self.say(FilesNotice::Info(crate::i18n::t("files.removed_kept", &[])));
            }
            FileRef::Backup { set, id, stamp } => {
                crate::apps::saves::drop_backup(peers, &writer, &self.peer_id, set, id, *stamp);
                self.say(FilesNotice::Info(crate::i18n::t("saves.backup_dropped", &[])));
            }
            // Refused by `FileRef::actions`; a hand-made event is ignored.
            FileRef::Save { .. } | FileRef::Work { .. } | FileRef::Offer { .. } => {}
        }
    }

    /// A save or a backup as the bundle a `.entitysave` file holds.
    fn save_bundle(&self, peers: &Peers, file: &FileRef) -> Option<crate::apps::saves::SaveBundle> {
        match file {
            FileRef::Save { set, id } => {
                let name = crate::views::games::catalog_name(peers, &self.peer_id, set, id);
                crate::apps::saves::bundle_live(peers, &self.peer_id, set, id, &name, now_ms())
            }
            FileRef::Backup { set, id, stamp } => {
                let name = crate::views::games::catalog_name(peers, &self.peer_id, set, id);
                crate::apps::saves::bundle_backup(peers, &self.peer_id, set, id, &name, *stamp)
            }
            _ => None,
        }
    }

    fn import(&self, peers: &Peers) {
        let Some((name, bytes)) = self.inbox.borrow_mut().take() else { return };
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else { return };
        let outcome = crate::apps::saves::import_file(peers, &writer, &self.peer_id, &name, &bytes, now_ms());
        let msg = outcome.message();
        self.say(if outcome.is_error() { FilesNotice::Error(msg) } else { FilesNotice::Info(msg) });
    }

    /// Every file in `place`, as the archive will hold it. Sync: the bytes that
    /// need a round trip are named, not fetched ([`ExportSource::Stored`]).
    pub fn export_plan(&self, peers: &Peers, place: Place) -> ExportPlan {
        let stored = |namespace: &'static str, hex: &str| match crate::file_offer::hash_from_id(hex) {
            Ok(blob) => ExportSource::Stored { namespace, blob },
            Err(e) => ExportSource::Unreadable(e),
        };
        let writer = peers.writer_handle_for(&self.peer_id);
        file_kinds::list(peers, &self.peer_id)
            .into_iter()
            .filter(|r| r.place() == place)
            .map(|row| {
                let secs = row.when_ms.map(|ms| ms / 1000).unwrap_or(0);
                match &row.file {
                    FileRef::Mine { blob_hex } => (row.name.clone(), secs, stored(crate::user_files::NAMESPACE, blob_hex)),
                    // Under the app's name: two apps may each keep a `notes.txt`.
                    FileRef::Kept { blob_hex, .. } => {
                        (format!("{}/{}", row.owner, row.name), secs, stored(crate::kept_files::NAMESPACE, blob_hex))
                    }
                    FileRef::Offer { blob_hex } => (row.name.clone(), secs, stored(crate::file_offer::NAMESPACE, blob_hex)),
                    FileRef::Save { set, id } => {
                        let path = crate::app_paths::app_save_path(crate::app_paths::APP_ID, &self.peer_id, set, id);
                        let state = peers.get_entity(&self.peer_id, &path).map(|e| crate::apps::format::AppSave::from_entity(&e).state);
                        (format!("{set}-{id}.json"), secs, state.map(|s| ExportSource::Inline(s.into_bytes())).unwrap_or_else(|| ExportSource::Unreadable("gone".into())))
                    }
                    FileRef::Backup { set, id, stamp } => {
                        let path = crate::app_paths::app_backup_path(crate::app_paths::APP_ID, &self.peer_id, set, id, *stamp);
                        let state = peers.get_entity(&self.peer_id, &path).map(|e| crate::apps::format::AppSave::from_entity(&e).state);
                        (format!("backups/{set}-{id}-{stamp}.json"), secs, state.map(|s| ExportSource::Inline(s.into_bytes())).unwrap_or_else(|| ExportSource::Unreadable("gone".into())))
                    }
                    FileRef::Work { set, app, path } => {
                        let source = match &writer {
                            Some(w) => {
                                let prefix = crate::app_paths::app_workspace_prefix(crate::app_paths::APP_ID, &self.peer_id, set, app);
                                match crate::apps::workspace::read(w, &prefix, path) {
                                    Ok(bytes) => ExportSource::Inline(bytes),
                                    Err(why) => ExportSource::Unreadable(why.to_string()),
                                }
                            }
                            None => ExportSource::Unreadable(crate::file_offer::not_routed_message(&self.peer_id)),
                        };
                        (format!("{set}/{app}/{path}"), secs, source)
                    }
                }
            })
            .collect()
    }

    /// Download every file in `place` as one archive: a `.zip` (members stored
    /// where the engine cannot deflate), or a `.tar.gz` (a plain `.tar` on an
    /// engine without gzip). Files that cannot be read are left out and counted
    /// in the notice — never a silently short archive.
    fn export_archive(&self, peers: &Peers, place: Place, format: ArchiveFormat) {
        let plan = self.export_plan(peers, place);
        if plan.is_empty() {
            self.say(FilesNotice::Info(crate::i18n::t("files.empty", &[])));
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(dispatch) = peers.dispatch_handle(&self.peer_id) else {
                self.say(FilesNotice::Error(crate::file_offer::not_routed_message(&self.peer_id)));
                return;
            };
            let notice = self.notice.clone();
            let flag = self.watch.flag();
            let stem = format!("{}-{}", place.key(), date_stamp());
            *notice.borrow_mut() = Some(FilesNotice::Info(crate::i18n::t("files.exporting", &[("n", &plan.len().to_string())])));
            flag.mark();
            wasm_bindgen_futures::spawn_local(async move {
                let mut entries = Vec::new();
                let mut unread = 0usize;
                for (path, mtime, source) in plan {
                    let bytes = match source {
                        ExportSource::Inline(b) => Ok(b),
                        ExportSource::Stored { namespace, blob } => crate::file_offer::read_own_in(&dispatch, namespace, &blob).await,
                        ExportSource::Unreadable(why) => Err(why),
                    };
                    match bytes {
                        Ok(bytes) => entries.push(crate::archive::ArchiveEntry { path, bytes, mtime }),
                        Err(why) => {
                            tracing::warn!(path = %path, why = %why, "files export: left out a file that could not be read");
                            unread += 1;
                        }
                    }
                }
                crate::archive::unique_names(&mut entries);
                let (name, tar) = match format {
                    ArchiveFormat::Zip => {
                        let mut deflated = Vec::with_capacity(entries.len());
                        for e in &entries {
                            match crate::ops::gzip::deflate_raw(&e.bytes).await {
                                Ok(d) => deflated.push(Some(d)),
                                Err(err) => {
                                    tracing::warn!(error = ?err, "files export: no deflate here, the zip stores its files");
                                    deflated.push(None);
                                }
                            }
                        }
                        (format!("{stem}.zip"), crate::archive::write_zip(&entries, &deflated))
                    }
                    ArchiveFormat::TarGz => {
                        let tar = crate::archive::write_tar(&entries);
                        match crate::ops::gzip::gzip(&tar.bytes).await {
                            Ok(gz) => (format!("{stem}.tar.gz"), crate::archive::Archive { bytes: gz, ..tar }),
                            Err(e) => {
                                tracing::warn!(error = ?e, "files export: no gzip here, handing over a plain .tar");
                                (format!("{stem}.tar"), tar)
                            }
                        }
                    }
                };
                let left_out = unread + tar.skipped.len();
                let said = match crate::ops::download::save_bytes(&name, &tar.bytes) {
                    Ok(()) if left_out == 0 => FilesNotice::Info(crate::i18n::t(
                        "files.exported",
                        &[("name", &name), ("n", &tar.written.to_string())],
                    )),
                    Ok(()) => FilesNotice::Error(crate::i18n::t(
                        "files.exported_partial",
                        &[("name", &name), ("n", &tar.written.to_string()), ("left", &left_out.to_string())],
                    )),
                    Err(why) => FilesNotice::Error(crate::i18n::t("filetransfer.save_failed", &[("name", &name), ("why", &why)])),
                };
                *notice.borrow_mut() = Some(said);
                flag.mark();
            });
        }
    }

    /// Keep what was picked or dropped in My files. Each file is its own
    /// dispatch; the notice names the last one to land, or the first refusal.
    fn add(&self, peers: &Peers) {
        let queued: Vec<_> = self.adds.borrow_mut().drain(..).collect();
        if queued.is_empty() {
            return;
        }
        let dispatch = peers.dispatch_handle(&self.peer_id);
        for (name, got) in queued {
            let bytes = match (got, &dispatch) {
                (Err(why), _) => {
                    self.say(FilesNotice::Error(crate::i18n::t("files.add_failed", &[("name", &name), ("why", &why)])));
                    continue;
                }
                (Ok(_), None) => {
                    let why = crate::file_offer::not_routed_message(&self.peer_id);
                    self.say(FilesNotice::Error(crate::i18n::t("files.add_failed", &[("name", &name), ("why", &why)])));
                    continue;
                }
                (Ok(bytes), Some(_)) => bytes,
            };
            #[cfg(target_arch = "wasm32")]
            {
                let dispatch = dispatch.clone().expect("checked above");
                let notice = self.notice.clone();
                let flag = self.watch.flag();
                wasm_bindgen_futures::spawn_local(async move {
                    let said = match crate::user_files::keep(&dispatch, &name, &bytes).await {
                        Ok(file) => FilesNotice::Info(crate::i18n::t("files.added", &[("name", &file.name)])),
                        Err(why) => FilesNotice::Error(crate::i18n::t("files.add_failed", &[("name", &name), ("why", &why)])),
                    };
                    *notice.borrow_mut() = Some(said);
                    flag.mark();
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = (name, bytes);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn download(&self, name: &str, bytes: &[u8]) {
        match crate::ops::download::save_bytes(name, bytes) {
            Ok(()) => self.say(FilesNotice::Info(crate::i18n::t("files.downloaded", &[("name", name)]))),
            Err(why) => self.say(FilesNotice::Error(crate::i18n::t(
                "filetransfer.save_failed",
                &[("name", name), ("why", &why)],
            ))),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn download(&self, name: &str, _bytes: &[u8]) {
        self.say(FilesNotice::Info(crate::i18n::t("files.downloaded", &[("name", name)])));
    }
}

/// `YYYY-MM-DD` in the device's local time, for an archive's file name.
#[cfg(target_arch = "wasm32")]
fn date_stamp() -> String {
    let d = js_sys::Date::new_0();
    format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}

fn app_paths_kept(peer_id: &str, set: &str, app: &str, blob_hex: &str) -> String {
    crate::app_paths::app_file_path(crate::app_paths::APP_ID, peer_id, set, app, blob_hex)
}

fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

impl WindowView for FilesWindow {
    fn title(&self) -> String {
        crate::i18n::window_title(TYPE_NAME)
    }

    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { event, value, .. } = action else {
            return;
        };
        match event.as_str() {
            PLACE_EVENT => {
                if let Some(place) = Place::from_key(value) {
                    self.place.set(Some(place));
                    *self.notice.borrow_mut() = None;
                }
            }
            REMOVE_EVENT => {
                if let Some(file) = FileRef::decode(value) {
                    self.remove(peers, &file);
                }
            }
            DOWNLOAD_SAVE_EVENT => match FileRef::decode(value).and_then(|f| self.save_bundle(peers, &f)) {
                Some(bundle) => self.download(&bundle.file_name(), &bundle.to_bytes()),
                None => self.say(FilesNotice::Error(crate::i18n::t("saves.err_no_save", &[("app", value)]))),
            },
            DOWNLOAD_WORK_EVENT => {
                if let Some(FileRef::Work { set, app, path }) = FileRef::decode(value) {
                    let Some(writer) = peers.writer_handle_for(&self.peer_id) else { return };
                    let prefix = crate::app_paths::app_workspace_prefix(crate::app_paths::APP_ID, &self.peer_id, &set, &app);
                    match crate::apps::workspace::read(&writer, &prefix, &path) {
                        Ok(bytes) => {
                            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                            self.download(&name, &bytes);
                        }
                        Err(why) => self.say(FilesNotice::Error(crate::i18n::t(
                            "filetransfer.save_failed",
                            &[("name", &path), ("why", &why.to_string())],
                        ))),
                    }
                }
            }
            IMPORT_EVENT => self.import(peers),
            ADD_EVENT => self.add(peers),
            EXPORT_TAR_EVENT | EXPORT_ZIP_EVENT => {
                if let Some(place) = Place::from_key(value) {
                    let format = if event == EXPORT_ZIP_EVENT { ArchiveFormat::Zip } else { ArchiveFormat::TarGz };
                    self.export_archive(peers, place, format);
                }
            }
            _ => {}
        }
        // Every event this window receives is worth a repaint — including the
        // file picker's `ft_wake`, which carries a refusal to the status slot.
        self.watch.mark_dirty();
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(&self, container: &web_sys::Element, peers: &Peers, ctx: &crate::dom::DomCtx) {
        let output = self.output(peers);
        crate::dom::files::render(container, &output, ctx, self.inbox.clone(), self.adds.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::format::AppSave;
    use crate::apps::paths;
    use crate::app_paths::{self, APP_ID};

    fn event(event: &str, value: &str) -> Action {
        Action::WindowEvent { window_id: 1, event: event.into(), value: value.into() }
    }

    #[test]
    fn window_type_is_named_files() {
        assert_eq!(FilesWindow::window_type().name, "Files");
    }

    /// The window's own half of the USB flow: a picked `.entitysave` lands as
    /// the app's save, and a picked file that is not one says so.
    #[tokio::test]
    async fn a_picked_save_file_is_imported_and_a_picked_non_save_is_named_as_such() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let mut w = FilesWindow::new(pid.clone());

        *w.inbox.borrow_mut() = Some(("photo.jpg".into(), b"\xff\xd8\xff".to_vec()));
        w.handle_action(&event(IMPORT_EVENT, ""), &peers);
        assert!(matches!(w.notice.borrow().clone(), Some(FilesNotice::Error(_))));

        let bundle = crate::apps::saves::SaveBundle {
            set: paths::GAMES_SET.into(),
            id: "chess".into(),
            app_name: "Chess".into(),
            state: "{\"board\":\"e4\"}".into(),
            saved_at_ms: 1,
        };
        *w.inbox.borrow_mut() = Some((bundle.file_name(), bundle.to_bytes()));
        w.handle_action(&event(IMPORT_EVENT, ""), &peers);
        assert!(matches!(w.notice.borrow().clone(), Some(FilesNotice::Info(_))));
        let live = peers.get_entity(&pid, &app_paths::app_save_path(APP_ID, &pid, paths::GAMES_SET, "chess"));
        assert_eq!(live.map(|e| AppSave::from_entity(&e).state), Some("{\"board\":\"e4\"}".into()));
        assert!(w.inbox.borrow().is_none(), "the inbox is emptied");
    }

    /// The archive plan names every file in a place, where its bytes are, and
    /// a path no two apps' files share.
    #[tokio::test]
    async fn an_export_plan_names_every_file_in_the_place_with_distinct_paths() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        peers.seed_write(&pid, app_paths::app_save_path(APP_ID, &pid, paths::GAMES_SET, "chess"), AppSave::new("{\"a\":1}").to_entity());
        peers.seed_write(&pid, app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 5_000), AppSave::new("old").to_entity());
        let w = FilesWindow::new(pid.clone());
        let plan = w.export_plan(&peers, Place::Saves);
        assert_eq!(
            plan,
            vec![
                ("games-chess.json".to_string(), 0, ExportSource::Inline(b"{\"a\":1}".to_vec())),
                ("backups/games-chess-5000.json".to_string(), 5, ExportSource::Inline(b"old".to_vec())),
            ]
        );
        assert!(w.export_plan(&peers, Place::MyFiles).is_empty());
    }

    #[tokio::test]
    async fn places_filter_the_rows_and_a_backup_is_removable_but_a_live_save_is_not() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        peers.seed_write(&pid, app_paths::app_save_path(APP_ID, &pid, paths::GAMES_SET, "chess"), AppSave::new("live").to_entity());
        peers.seed_write(&pid, app_paths::app_backup_path(APP_ID, &pid, paths::GAMES_SET, "chess", 5), AppSave::new("old").to_entity());
        let mut w = FilesWindow::new(pid.clone());
        assert_eq!(w.output(&peers).place, Place::Saves, "opens on the first place that holds a file");
        assert_eq!(FilesWindow::new(pid.clone()).output(&Peers::new_direct()).place, Place::MyFiles,
            "an empty device opens on My files");

        w.handle_action(&event(PLACE_EVENT, Place::MyFiles.key()), &peers);
        assert!(w.output(&peers).rows.is_empty(), "a chosen place sticks, empty or not");
        w.handle_action(&event(PLACE_EVENT, Place::Saves.key()), &peers);
        let out = w.output(&peers);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.counts.iter().find(|(p, _)| *p == Place::Saves).map(|c| c.1), Some(2));

        // A hand-made remove of the live save is ignored, not obeyed.
        w.handle_action(&event(REMOVE_EVENT, &FileRef::Save { set: paths::GAMES_SET.into(), id: "chess".into() }.encode()), &peers);
        w.handle_action(&event(REMOVE_EVENT, &FileRef::Backup { set: paths::GAMES_SET.into(), id: "chess".into(), stamp: 5 }.encode()), &peers);
        let out = w.output(&peers);
        assert_eq!(out.rows.len(), 1, "{:?}", out.rows);
        assert!(matches!(out.rows[0].file, FileRef::Save { .. }), "the live save stays");
    }
}
