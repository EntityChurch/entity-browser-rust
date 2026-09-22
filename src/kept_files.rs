//! `kept_files` — a file an app handed the host, kept **privately**.
//!
//! An entity-app hands the host a file with `x-file` (`crate::app_files`): a
//! program the person ran inside the app produced it, or they moved it out of a
//! machine's floppy or home directory. Until 2026-09-14 the host made that file a
//! [`crate::file_offer`] — a manifest under `app/entity-browser/offers/`, the
//! prefix connected peers list to see what they may pull. **Pulling a file out of
//! an app shared it**, and the person reporting it named the cost exactly: "I may
//! have private information I'm working on… and suddenly I'm accidentally
//! offering it."
//!
//! So a kept file is now:
//!
//! - **bytes** in our own `system/content`, in the [`NAMESPACE`] namespace — not
//!   the `files` namespace offers are ingested into. That records which act put
//!   them there; it is **not** an access boundary today (`system/content:get`
//!   serves by hash without consulting the namespace binding — see the
//!   `file_offer` reclaim test in `peers.rs`);
//! - **a manifest** of type [`KEPT_TYPE`] at
//!   `app/entity-browser/apps/{set}/files/{app}/{blob-hex}` — under the app's own
//!   directory, beside its workspace, where the entity tree shows it as the app's
//!   files. Same body as an offer manifest (name, size, blob, from, app), a
//!   different type, so a private file can never decode as an offer.
//!
//! Sharing one is a separate, deliberate act: File Transfer lists kept files and
//! offers **"Offer to peers"**, which runs the ordinary offer path on the bytes.
//!
//! **What "private" means here, stated so nobody reads it wider:** the file is
//! not *advertised* — no peer can discover it by listing our offers. It is not
//! *sealed*: this build still runs with `debug_open_grants` (see `share.rs`),
//! under which a connected peer can read any path in our tree if it knows where
//! to look, and anyone holding a content hash can `get` its bytes. When grant
//! enforcement lands, what a share grants is the offers prefix, and the app files
//! prefix is not in it.

use crate::app_paths::{app_file_path, app_files_prefix, offer_path, APP_ID};
use crate::dispatch_handle::DispatchHandle;
use crate::file_offer::{self, FileOffer, OfferSource, MAX_OFFER_BYTES};
use entity_hash::Hash;

/// Entity type of a kept file's manifest — our namespace, not an offer's type.
pub const KEPT_TYPE: &str = "app/entity-browser/app-file";

/// The `system/content` namespace a kept file's bytes are ingested into.
pub const NAMESPACE: &str = "app-files";

/// Keep `raw`, which the app `source` handed over under `name`. Refuses over the
/// same ceiling an offer has (one number, [`MAX_OFFER_BYTES`]) before allocating.
pub async fn keep(
    dispatch: &DispatchHandle,
    name: &str,
    raw: &[u8],
    source: OfferSource,
) -> Result<FileOffer, String> {
    if raw.len() as u64 > MAX_OFFER_BYTES {
        return Err(file_offer::too_large_message(name, raw.len() as u64));
    }
    let local_pid = dispatch.local_peer_id();
    let blob = file_offer::ingest_into(dispatch, NAMESPACE, raw).await?;
    let file = FileOffer {
        name: name.to_string(),
        size: raw.len() as u64,
        blob: blob.content_hash,
        from: local_pid.clone(),
        source: Some(source),
    };
    write_manifest(dispatch, &file).await?;
    Ok(file)
}

async fn write_manifest(dispatch: &DispatchHandle, file: &FileOffer) -> Result<(), String> {
    let src = file.source.as_ref().ok_or("a kept file must name the app it came from")?;
    dispatch
        .put(
            app_file_path(APP_ID, &file.from, &src.set, &src.app, &file.id()),
            file_offer::manifest_entity_as(file, KEPT_TYPE)?,
        )
        .await
}

/// The bytes of one of our kept files. Local: no peer, no connection.
pub async fn read_bytes(dispatch: &DispatchHandle, blob: &Hash) -> Result<Vec<u8>, String> {
    file_offer::read_own_in(dispatch, NAMESPACE, blob).await
}

