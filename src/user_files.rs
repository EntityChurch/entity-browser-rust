//! `user_files` — **My files**: a file a person brought onto this device, kept
//! privately (`DESIGN-2026-09-14-b` §4, Files F2).
//!
//! Until this existed there was nowhere private to put a file from the device:
//! the only way in was *Offer a file*, which is the sharing act. A file added
//! from the device's picker, dropped onto the File Manager, or pulled from a
//! peer with *Keep in My files* lands here instead, and sharing it stays a
//! separate, deliberate *Offer to peers*.
//!
//! The same shape as an app's kept file ([`crate::kept_files`]), minus the app:
//!
//! - **bytes** in our own `system/content`, namespace [`NAMESPACE`] — which
//!   records the act that put them there and is **not** an access boundary
//!   (`system/content:get` serves by hash);
//! - **a manifest** of type [`USER_FILE_TYPE`] at
//!   `app/entity-browser/files/{blob-hex}` — the offer manifest's body with no
//!   `source`, under a type no offer reader decodes, so a private file can
//!   never be read as an offer.
//!
//! "Private" means what it means for kept files, and the File Manager's footer
//! says so: not *listed* to other devices, not *sealed* while this build runs
//! with open grants.

use crate::app_paths::{user_file_path, user_files_prefix, APP_ID};
use crate::dispatch_handle::DispatchHandle;
use crate::file_offer::{self, FileOffer, MAX_OFFER_BYTES};
use entity_hash::Hash;

/// Entity type of a My files manifest.
pub const USER_FILE_TYPE: &str = "app/entity-browser/user-file";

/// The `system/content` namespace My files' bytes are ingested into.
pub const NAMESPACE: &str = "user-files";

/// Which private store a file's bytes were ingested into — what a save or an
/// offer of that file reads back from. An offer is not a private store and is
/// not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateStore {
    /// A file an app handed the host ([`crate::kept_files`]).
    AppFiles,
    /// A file a person brought onto the device (this module).
    MyFiles,
}

impl PrivateStore {
    pub fn namespace(self) -> &'static str {
        match self {
            PrivateStore::AppFiles => crate::kept_files::NAMESPACE,
            PrivateStore::MyFiles => NAMESPACE,
        }
    }
}

/// Keep `raw` in My files under `name`. Refuses over the offer ceiling (one
/// number, [`MAX_OFFER_BYTES`]) before allocating anything in the store.
pub async fn keep(dispatch: &DispatchHandle, name: &str, raw: &[u8]) -> Result<FileOffer, String> {
    if raw.len() as u64 > MAX_OFFER_BYTES {
        return Err(file_offer::too_large_message(name, raw.len() as u64));
    }
    let name = clean_name(name);
    let local_pid = dispatch.local_peer_id();
    let blob = file_offer::ingest_into(dispatch, NAMESPACE, raw).await?;
    let file = FileOffer {
        name,
        size: raw.len() as u64,
        blob: blob.content_hash,
        from: local_pid.clone(),
        source: None,
    };
    dispatch
        .put(user_file_path(APP_ID, &local_pid, &file.id()), file_offer::manifest_entity_as(&file, USER_FILE_TYPE)?)
        .await?;
    Ok(file)
}

/// The bytes of one of My files. Local: no peer, no connection.
pub async fn read_bytes(dispatch: &DispatchHandle, blob: &Hash) -> Result<Vec<u8>, String> {
    file_offer::read_own_in(dispatch, NAMESPACE, blob).await
}

/// The name a file is listed under: the last path segment a browser or a peer
/// supplied, never a path. Empty becomes a name a person can still recognise.
pub fn clean_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if last.is_empty() {
        "file".to_string() // i18n-ignore — a file name, not prose
    } else {
        last.to_string()
    }
}

/// Every file in My files, from our own tree, sorted by name then blob so rows
/// do not reshuffle. The caller subscribes [`user_files_prefix`] on the Worker
/// arm.
pub fn read_mine(peers: &crate::peers::Peers, peer_id: &str) -> Vec<FileOffer> {
    let prefix = user_files_prefix(APP_ID, peer_id);
    let mut out: Vec<FileOffer> = peers
        .tree_listing(peer_id, &prefix)
        .into_iter()
        .filter_map(|entry| {
            let id = entry.path.strip_prefix(&prefix)?;
            if id.is_empty() || id.contains('/') {
                return None;
            }
            file_offer::decode_manifest_as(&peers.get_entity(peer_id, &entry.path)?, USER_FILE_TYPE)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.blob.to_hex().cmp(&b.blob.to_hex())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_last_segment_and_never_empty() {
        assert_eq!(clean_name("photo.jpg"), "photo.jpg");
        assert_eq!(clean_name("C:\\Users\\me\\notes.txt"), "notes.txt");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("  "), "file");
        assert_eq!(clean_name("dir/"), "file");
    }

    #[test]
    fn each_store_reads_back_from_its_own_namespace() {
        assert_eq!(PrivateStore::AppFiles.namespace(), crate::kept_files::NAMESPACE);
        assert_eq!(PrivateStore::MyFiles.namespace(), NAMESPACE);
        assert_ne!(NAMESPACE, file_offer::NAMESPACE, "My files are not ingested where offers are");
    }
}
