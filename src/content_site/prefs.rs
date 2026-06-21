//! Site-cache **preferences** — the app-tier half of the field-split
//! (Category B).
//!
//! ## The field-split (the third record)
//!
//! When this peer browses a site — owned or cached — three different records
//! can appear, with three different owners:
//!
//! - **Foreign authored content** (Category A) — the manifest + page entities,
//!   hash-verified — at `/{peer}/sites/{S}/...` ([`super::paths`]).
//! - **SDK-tier provenance** (Category B, substrate) — `last_reconciled` /
//!   `pinned_root_hash` / `source_transport` — at `/{me}/system/cache/...`
//!   ([`super::cache`]). Uniform across every L5 app; never synced.
//! - **App-tier preferences** (Category B, app) — *this module* — the browser's
//!   own bookkeeping about a site: visit count, bookmarked, is-home, kept
//!   offline, last-viewed page. Genuinely Content-Site's, so it lives in the
//!   app namespace at [`crate::app_paths::site_cache_prefs_path`]
//!   (`/{me}/app/entity-browser/site-cache/{peer}/sites/{S}/prefs`).
//!
//! Preferences are keyed by `(site_peer, site)` exactly like provenance, where
//! `site_peer` may be **my own id** (prefs on a site I authored — `is_home`
//! applies to owned sites too) or a **foreign id** (prefs on a cached site).
//! The keying is uniform; nothing here distinguishes owned from cached (the
//! path's peer-segment already carries that — V7 §1.4, derived at read time).
//!
//! ## What `keep_offline` gates (O3 manifest-pinned mode)
//!
//! `keep_offline` is the per-site "keep this site" promotion of the O3
//! manifest-pinned browse mode (§5): the resolver always durably persists a
//! visited site's **manifest** (so it stays enumerable + navigable), but only
//! writes **page bodies** through to the durable tree when the site is kept.
//! This module owns the flag; the resolver reads it (`persist_to_cache`).

use entity_entity::Entity;

use crate::app_paths::{self, APP_ID};
use crate::peers::Peers;

/// Entity type for an app-tier site preferences record.
const PREFS_TYPE: &str = "app/state/site_cache_prefs";

/// App-tier preferences for one browsed site (owned or cached). All fields
/// default to "nothing recorded yet" so an absent record reads as
/// [`SitePrefs::default`] — a never-visited site.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SitePrefs {
    /// How many times this surface has opened the site (a soft recency signal).
    pub visit_count: u64,
    /// The user bookmarked this site (pin it in the site list).
    pub bookmarked: bool,
    /// This site is the user's chosen home — a UI marker (the durable boot
    /// home target is still `session_config::home_site`; this is the per-site
    /// "star as home" affordance the site list shows).
    pub is_home: bool,
    /// "Keep this site" — promote from manifest-pinned (default) to full
    /// offline caching, so the resolver writes page bodies through to the
    /// durable tree (O3, §5). Off = manifest persists, pages re-fetch on demand.
    pub keep_offline: bool,
    /// Last page slug viewed in this site (UI restore hint). Empty = the root.
    pub last_viewed_page: String,
}

impl SitePrefs {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let Some(map) = value.as_map() else {
            return Self::default();
        };
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("visit_count") => {
                    if let Some(i) = v.as_integer() {
                        out.visit_count = u64::try_from(i128::from(i)).unwrap_or(0);
                    }
                }
                Some("bookmarked") => {
                    if let Some(b) = v.as_bool() {
                        out.bookmarked = b;
                    }
                }
                Some("is_home") => {
                    if let Some(b) = v.as_bool() {
                        out.is_home = b;
                    }
                }
                Some("keep_offline") => {
                    if let Some(b) = v.as_bool() {
                        out.keep_offline = b;
                    }
                }
                Some("last_viewed_page") => {
                    if let Some(s) = v.as_text() {
                        out.last_viewed_page = s.to_string();
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "visit_count" => entity_ecf::integer(self.visit_count as i64),
            "bookmarked" => entity_ecf::bool_val(self.bookmarked),
            "is_home" => entity_ecf::bool_val(self.is_home),
            "keep_offline" => entity_ecf::bool_val(self.keep_offline),
            "last_viewed_page" => entity_ecf::text(&self.last_viewed_page)
        });
        Entity::new(PREFS_TYPE, data).unwrap()
    }
}