/// Every file apps in any set have kept, from our own tree. The caller must
/// subscribe [`app_files_prefix`] for each set on the Worker arm. Sorted by app,
/// then name, so rows do not reshuffle as files arrive.
pub fn read_kept(peers: &crate::peers::Peers, peer_id: &str) -> Vec<FileOffer> {
    let mut out = Vec::new();
    for set in crate::apps::paths::APP_SETS {
        let prefix = app_files_prefix(APP_ID, peer_id, set);
        for entry in peers.tree_listing(peer_id, &prefix) {
            let Some(rest) = entry.path.strip_prefix(&prefix) else { continue };
            // exactly `{app}/{blob-hex}`
            if rest.split('/').count() != 2 || rest.ends_with('/') {
                continue;
            }
            if let Some(file) = peers
                .get_entity(peer_id, &entry.path)
                .and_then(|e| file_offer::decode_manifest_as(&e, KEPT_TYPE))
            {
                out.push(file);
            }
        }
    }
    out.sort_by(|a, b| {
        let app = |f: &FileOffer| f.source.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        app(a).cmp(&app(b)).then(a.name.cmp(&b.name)).then(a.blob.to_hex().cmp(&b.blob.to_hex()))
    });
    out
}

/// Take back what an earlier build shared on the person's behalf: every OFFER
/// that records an app as its source is made a kept file, then its offer manifest
/// is removed. Returns how many were moved.
///
/// Order matters, and it is the safe one: the kept copy is written and its bytes
/// re-read **before** the offer is removed, so a failure part-way leaves the file
/// still offered (visible, and retried next boot) rather than lost. An offer a
/// person made by hand has no source and is never touched — which is why
/// File Transfer's deliberate "Offer to peers" on a kept file offers it WITHOUT a
/// source: an offer naming an app is, by construction, one nobody chose to make.
pub async fn take_back_app_offers(
    dispatch: &DispatchHandle,
    writer: &crate::writer_handle::WriterHandle,
    offers: Vec<FileOffer>,
) -> Result<usize, String> {
    let mut moved = 0;
    for offer in offers {
        let Some(source) = offer.source.clone() else { continue };
        let pid = dispatch.local_peer_id();
        let raw = file_offer::read_own_offer(dispatch, &offer.blob).await?;
        let kept = keep(dispatch, &offer.name, &raw, source).await?;
        if read_bytes(dispatch, &kept.blob).await? != raw {
            return Err(format!("the private copy of {} did not read back identically; left it offered", offer.name));
        }
        writer.remove(offer_path(APP_ID, &pid, &offer.id()));
        moved += 1;
    }
    Ok(moved)
}

/// The per-frame reconciler that runs [`take_back_app_offers`]: watches our
/// offers prefix and, whenever it holds an offer that names an app, moves it to a
/// kept file. A reconcile rather than a one-shot boot step for the reason
/// `share::ShareSync` gives — the Worker mirror seeds asynchronously, so "no app
/// offers" on the first frame is not an answer — and because it also catches any
/// future path that writes one.
pub struct TakeBack {
    watch: crate::window_watch::WindowWatch,
    peer_id: String,
    busy: std::rc::Rc<std::cell::Cell<bool>>,
}

impl TakeBack {
    pub fn new(peers: &crate::peers::Peers, peer_id: &str) -> Self {
        let mut watch = crate::window_watch::WindowWatch::new();
        peers.watch_prefix(&mut watch, peer_id, crate::app_paths::offers_prefix(APP_ID, peer_id));
        Self { watch, peer_id: peer_id.to_string(), busy: Default::default() }
    }

    /// One atomic check unless the offers prefix changed.
    pub fn sync(&mut self, peers: &crate::peers::Peers) {
        if self.busy.get() || !self.watch.take_dirty() {
            return;
        }
        let offers: Vec<FileOffer> = file_offer::read_own_offers(peers, &self.peer_id)
            .into_iter()
            .filter(|o| o.source.is_some())
            .collect();
        if offers.is_empty() {
            return;
        }
        let (Some(dispatch), Some(writer)) =
            (peers.dispatch_handle(&self.peer_id), peers.writer_handle_for(&self.peer_id))
        else {
            return;
        };
        let names: Vec<String> = offers.iter().map(|o| o.name.clone()).collect();
        tracing::warn!(peer = %self.peer_id, files = ?names,
            "kept files: an app's file was OFFERED to peers (an earlier build did this) — taking it back to private");
        #[cfg(target_arch = "wasm32")]
        {
            let busy = self.busy.clone();
            let flag = self.watch.flag();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match take_back_app_offers(&dispatch, &writer, offers).await {
                    Ok(n) => tracing::info!(moved = n, "kept files: app files taken back from the offers list"),
                    Err(e) => tracing::warn!(error = %e, "kept files: could not take an app file back; it stays offered and is retried"),
                }
                busy.set(false);
                flag.mark();
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (dispatch, writer, offers);
    }
}
