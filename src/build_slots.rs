// **C9 — the publisher half of rollback: retained shells and `/builds.json`.**
//
// Heal-path row 10 / `DESIGN-CODE-AXIS-RECOVERY-AND-BOOT-SLOTS` §3.2. Today `/`
// is the only shell and each deploy overwrites it, so the previous *Entry* is
// destroyed while all its *Assets* survive — the half-move the immutable-web-app
// ordering warns about. There is no mechanism whatsoever for rolling back
// (§3.0's own table), which is release-risk **R-3** and brick-matrix cells
// **#3/#4** at E6.
//
// Two additions, neither of which touches the running client:
//
// * **Retain each build's shell** at `/builds/{build_id}/index.html`. ~50 KB
//   each; the bundles are already shared by hash and already retained.
// * **`/builds.json`** — the *Entries* manifest, newest first, mutable tier.
//
// # What a `build_id` is, and the limit that comes with it
//
// §3.1 fixes it: a build is **identified by the main bundle hash** and
// **labelled by the commit**. That is deliberate — two docs-only commits produce
// byte-identical wasm, and §4A.0 measured exactly that across the two live
// domains. Rolling back to either is the same act, so they are one slot.
//
// **The limit this buys, stated because it will bite someone:** a change
// confined to UNHASHED assets — `assets/sw.js`, the worker pair, `index.html`
// itself — does not move the bundle hash and so is **not a distinct build id**.
// Retaining it overwrites the shell already stored under that id. The commit
// label still changes, so `builds.json` records the newer commit against the
// same slot; you can see that it happened, you just cannot roll between them.
// Widening the id to cover unhashed assets is a real change, not a tweak: it
// would make every `sw.js` edit a new slot to retain and prune.
//
// # `released_at` is a publish fact, not a build fact
//
// `tools/build-stamp.sh` deliberately carries **no timestamp**, because the same
// commit must produce the same bytes and a build clock would destroy that. This
// is not a contradiction: `builds.json` is written by the *publish*, is mutable,
// and is never an input to a build. It is passed in rather than read from the
// clock so a re-run is reproducible and so a release pipeline can stamp the time
// it means.
//
// # Ordering is the publisher's, not this tool's
//
// §3.2's rule is **assets → per-build entry → `/` → `builds.json` → (much
// later) prune**, and it governs the UPLOAD, which this does not perform. What
// this owns is the half that can be got wrong locally: **never remove an asset a
// retained build names** — and it is opt-in, because pruning is the only
// destructive thing here.
//
// **What `--prune` actually does, stated because the earlier wording overclaimed
// it:** it removes retired *shells* and drops their entries from `builds.json`,
// un-naming before removing so the manifest never advertises a slot that 404s.
// It does **not** remove assets. `assets_named_by` is computed and reported for
// whoever implements that, and it is not an active guard — the safe direction,
// since an over-eager asset prune is the one mistake here that cannot be undone
// by re-publishing. The limit that comes with it: a shell is ~100 KB against a
// bundle measured in MB, so pruning reclaims the small half and the hashed
// bundles accumulate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One retained build — an *Entry* in the immutable-web-app sense.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildEntry {
    /// The main bundle's content hash. The slot identity. See the module note.
    pub build_id: String,
    /// The commit `build-stamp.sh` recorded. A label, never the identity:
    /// several commits can share one build id.
    pub commit: Option<String>,
    /// When this was published. Supplied, never read from the clock.
    pub released_at: String,
    /// The changelog line a person reads in the recovery UI.
    pub notes: String,
    /// Monotone publish counter. **This is what `min_rollback_index` compares
    /// against** — an ordering that survives `build_id` being a content hash
    /// with no order of its own.
    pub index: u64,
}

