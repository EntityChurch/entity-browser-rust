//! Ingest an `entity-apps` `dist/` directory into the embedded-app formats.
//!
//! Mirrors `content_site::ingest`: a pure disk→entities reader (no peer/tree
//! coupling) so it's native-testable and reusable by both the publish pipeline
//! (disk→`.bin`) and an in-app importer. The `dist/` shape is:
//! ```text
//! dist/index.json     [{ id, name, description, saves, type }, …]  (the catalog)
//! dist/<id>.html      one self-contained bundle per entry
//! ```
//!
//! Entries are **split into app-sets by their `type`** ([`paths::set_for_type`]):
//! `canvas-game` → the `games` set, everything else (tool, calendar, …) → the
//! `apps` set. A single `dist/` therefore yields one [`IngestedApps`] per set.

use std::collections::BTreeMap;
use std::path::Path;

use crate::apps::format::{AppBundle, AppCatalog, AppEntry, AppSize};
use crate::apps::paths;
use crate::peers::Peers;

/// Parse a catalog `size` value (`{ "width": N|null, "height": N|null }`) into
/// an [`AppSize`]. A missing/null/non-positive axis → `None` ("fill that axis").
/// `size` absent (or not an object) → `None` (the host's per-set default).
fn parse_size(v: Option<&serde_json::Value>) -> Option<AppSize> {
    let obj = v?.as_object()?;
    let axis = |k: &str| {
        obj.get(k)
            .and_then(|x| x.as_u64())
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
    };
    let size = AppSize {
        width: axis("width"),
        height: axis("height"),
    };
    // All-null/absent axes carry no information beyond the default — collapse to
    // `None` so such an entry encodes identically to a hint-less one.
    if size.width.is_none() && size.height.is_none() {
        None
    } else {
        Some(size)
    }
}

/// The catalog + every bundle read from a `dist/` directory. The catalog is the
/// index; `bundles` pairs each `id` with its self-contained HTML.
#[derive(Default)]
pub struct IngestedApps {
    pub catalog: AppCatalog,
    pub bundles: Vec<(String, AppBundle)>,
}

/// One ingested app-set keyed by set id (`games` / `apps`), each with its own
/// catalog + bundles. A `BTreeMap` so iteration order is stable (set ids sort).
pub type IngestedSets = BTreeMap<String, IngestedApps>;

/// Read an entity-apps `dist/` directory, splitting entries into app-sets by
/// their `type`. Skips catalog entries whose `<id>.html` is missing (logged),
/// so a partial `dist/` still yields the bundles present. Empty sets are
/// omitted from the returned map.
pub fn read_dist(dir: &Path) -> Result<IngestedSets, String> {
    let index_json = dir.join("index.json");
    let txt = std::fs::read_to_string(&index_json)
        .map_err(|e| format!("read {}: {e}", index_json.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&txt).map_err(|e| format!("parse index.json: {e}"))?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| "index.json is not a JSON array".to_string())?;

    let mut sets: IngestedSets = BTreeMap::new();
    for item in arr {
        let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }
        let app_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let set = paths::set_for_type(app_type);
        let entry = AppEntry {
            id: id.clone(),
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string(),
            description: item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            saves: item.get("saves").and_then(|v| v.as_bool()).unwrap_or(false),
            category: item
                .get("category")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            glyph: item.get("glyph").and_then(|v| v.as_str()).map(str::to_string),
            // We support `icon: { type: "svg", body }` — keep only the body.
            icon: item
                .get("icon")
                .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("svg"))
                .and_then(|i| i.get("body"))
                .and_then(|b| b.as_str())
                .map(str::to_string),
            size: parse_size(item.get("size")),
        };

        let html_path = dir.join(format!("{id}.html"));
        match std::fs::read_to_string(&html_path) {
            Ok(html) => {
                let into = sets.entry(set.to_string()).or_default();
                into.bundles.push((id.clone(), AppBundle::new(html)));
                into.catalog.entries.push(entry);
            }
            Err(e) => {
                // A catalog entry without its bundle is skipped, not fatal.
                tracing::warn!(id = %id, error = %e, "ingest: bundle html missing, skipping entry");
            }
        }
    }
    Ok(sets)
}

/// Write one already-read app-set into a peer's tree under `/{peer}/apps/{set}/`
/// via the arm-aware [`Peers::seed_write`] router (Direct → sync L0 put, so a
/// same-pass read sees it). Returns the number of bundles written.
pub fn write_set_into(peers: &Peers, peer_id: &str, set: &str, ing: &IngestedApps) -> usize {
    peers.seed_write(peer_id, paths::catalog_path(peer_id, set), ing.catalog.to_entity());
    for (id, bundle) in &ing.bundles {
        peers.seed_write(peer_id, paths::bundle_path(peer_id, set, id), bundle.to_entity());
    }
    ing.bundles.len()
}

