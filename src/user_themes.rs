//! User-defined themes — tree persistence + the registry sync.
//!
//! Durable record: one entity per theme at
//! `/{system}/app/entity-browser/themes/{name}` (`app_paths::user_theme_path`),
//! written L1 (`dispatch_write`). The runtime registry in
//! [`crate::theme_tokens`] is a rebuildable **projection** of this prefix —
//! never a second source of truth (DESIGN-USER-THEMES §2).
//!
//! Reactivity: the app holds ONE [`UserThemes`] for its lifetime. Its
//! `WindowWatch` subscribes the themes prefix at construction — on the
//! Worker arm that subscription is also what feeds the cache mirror, so the
//! boot listing is readable at all (the worker-cache rule). [`UserThemes::sync`]
//! runs each frame but is a single atomic-bool check when nothing changed;
//! on a real change it reconciles the registry (register new/changed,
//! unregister deleted, warn-and-skip malformed — never panic in frame) and
//! re-installs the live appearance surfaces via
//! [`crate::theme_tokens::reinstall_current`] (an edited theme recolors
//! live; a stale boot paint-hint mirror self-heals).

use entity_entity::Entity;

use crate::app_paths;
use crate::peers::Peers;
use crate::theme_tokens::{self, UserThemeSpec};
use crate::window_watch::WindowWatch;

/// Entity type for a persisted user theme.
pub const USER_THEME_TYPE: &str = "app/state/theme";

/// Encode a theme spec as its tree entity. `vars` is a CBOR **array** of
/// `[token, value]` pairs — an array, not a map, because ECF canonicalizes
/// map keys (sorts them) and the token order carries the editor's group
/// structure (surfaces / text / borders / …) through the round-trip.
pub fn spec_to_entity(spec: &UserThemeSpec) -> Entity {
    let vars: Vec<ciborium::Value> = spec
        .vars
        .iter()
        .map(|(k, v)| {
            ciborium::Value::Array(vec![
                ciborium::Value::Text(k.clone()),
                ciborium::Value::Text(v.clone()),
            ])
        })
        .collect();
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "name" => entity_ecf::text(&spec.name),
        "label" => entity_ecf::text(&spec.label),
        "scheme" => entity_ecf::text(&spec.scheme),
        "vars" => ciborium::Value::Array(vars)
    });
    Entity::new(USER_THEME_TYPE, data).unwrap()
}

/// Decode a persisted theme entity. `None` (caller warns) for anything
/// malformed — wrong shape, missing fields, non-text vars.
pub fn spec_from_entity(entity: &Entity) -> Option<UserThemeSpec> {
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    let mut spec = UserThemeSpec {
        name: String::new(),
        label: String::new(),
        scheme: String::new(),
        vars: Vec::new(),
    };
    for (k, v) in map {
        match k.as_text() {
            Some("name") => spec.name = v.as_text()?.to_string(),
            Some("label") => spec.label = v.as_text()?.to_string(),
            Some("scheme") => spec.scheme = v.as_text()?.to_string(),
            Some("vars") => {
                for pair in v.as_array()? {
                    let pair = pair.as_array()?;
                    let [vk, vv] = pair.as_slice() else {
                        return None;
                    };
                    spec.vars.push((vk.as_text()?.to_string(), vv.as_text()?.to_string()));
                }
            }
            _ => {}
        }
    }
    (!spec.name.is_empty() && !spec.vars.is_empty()).then_some(spec)
}

/// Save (create or overwrite) a user theme: validate + register into the
/// runtime registry NOW (instant dropdowns/preview), then write the durable
/// entity. The subscription round-trip re-syncs idempotently (the sync's
/// same-content compare skips a re-leak).
pub fn save_theme(peers: &Peers, spec: UserThemeSpec) -> Result<(), String> {
    let entity = spec_to_entity(&spec);
    let name = spec.name.clone();
    theme_tokens::register_user_theme(spec)?;
    let pid = peers.system_peer_id().to_string();
    peers.dispatch_write(&pid, app_paths::user_theme_path(app_paths::APP_ID, &pid, &name), entity);
    // If the saved theme IS the current chrome theme / site override, this
    // recolors the live page and refreshes the boot paint-hint mirror NOW —
    // the subscription round-trip won't (its same-content compare sees the
    // registration we just made and skips).
    theme_tokens::reinstall_current();
    Ok(())
}