/// `/builds.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildsManifest {
    /// Newest first. §3.2.
    pub builds: Vec<BuildEntry>,
    /// **The anti-rollback floor (§3.6).** A pin below this is refused with a
    /// stated reason. Kept at the DOCUMENT level, not per entry: the design's
    /// §3.2 lists it inside the per-build tuple, but §3.6 describes exactly one
    /// value — *"the client stores the highest it has seen"* — and N per-entry
    /// floors would be N answers to a question with one.
    ///
    /// **Only ever advanced for a build that fixes something not safely
    /// rollback-able** (the `ecdeos.org` re-key is the worked example), and
    /// **after** the build is marked successful, never before — the reverse
    /// order brands the fallback unbootable at the moment it is needed
    /// (§2.2(5)).
    pub min_rollback_index: u64,
}

impl BuildsManifest {
    pub fn find(&self, build_id: &str) -> Option<&BuildEntry> {
        self.builds.iter().find(|b| b.build_id == build_id)
    }

    /// The next publish counter. Max+1 rather than `len()`, so pruning entries
    /// cannot make a later publish reuse an index a client already stored as
    /// its floor.
    pub fn next_index(&self) -> u64 {
        self.builds.iter().map(|b| b.index).max().map_or(0, |m| m + 1)
    }

    /// Record a publish.
    ///
    /// **Re-publishing an existing `build_id` updates it in place and keeps its
    /// original index.** That is the docs-only-commit case from the module note:
    /// the same code, published again, is not a new slot, and giving it a new
    /// index would push the whole ordering forward for a release that changed no
    /// code. The commit label and notes are refreshed, because those did change.
    pub fn record(&mut self, mut entry: BuildEntry) {
        if let Some(existing) = self.builds.iter().position(|b| b.build_id == entry.build_id) {
            entry.index = self.builds[existing].index;
            self.builds[existing] = entry;
        } else {
            entry.index = self.next_index();
            self.builds.insert(0, entry);
        }
        // Newest first, by index — `record` is the only writer, so this is the
        // one place the order is established.
        self.builds.sort_by(|a, b| b.index.cmp(&a.index));
    }

    /// The `build_id`s to retain, newest first, capped at `keep`.
    pub fn retained(&self, keep: usize) -> Vec<String> {
        self.builds.iter().take(keep).map(|b| b.build_id.clone()).collect()
    }