/// Read the preferences for `(site_peer, site)` from MY store, or
/// [`SitePrefs::default`] when none have been recorded. Selector = my peer
/// (the prefs ledger is under my namespace). On the Worker arm the reader must
/// observe [`app_paths::site_cache_prefix`] for this to reflect writes.
pub fn read_prefs(peers: &Peers, my_peer_id: &str, site_peer_id: &str, site_id: &str) -> SitePrefs {
    let path = app_paths::site_cache_prefs_path(APP_ID, my_peer_id, site_peer_id, site_id);
    peers
        .get_entity(my_peer_id, &path)
        .map(|e| SitePrefs::from_entity(&e))
        .unwrap_or_default()
}

/// Write the preferences for `(site_peer, site)` to MY store via the arm-aware
/// [`Peers::seed_write`] (Direct → sync L0; Worker → `dispatch_write`). The
/// record lives under my own namespace, so no cross-namespace capability is
/// involved (contrast the Category-A content write).
pub fn write_prefs(
    peers: &Peers,
    my_peer_id: &str,
    site_peer_id: &str,
    site_id: &str,
    prefs: &SitePrefs,
) {
    let path = app_paths::site_cache_prefs_path(APP_ID, my_peer_id, site_peer_id, site_id);
    peers.seed_write(my_peer_id, path, prefs.to_entity());
}

/// Read-modify-write a single site's preferences. Reads the current record (or
/// default), applies `mutate`, writes it back. Used by the site-list toggles
/// (bookmark / set-home / keep-offline) — a small, deterministic update with no
/// lost-update concern for the single-window UI that drives it.
pub fn update_prefs(
    peers: &Peers,
    my_peer_id: &str,
    site_peer_id: &str,
    site_id: &str,
    mutate: impl FnOnce(&mut SitePrefs),
) {
    let mut prefs = read_prefs(peers, my_peer_id, site_peer_id, site_id);
    mutate(&mut prefs);
    write_prefs(peers, my_peer_id, site_peer_id, site_id, &prefs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_through_entity() {
        let p = SitePrefs {
            visit_count: 7,
            bookmarked: true,
            is_home: false,
            keep_offline: true,
            last_viewed_page: "guide/intro".into(),
        };
        assert_eq!(SitePrefs::from_entity(&p.to_entity()), p);
    }

    #[test]
    fn default_prefs_round_trip() {
        let p = SitePrefs::default();
        assert_eq!(SitePrefs::from_entity(&p.to_entity()), p);
        // A never-visited site reads as default (every field "nothing yet").
        assert_eq!(p.visit_count, 0);
        assert!(!p.bookmarked && !p.is_home && !p.keep_offline);
        assert!(p.last_viewed_page.is_empty());
    }

    #[test]
    fn absent_record_reads_as_default() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert_eq!(read_prefs(&peers, &me, "BOB", "blog"), SitePrefs::default());
    }

    #[test]
    fn write_then_read_round_trips_through_my_store() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let p = SitePrefs { visit_count: 3, bookmarked: true, ..Default::default() };
        write_prefs(&peers, &me, "BOB", "blog", &p);
        assert_eq!(read_prefs(&peers, &me, "BOB", "blog"), p);
    }

    #[test]
    fn update_prefs_read_modify_writes() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        // First toggle bookmarks; a second toggle, on the persisted record,
        // bumps visit_count — proving the read-modify-write sees prior state.
        update_prefs(&peers, &me, "BOB", "blog", |p| p.bookmarked = true);
        update_prefs(&peers, &me, "BOB", "blog", |p| p.visit_count += 1);
        let got = read_prefs(&peers, &me, "BOB", "blog");
        assert!(got.bookmarked, "first update persisted across the second");
        assert_eq!(got.visit_count, 1);
    }

    #[test]
    fn owned_site_prefs_key_by_my_own_id() {
        // is_home applies to an owned site too — keyed by my id, same shape.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        update_prefs(&peers, &me, &me, "demo", |p| p.is_home = true);
        assert!(read_prefs(&peers, &me, &me, "demo").is_home);
    }
}