/// Delete a user theme. Refused (with the reason, D13) while it is the
/// current chrome theme or the site-appearance strict override — the
/// settings that reference it live in `SettingsState`, read here from the
/// tree so the check works on both arms and in native tests.
pub fn delete_theme(peers: &Peers, name: &str) -> Result<(), String> {
    let pid = peers.system_peer_id().to_string();
    let settings_path = app_paths::settings_path(
        app_paths::APP_ID,
        &pid,
        crate::views::settings::model::SETTINGS_PATH,
    );
    if let Some(entity) = peers.get_entity(&pid, &settings_path) {
        let state = crate::views::settings::model::SettingsState::from_entity(&entity);
        if state.theme == name {
            return Err(format!("\"{name}\" is the current theme — switch themes first"));
        }
        if state.site_appearance == name {
            return Err(format!(
                "\"{name}\" is the site-appearance override — change Site appearance first"
            ));
        }
    }
    theme_tokens::unregister_user_theme(name);
    peers.dispatch_remove(&pid, app_paths::user_theme_path(app_paths::APP_ID, &pid, name));
    Ok(())
}

/// App-held sync state: the themes-prefix watch. One instance for the app
/// lifetime (the watch's subscriptions live as long as it does — D9: that
/// lifetime is the design, not a leak).
pub struct UserThemes {
    watch: WindowWatch,
}

impl UserThemes {
    /// Subscribe the themes prefix on the system peer. The watch starts
    /// dirty, so the first frame's [`sync`](Self::sync) performs the boot
    /// load (Worker arm: the listing fills in as the observe seeds the
    /// cache mirror; each seed event re-dirties, so it converges).
    pub fn new(peers: &Peers) -> Self {
        let mut watch = WindowWatch::new();
        let pid = peers.system_peer_id().to_string();
        peers.watch_prefix(&mut watch, &pid, app_paths::user_themes_prefix(app_paths::APP_ID, &pid));
        Self { watch }
    }

    /// Reconcile the runtime registry from the tree. Cheap no-op (one
    /// atomic check) unless the themes prefix changed since the last call.
    pub fn sync(&mut self, peers: &Peers) {
        if !self.watch.take_dirty() {
            return;
        }
        let pid = peers.system_peer_id().to_string();
        let prefix = app_paths::user_themes_prefix(app_paths::APP_ID, &pid);

        let mut tree_names: Vec<String> = Vec::new();
        let mut registered_n = 0usize;
        for entry in peers.tree_listing(&pid, &prefix) {
            let Some(name) = entry.path.strip_prefix(&prefix).filter(|n| !n.contains('/')) else {
                continue;
            };
            let Some(entity) = peers.get_entity(&pid, &entry.path) else {
                continue;
            };
            let Some(spec) = spec_from_entity(&entity) else {
                tracing::warn!(path = %entry.path, "user theme entity is malformed — skipped");
                continue;
            };
            if spec.name != name {
                tracing::warn!(path = %entry.path, name = %spec.name, "user theme name/path mismatch — skipped");
                continue;
            }
            tree_names.push(name.to_string());
            if theme_is_current(&spec) {
                continue; // identical to the registered one — skip the re-leak.
            }
            match theme_tokens::register_user_theme(spec) {
                Ok(()) => registered_n += 1,
                Err(reason) => {
                    tracing::warn!(name = %name, %reason, "persisted user theme rejected — skipped")
                }
            }
        }

        let mut removed_n = 0usize;
        for name in theme_tokens::user_theme_names() {
            if !tree_names.iter().any(|n| n == name) {
                theme_tokens::unregister_user_theme(name);
                removed_n += 1;
            }
        }

        if registered_n + removed_n > 0 {
            tracing::info!(
                registered = registered_n,
                removed = removed_n,
                total = theme_tokens::user_theme_names().len(),
                "user themes synced from tree"
            );
            // Re-derive the live appearance surfaces against the new
            // registry: recolors an edited current theme, self-heals the
            // boot paint-hint mirrors, drops a deleted theme to dark.
            theme_tokens::reinstall_current();
        }
    }
}