/// Ingest a `dist/` directory **into a peer's tree** — split it into app-sets
/// and write each under its `/{peer}/apps/{set}/` subgraph. The disk→tree half,
/// mirroring `content_site::ingest::ingest_path`. Returns the total bundle count.
pub fn ingest_into(peers: &Peers, peer_id: &str, dir: &Path) -> Result<usize, String> {
    let sets = read_dist(dir)?;
    let mut total = 0;
    for (set, ing) in &sets {
        total += write_set_into(peers, peer_id, set, ing);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, content: &str) {
        let p = dir.join(name);
        let mut f = std::fs::File::create(p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn reads_and_splits_sets_by_type() {
        let tmp = std::env::temp_dir().join(format!("apps-ingest-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "index.json",
            r#"[{"id":"ttt","name":"Tic","description":"d","saves":true,"type":"canvas-game"},
                {"id":"war","name":"War","description":"w","saves":false,"type":"canvas-game"},
                {"id":"calc","name":"Calc","description":"c","saves":false,"type":"tool"}]"#,
        );
        write(&tmp, "ttt.html", "<html>ttt</html>");
        write(&tmp, "war.html", "<html>war</html>");
        write(&tmp, "calc.html", "<html>calc</html>");

        let sets = read_dist(&tmp).unwrap();
        // canvas-game → games set; tool → apps set.
        let games = sets.get(paths::GAMES_SET).expect("games set present");
        assert_eq!(games.catalog.entries.len(), 2);
        assert!(games.catalog.entries.iter().any(|e| e.id == "ttt"));
        assert_eq!(games.bundles.len(), 2);
        let apps = sets.get(paths::APPS_SET).expect("apps set present");
        assert_eq!(apps.catalog.entries.len(), 1);
        assert_eq!(apps.catalog.entries[0].id, "calc");
        assert_eq!(apps.bundles[0].1.html, "<html>calc</html>");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Smoke-test the REAL `../entity-apps/dist` if it's checked out beside us:
    /// every entry declared in `index.json` must resolve to a present,
    /// non-trivial `<id>.html` bundle. This is the guard for "apps keep
    /// flowing through" — when a new app is added but its bundle fails to
    /// build (or `build.py` wasn't re-run), the declared/ingested counts
    /// diverge and this fails, instead of the gap surfacing only at publish.
    /// Skipped (not failed) when the sibling repo is absent, so CI without it
    /// stays green. Mirrors what `make publish-papers --ingest-apps` reads.
    #[test]
    fn real_entity_apps_dist_has_a_bundle_for_every_entry() {
        let dir = Path::new("../entity-apps/dist");
        let index = dir.join("index.json");
        if !index.exists() {
            eprintln!("skip: {} not present (entity-apps not checked out here)", index.display());
            return;
        }
        let txt = std::fs::read_to_string(&index).unwrap();
        let declared = serde_json::from_str::<serde_json::Value>(&txt)
            .expect("entity-apps index.json must be valid JSON")
            .as_array()
            .expect("index.json must be a JSON array")
            .len();
        let sets = read_dist(dir).expect("real entity-apps dist should ingest cleanly");
        let ingested: usize = sets.values().map(|s| s.bundles.len()).sum();
        assert_eq!(
            ingested, declared,
            "every entity-apps index.json entry ({declared}) must have a present <id>.html \
             bundle, but read_dist captured {ingested} — a mismatch means a declared app has a \
             missing/unbuilt bundle file (re-run entity-apps build.py?)"
        );
        for (set_id, apps) in &sets {
            for (id, bundle) in &apps.bundles {
                assert!(
                    bundle.html.len() > 200,
                    "set '{set_id}' app '{id}': bundle HTML is suspiciously small \
                     ({} bytes) — a broken build?",
                    bundle.html.len()
                );
            }
        }
    }

    #[test]
    fn parses_presentation_hints_from_index_json() {
        let tmp = std::env::temp_dir().join(format!("apps-ingest-hints-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "index.json",
            r#"[{"id":"audio-lab","name":"Audio Lab","saves":false,"type":"tool",
                 "category":"audio","glyph":"🎛️",
                 "icon":{"type":"svg","body":"<path d='M6 4v16'/>"},
                 "size":{"width":460,"height":600}},
                {"id":"beat-maker","name":"Beat Maker","saves":true,"type":"tool",
                 "size":{"width":680,"height":null}},
                {"id":"war","name":"War","saves":true,"type":"canvas-game"}]"#,
        );
        write(&tmp, "audio-lab.html", "<html>al</html>");
        write(&tmp, "beat-maker.html", "<html>bm</html>");
        write(&tmp, "war.html", "<html>war</html>");

        let sets = read_dist(&tmp).unwrap();
        let apps = sets.get(paths::APPS_SET).expect("apps set present");
        let al = apps.catalog.entries.iter().find(|e| e.id == "audio-lab").unwrap();
        assert_eq!(al.category.as_deref(), Some("audio"));
        assert_eq!(al.glyph.as_deref(), Some("🎛️"));
        assert_eq!(al.icon.as_deref(), Some("<path d='M6 4v16'/>"));
        assert_eq!(al.size.unwrap().width, Some(460));
        assert_eq!(al.size.unwrap().height, Some(600));
        // height:null → fill that axis (None), width still capped.
        let bm = apps.catalog.entries.iter().find(|e| e.id == "beat-maker").unwrap();
        assert_eq!(bm.size.unwrap().width, Some(680));
        assert_eq!(bm.size.unwrap().height, None);
        // No hints at all → all None.
        let games = sets.get(paths::GAMES_SET).expect("games set present");
        let war = &games.catalog.entries[0];
        assert!(war.category.is_none() && war.glyph.is_none() && war.icon.is_none() && war.size.is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn skips_entry_with_missing_bundle() {
        let tmp = std::env::temp_dir().join(format!("apps-ingest-miss-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "index.json",
            r#"[{"id":"have","name":"H","saves":false,"type":"canvas-game"},
                {"id":"gone","name":"G","saves":false,"type":"canvas-game"}]"#,
        );
        write(&tmp, "have.html", "<html>have</html>");
        let sets = read_dist(&tmp).unwrap();
        let games = sets.get(paths::GAMES_SET).expect("games set present");
        assert_eq!(games.catalog.entries.len(), 1, "missing bundle entry skipped");
        assert_eq!(games.bundles.len(), 1);
        assert_eq!(games.bundles[0].0, "have");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