    pub fn to_json(&self) -> String {
        let builds: Vec<serde_json::Value> = self
            .builds
            .iter()
            .map(|b| {
                serde_json::json!({
                    "build_id": b.build_id,
                    "commit": b.commit,
                    "released_at": b.released_at,
                    "notes": b.notes,
                    "index": b.index,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "min_rollback_index": self.min_rollback_index,
            "builds": builds,
        });
        serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
    }

    /// Parse, tolerating a document written by a newer publisher.
    ///
    /// **A malformed manifest yields `None`, never an empty one.** *"There is no
    /// manifest"* and *"there is one and I cannot read it"* decide different
    /// things — the first is a first publish, the second must not silently
    /// discard every retained build's record and restart the index at 0, which
    /// would let a later publish reuse an index a client already stored as its
    /// anti-rollback floor. Same distinction `window_index` draws between
    /// `no-index` and `malformed`, and for the same reason.
    pub fn from_json(text: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let arr = v.get("builds")?.as_array()?;
        let mut builds = Vec::with_capacity(arr.len());
        for b in arr {
            builds.push(BuildEntry {
                build_id: b.get("build_id")?.as_str()?.to_string(),
                commit: b.get("commit").and_then(|c| c.as_str()).map(str::to_string),
                released_at: b
                    .get("released_at")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string(),
                notes: b.get("notes").and_then(|c| c.as_str()).unwrap_or_default().to_string(),
                index: b.get("index")?.as_u64()?,
            });
        }
        builds.sort_by(|a, b| b.index.cmp(&a.index));
        // The floor gets the same split as `builds`: ABSENT (or an explicit
        // `null`) is a real zero — a first publish, or a publisher that never set
        // one. PRESENT AND UNREADABLE establishes nothing, and defaulting it to
        // zero is not neutral: zero is the one value that disarms the
        // refuse-to-lower guard below AND is written straight back out by the
        // next publish, erasing the floor with no flag and no warning.
        let min_rollback_index = match v.get("min_rollback_index") {
            None | Some(serde_json::Value::Null) => 0,
            Some(x) => x.as_u64()?,
        };
        Some(Self { builds, min_rollback_index })
    }
}

/// Every asset path a shell names, relative to the tree root.
///
/// Used only by `prune`, and it is the whole safety argument there: a retained
/// shell whose bundle has been deleted is a slot that looks bootable in
/// `builds.json` and 404s when someone actually falls back to it — which is
/// worse than not retaining it, because the fallback fails at the moment it is
/// needed.
///
/// Deliberately over-inclusive: anything that looks like a root-relative path to
/// a hashed asset counts. A false positive costs disk; a false negative costs a
/// build that cannot boot.
pub fn assets_named_by(shell_html: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = shell_html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' || c == b'\'' {
            if let Some(end) = shell_html[i + 1..].find(c as char) {
                let cand = &shell_html[i + 1..i + 1 + end];
                if cand.ends_with(".js") || cand.ends_with(".wasm") {
                    out.insert(cand.trim_start_matches("./").trim_start_matches('/').to_string());
                }
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Where a retained shell lives.
pub fn shell_path(root: &Path, build_id: &str) -> PathBuf {
    root.join("builds").join(build_id).join("index.html")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, idx: u64) -> BuildEntry {
        BuildEntry {
            build_id: id.into(),
            commit: Some(format!("c{idx}")),
            released_at: "2026-09-02T00:00:00Z".into(),
            notes: format!("note {idx}"),
            index: idx,
        }
    }

    #[test]
    fn a_new_build_lands_newest_first_and_takes_the_next_index() {
        let mut m = BuildsManifest::default();
        m.record(entry("aaa", 0));
        m.record(entry("bbb", 0));
        m.record(entry("ccc", 0));
        assert_eq!(
            m.builds.iter().map(|b| b.build_id.as_str()).collect::<Vec<_>>(),
            vec!["ccc", "bbb", "aaa"]
        );
        assert_eq!(m.builds[0].index, 2);
        assert_eq!(m.next_index(), 3);
    }

    /// The docs-only-commit case, which §4A.0 measured on the live fleet: two
    /// commits, byte-identical wasm, one slot.
    #[test]
    fn republishing_the_same_build_id_updates_it_in_place_and_keeps_its_index() {
        let mut m = BuildsManifest::default();
        m.record(entry("aaa", 0));
        m.record(entry("bbb", 0));
        let mut again = entry("aaa", 0);
        again.commit = Some("newer-commit".into());
        again.notes = "docs only".into();
        m.record(again);

        assert_eq!(m.builds.len(), 2, "a republish must not add a slot");
        let a = m.find("aaa").expect("still present");
        assert_eq!(a.index, 0, "the index must not move for a republish");
        assert_eq!(a.commit.as_deref(), Some("newer-commit"), "the label is refreshed");
        assert_eq!(a.notes, "docs only");
        assert_eq!(
            m.builds[0].build_id, "bbb",
            "and it must not be promoted to newest — the code did not change"
        );
    }

    /// Pruning must not make a later publish reuse an index a client may already
    /// hold as its anti-rollback floor.
    #[test]
    fn the_index_never_goes_backwards_after_entries_are_dropped() {
        let mut m = BuildsManifest::default();
        for id in ["a", "b", "c", "d"] {
            m.record(entry(id, 0));
        }
        assert_eq!(m.next_index(), 4);
        m.builds.truncate(1); // a prune
        assert_eq!(
            m.next_index(),
            4,
            "next_index is max+1, not len — a pruned manifest must not restart the counter"
        );
    }

    #[test]
    fn a_manifest_round_trips() {
        let mut m = BuildsManifest {
            min_rollback_index: 2,
            ..Default::default()
        };
        m.record(entry("aaa", 0));
        m.record(entry("bbb", 0));
        let back = BuildsManifest::from_json(&m.to_json()).expect("parses");
        assert_eq!(back, m);
    }

    /// *"There is none"* and *"there is one and I cannot read it"* decide
    /// different things — the second must not silently restart the index.
    #[test]
    fn a_malformed_manifest_is_none_and_not_an_empty_one() {
        assert!(BuildsManifest::from_json("not json").is_none());
        assert!(BuildsManifest::from_json("{}").is_none(), "no builds key");
        assert!(
            BuildsManifest::from_json(r#"{"builds":[{"commit":"x"}]}"#).is_none(),
            "an entry with no build_id fails the WHOLE manifest — a short manifest still \
             authorizes a prune"
        );
        let empty = BuildsManifest::from_json(r#"{"builds":[]}"#).expect("an empty list is valid");
        assert_eq!(empty.builds.len(), 0);
    }

    /// The floor gets the same `no-index` vs `malformed` split the `builds` array
    /// already has, and it is the field where collapsing them is most expensive.
    ///
    /// **Absent** legitimately means zero — a first publish, or a publisher that
    /// never set a floor. **Present and unreadable** establishes nothing, and
    /// reading it as zero is not a neutral default: it is the one value that
    /// disarms `--min-rollback-index`'s refuse-to-lower guard (which compares
    /// against what was parsed) *and* silently rewrites the document with the
    /// floor erased on the very next publish, no flag and no warning required.
    /// Widening the floor's domain is the unsafe direction — AP40.
    #[test]
    fn a_floor_that_is_present_but_unreadable_is_malformed_not_zero() {
        let with = |f: &str| format!(r#"{{"min_rollback_index":{f},"builds":[]}}"#);

        assert_eq!(
            BuildsManifest::from_json(&with("3")).expect("a u64 floor parses").min_rollback_index,
            3
        );
        assert_eq!(
            BuildsManifest::from_json(r#"{"builds":[]}"#).expect("absent parses").min_rollback_index,
            0,
            "absent is a REAL zero — a first publish must not be a hard stop"
        );
        assert_eq!(
            BuildsManifest::from_json(&with("null")).expect("null parses").min_rollback_index,
            0,
            "an explicit null is 'no floor', the same fact as absent"
        );

        for bad in [r#""3""#, "3.5", "-1", "true", "[3]", "{}"] {
            assert!(
                BuildsManifest::from_json(&with(bad)).is_none(),
                "a floor of {bad} is present and unreadable — that must fail the WHOLE \
                 manifest, not silently become 0 and let the next publish erase it"
            );
        }
    }

    #[test]
    fn unknown_fields_are_tolerated_so_an_older_publisher_can_read_a_newer_document() {
        let doc = r#"{"min_rollback_index":1,"tomorrows_field":true,
            "builds":[{"build_id":"aaa","commit":"c","released_at":"t","notes":"n",
                       "index":0,"something_new":42}]}"#;
        let m = BuildsManifest::from_json(doc).expect("parses");
        assert_eq!(m.builds.len(), 1);
        assert_eq!(m.min_rollback_index, 1);
    }

    /// A pruned shell must not be left named in the manifest.
    ///
    /// `builds.json` is the slot list C14 renders as *"Boot this version"*. An
    /// entry whose shell has been removed is a button that 404s at exactly the
    /// moment someone is falling back — and at that URL none of our code runs,
    /// so C10's attempt counter can only heal it on a later visit to `/`. The
    /// index must survive the drop, because `retained` keeps the newest and
    /// `next_index` is max+1.
    #[test]
    fn dropping_pruned_entries_keeps_the_index_and_leaves_no_name_without_a_shell() {
        let mut m = BuildsManifest::default();
        for id in ["a", "b", "c", "d"] {
            m.record(entry(id, 0));
        }
        assert_eq!(m.next_index(), 4);

        // What `run` does under `--prune` with keep=2.
        let keep_ids: BTreeSet<String> = m.retained(2).into_iter().collect();
        assert_eq!(keep_ids.len(), 2, "the two newest");
        m.builds.retain(|b| keep_ids.contains(&b.build_id));

        assert_eq!(
            m.builds.iter().map(|b| b.build_id.as_str()).collect::<Vec<_>>(),
            vec!["d", "c"],
            "only the shells that still exist may be named"
        );
        assert!(m.find("a").is_none() && m.find("b").is_none(), "a pruned shell is un-named");
        assert_eq!(
            m.next_index(),
            4,
            "dropping the OLDEST entries cannot move the counter — the max is what survives"
        );
        assert!(
            BuildsManifest::from_json(&m.to_json()).expect("round-trips").find("a").is_none(),
            "and the document written to disk carries the drop, not just the in-memory copy"
        );
    }

    #[test]
    fn retained_is_newest_first_and_capped() {
        let mut m = BuildsManifest::default();
        for id in ["a", "b", "c", "d"] {
            m.record(entry(id, 0));
        }
        assert_eq!(m.retained(3), vec!["d", "c", "b"]);
        assert_eq!(m.retained(99).len(), 4, "keep larger than the list is not an error");
    }

    #[test]
    fn a_shells_asset_references_are_found_however_they_are_quoted() {
        let html = r#"<html><head>
            <link rel="preload" href="/entity-browser-ee8896686ebf0754_bg.wasm">
            <script type="module" src='./entity-browser-ee8896686ebf0754.js'></script>
            <script>var w = "entity-worker.js";</script>
            <img src="/logo.png">
        </head></html>"#;
        let a = assets_named_by(html);
        assert!(a.contains("entity-browser-ee8896686ebf0754_bg.wasm"));
        assert!(a.contains("entity-browser-ee8896686ebf0754.js"));
        assert!(a.contains("entity-worker.js"), "the worker pair is named too and is NOT hashed");
        assert!(!a.iter().any(|p| p.ends_with(".png")), "only code assets are the prune hazard");
    }

    #[test]
    fn shell_path_is_the_documented_location() {
        assert_eq!(
            shell_path(Path::new("/tmp/dist"), "abc123"),
            Path::new("/tmp/dist/builds/abc123/index.html")
        );
    }
}

// ===========================================================================
// The verb — `entity-browser builds <DIR> [flags]`
// ===========================================================================

/// Retain this tree's shell as a build and record it in `/builds.json`.
///
/// Run AFTER the tree is assembled and stamped, so the shell it retains is the
/// one that will be served. In `make site-dist` that is after both sub-makes.
///
/// **`--prune` is opt-in and it is the only destructive thing here.** Without
/// it, nothing is ever removed — which is the correct default given §3.2's
/// ordering puts pruning *"much later"* than everything else, and given that a
/// retained shell whose assets were pruned is a slot that 404s at exactly the
/// moment someone falls back to it.
pub fn run(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;

    let mut dir: Option<String> = None;
    let mut notes = String::new();
    let mut released_at = String::new();
    let mut keep: usize = 3;
    let mut prune = false;
    let mut advance_floor: Option<u64> = None;

    for a in args.iter().skip(1) {
        if let Some(v) = a.strip_prefix("--notes=") {
            notes = v.to_string();
        } else if let Some(v) = a.strip_prefix("--released-at=") {
            released_at = v.to_string();
        } else if let Some(v) = a.strip_prefix("--keep=") {
            match v.parse::<usize>() {
                Ok(n) if n >= 1 => keep = n,
                _ => {
                    eprintln!("builds: --keep must be a positive integer, got {v:?}");
                    return ExitCode::FAILURE;
                }
            }
        } else if let Some(v) = a.strip_prefix("--min-rollback-index=") {
            match v.parse::<u64>() {
                Ok(n) => advance_floor = Some(n),
                Err(_) => {
                    eprintln!("builds: --min-rollback-index must be an integer, got {v:?}");
                    return ExitCode::FAILURE;
                }
            }
        } else if a == "--prune" {
            prune = true;
        } else if a.starts_with("--") {
            eprintln!("builds: unknown flag {a:?}");
            return ExitCode::FAILURE;
        } else if dir.is_none() {
            dir = Some(a.clone());
        } else {
            eprintln!("builds: unexpected argument {a:?}");
            return ExitCode::FAILURE;
        }
    }

    let Some(dir) = dir else {
        eprintln!("usage: entity-browser builds <DIR> [--notes=…] [--released-at=…] [--keep=N] [--prune] [--min-rollback-index=N]");
        return ExitCode::FAILURE;
    };
    let root = Path::new(&dir);
    let index_html = root.join("index.html");
    let Ok(shell) = std::fs::read_to_string(&index_html) else {
        eprintln!("builds: no {} — assemble the tree first (make site-dist)", index_html.display());
        return ExitCode::FAILURE;
    };

    let Some(build_id) = crate::build_id::parse_bundle_hash(&shell) else {
        eprintln!(
            "builds: {} names no entity-browser-<hash> bundle, so this tree has no build \
             identity to retain. A shell that cannot be identified cannot be rolled back to.",
            index_html.display()
        );
        return ExitCode::FAILURE;
    };
    let commit = parse_commit(&shell);
    if commit.as_deref().is_some_and(|c| c.ends_with("-dirty")) {
        eprintln!(
            "builds: WARNING — this shell is stamped {:?}. A dirty build has a provenance its \
             bytes do not, and retaining it as a rollback target records that claim durably.",
            commit.as_deref().unwrap_or_default()
        );
    }

    // Read the existing manifest. A malformed one is a HARD STOP, never an
    // implicit fresh start: restarting the index could hand a later publish an
    // index a client already stored as its anti-rollback floor.
    let manifest_path = root.join("builds.json");
    let mut manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => match BuildsManifest::from_json(&text) {
            Some(m) => m,
            None => {
                eprintln!(
                    "builds: {} exists but could not be parsed. REFUSING to write a fresh one \
                     — that would restart the publish index and could hand a later build an \
                     index a client already holds as its anti-rollback floor. Fix or remove \
                     the file deliberately.",
                    manifest_path.display()
                );
                return ExitCode::FAILURE;
            }
        },
        Err(_) => BuildsManifest::default(),
    };

    if let Some(floor) = advance_floor {
        if floor < manifest.min_rollback_index {
            eprintln!(
                "builds: REFUSING to LOWER min_rollback_index from {} to {floor}. The floor is \
                 monotone by construction — a client stores the highest it has seen, so lowering \
                 it here changes nothing for anyone who already saw the higher value and only \
                 makes this document disagree with them.",
                manifest.min_rollback_index
            );
            return ExitCode::FAILURE;
        }
        manifest.min_rollback_index = floor;
    }

    manifest.record(BuildEntry {
        build_id: build_id.clone(),
        commit: commit.clone(),
        released_at,
        notes,
        index: 0, // assigned by `record`
    });

    // 1. The per-build entry, BEFORE builds.json names it (§3.2's ordering).
    let dest = shell_path(root, &build_id);
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("builds: could not create {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = std::fs::write(&dest, &shell) {
        eprintln!("builds: could not write {}: {e}", dest.display());
        return ExitCode::FAILURE;
    }

    // 2. Then the manifest that points at it.
    if let Err(e) = std::fs::write(&manifest_path, manifest.to_json()) {
        eprintln!("builds: could not write {}: {e}", manifest_path.display());
        return ExitCode::FAILURE;
    }

    println!("builds: retained {} → {}", build_id, dest.display());
    println!(
        "builds: {} now lists {} build(s), min_rollback_index={}",
        manifest_path.display(),
        manifest.builds.len(),
        manifest.min_rollback_index
    );
    for b in &manifest.builds {
        let here = shell_path(root, &b.build_id).exists();
        println!(
            "  [{}] {} {}  {}{}",
            b.index,
            b.build_id,
            b.commit.as_deref().unwrap_or("(unstamped)"),
            if here { "shell retained" } else { "SHELL MISSING" },
            if b.notes.is_empty() { String::new() } else { format!("  — {}", b.notes) }
        );
    }

    if prune {
        // UN-NAME BEFORE REMOVING, which is §3.2's publish ordering run
        // backwards. `builds.json` must never advertise a slot whose shell is
        // gone: that entry is a "Boot this version" button (C14) that 404s at
        // exactly the moment someone is falling back, and at that URL none of
        // our code runs, so it can only heal on a later visit to `/`.
        //
        // Dropping the entries cannot move the index: `retained` keeps the
        // NEWEST `keep`, so the maximum survives and `next_index` (max+1) is
        // unchanged — which is the property `the_index_never_goes_backwards…`
        // pins. Each intermediate state on disk is safe in its own right: a
        // shell nothing names is unreachable, whereas a name with no shell is
        // the failure above.
        let keep_ids: BTreeSet<String> = manifest.retained(keep).into_iter().collect();
        let dropped: Vec<String> = manifest
            .builds
            .iter()
            .filter(|b| !keep_ids.contains(&b.build_id))
            .map(|b| b.build_id.clone())
            .collect();
        if !dropped.is_empty() {
            manifest.builds.retain(|b| keep_ids.contains(&b.build_id));
            if let Err(e) = std::fs::write(&manifest_path, manifest.to_json()) {
                eprintln!("builds: could not rewrite {}: {e}", manifest_path.display());
                return ExitCode::FAILURE;
            }
            for d in &dropped {
                println!("builds: dropped entry {d} from {}", manifest_path.display());
            }
        }

        match prune_shells(root, &manifest, keep) {
            Ok(removed) => {
                for r in &removed {
                    println!("builds: pruned shell {r}");
                }
                if removed.is_empty() && dropped.is_empty() {
                    println!("builds: nothing to prune (keep={keep})");
                }
            }
            Err(e) => {
                eprintln!("builds: prune failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// The `entity-build` stamp, read back out of a shell.
pub fn parse_commit(shell_html: &str) -> Option<String> {
    let needle = "<meta name=\"entity-build\" content=\"";
    let start = shell_html.find(needle)? + needle.len();
    let rest = &shell_html[start..];
    let end = rest.find('"')?;
    let v = &rest[..end];
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Remove retained shells beyond `keep`, and ONLY the shells.
///
/// **It never removes an asset**, which is §3.2's rule applied where it can
/// actually be enforced: the union of every surviving shell's references is
/// computed and reported, and an asset in that union is not a thing this command
/// will touch. Deleting shared bundles is left to a deliberate, separate act,
/// because a retained build whose bundle is gone is worse than one never
/// retained — it advertises a fallback that 404s at the moment it is needed.
fn prune_shells(
    root: &Path,
    manifest: &BuildsManifest,
    keep: usize,
) -> Result<Vec<String>, String> {
    let keep_ids: BTreeSet<String> = manifest.retained(keep).into_iter().collect();
    let builds_dir = root.join("builds");
    let Ok(entries) = std::fs::read_dir(&builds_dir) else {
        return Ok(Vec::new());
    };

    // The union of what SURVIVES — computed before anything is removed, so a
    // bug in the loop below cannot make the guard weaker as it goes.
    let mut protected: BTreeSet<String> = BTreeSet::new();
    for id in &keep_ids {
        if let Ok(html) = std::fs::read_to_string(shell_path(root, id)) {
            protected.extend(assets_named_by(&html));
        }
    }
    if let Ok(html) = std::fs::read_to_string(root.join("index.html")) {
        protected.extend(assets_named_by(&html));
    }
    // **Assets are NOT pruned, and this set guards nothing today.** It is
    // computed and reported so the number is visible to whoever implements asset
    // pruning — but a set that is only printed must not be read as an active
    // guard, which is how it read before this line said so. The consequence is a
    // real limit, not a nicety: a shell is ~100 KB and the bundle it names is
    // measured in MB, so `--prune` reclaims the small half and what actually
    // accumulates stays on disk forever.
    println!(
        "builds: {} asset(s) are named by a retained shell (NOTE: assets are not pruned — \
         only shells are; this set is reported, not enforced)",
        protected.len()
    );

    let mut removed = Vec::new();
    for e in entries.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if keep_ids.contains(&name) {
            continue;
        }
        if !e.path().is_dir() {
            continue;
        }
        std::fs::remove_dir_all(e.path())
            .map_err(|err| format!("removing {}: {err}", e.path().display()))?;
        removed.push(name);
    }
    Ok(removed)
}