/// Is `spec` value-identical to the currently registered theme of the same
/// name? Guards the per-sync re-leak (the sync re-reads EVERY entity under
/// the prefix on any prefix event).
fn theme_is_current(spec: &UserThemeSpec) -> bool {
    let Some(t) = theme_tokens::registered(&spec.name) else {
        return false;
    };
    t.label == spec.label
        && t.scheme == spec.scheme
        && t.vars.len() == spec.vars.len()
        && t.vars.iter().zip(&spec.vars).all(|((k1, v1), (k2, v2))| k1 == k2 && v1 == v2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> UserThemeSpec {
        UserThemeSpec {
            name: name.into(),
            label: format!("Theme {name}"),
            scheme: "light".into(),
            vars: crate::theme_tokens::DARK
                .vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    async fn flush() {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    #[test]
    fn spec_round_trips_through_entity() {
        let s = spec("mine");
        let e = spec_to_entity(&s);
        assert_eq!(e.entity_type, USER_THEME_TYPE);
        assert_eq!(spec_from_entity(&e), Some(s));
    }

    #[test]
    fn malformed_entity_decodes_to_none() {
        let junk = Entity::new(USER_THEME_TYPE, b"not cbor".to_vec()).unwrap();
        assert_eq!(spec_from_entity(&junk), None);
        // Structurally valid CBOR but no vars → rejected.
        let empty = UserThemeSpec {
            name: "x".into(),
            label: "X".into(),
            scheme: "dark".into(),
            vars: Vec::new(),
        };
        assert_eq!(spec_from_entity(&spec_to_entity(&empty)), None);
    }

    #[tokio::test]
    async fn save_registers_and_persists_then_sync_is_stable() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let mut ut = UserThemes::new(&pm);

        save_theme(&pm, spec("ocean")).expect("saves");
        assert!(theme_tokens::registered("ocean").is_some(), "registered immediately");
        flush().await;

        let path = app_paths::user_theme_path(app_paths::APP_ID, &pid, "ocean");
        let entity = pm.get_entity(&pid, &path).expect("persisted");
        assert_eq!(spec_from_entity(&entity).unwrap().name, "ocean");

        // The subscription round-trip re-syncs without churn (same-content
        // compare) and keeps the theme registered.
        ut.sync(&pm);
        assert!(theme_tokens::registered("ocean").is_some());
        theme_tokens::unregister_user_theme("ocean");
    }

    #[tokio::test]
    async fn sync_loads_persisted_theme_at_boot_and_drops_deleted() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        // Warm-boot shape: the entity is already in the tree BEFORE the app
        // (and its registry) exists.
        let path = app_paths::user_theme_path(app_paths::APP_ID, &pid, "boots");
        pm.seed_write(&pid, path.clone(), spec_to_entity(&spec("boots")));

        let mut ut = UserThemes::new(&pm);
        ut.sync(&pm); // watch starts dirty → boot load
        assert!(theme_tokens::registered("boots").is_some(), "loaded from tree at boot");

        // Delete the entity → next dirty sync unregisters it.
        pm.dispatch_remove(&pid, path);
        flush().await;
        ut.sync(&pm);
        assert!(theme_tokens::registered("boots").is_none(), "deleted theme dropped");
    }

    #[tokio::test]
    async fn sync_skips_malformed_and_mismatched_entities() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let prefix = app_paths::user_themes_prefix(app_paths::APP_ID, &pid);
        pm.seed_write(
            &pid,
            format!("{prefix}junk"),
            Entity::new(USER_THEME_TYPE, b"junk".to_vec()).unwrap(),
        );
        // Path leaf says "renamed", payload says "other" → skipped.
        pm.seed_write(&pid, format!("{prefix}renamed"), spec_to_entity(&spec("other")));

        let mut ut = UserThemes::new(&pm);
        ut.sync(&pm);
        assert!(theme_tokens::registered("junk").is_none());
        assert!(theme_tokens::registered("other").is_none());
        assert!(theme_tokens::registered("renamed").is_none());
    }

    #[tokio::test]
    async fn delete_refuses_while_in_use_then_succeeds() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        save_theme(&pm, spec("indigo")).unwrap();
        flush().await;

        // Make it the current chrome theme via the settings model.
        let model = crate::views::settings::model::SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);
        model.set_theme("indigo", &pm);
        flush().await;

        let err = delete_theme(&pm, "indigo").expect_err("refused while current theme");
        assert!(err.contains("current theme"), "err: {err}");
        assert!(theme_tokens::registered("indigo").is_some(), "still registered");

        model.set_theme("dark", &pm);
        flush().await;
        delete_theme(&pm, "indigo").expect("deletes once unused");
        flush().await;
        assert!(theme_tokens::registered("indigo").is_none());
        let path = app_paths::user_theme_path(app_paths::APP_ID, &pid, "indigo");
        assert!(pm.get_entity(&pid, &path).is_none(), "entity removed");
    }
}
