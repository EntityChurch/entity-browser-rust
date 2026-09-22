//! **The cache-immutability rule — the one expression of it (C15).**
//!
//! `Cache-Control` here is **opt IN to immutable**, never opt in to mutable.
//! An under-cached immutable file is slow; a mis-cached mutable file is a
//! deployment nobody can correct for a year — brick-matrix cells #7/#8/#9, and
//! #9 has **no remedy at all** once both the browser and the CDN hold it.
//!
//! # Why this file exists
//!
//! `REVIEW-2026-08-25` §2.1 found the rule written **four times**, disagreeing
//! four ways, each version individually reasonable:
//!
//! | | Where | Content store | Hash-named bundle |
//! |---|---|---|---|
//! | 1 | `tools/cors-serve.py` | `"/content/" in path`, on the raw target *including the query* | hex must run to the extension |
//! | 2 | `src-tauri/.../app_server.rs` | `starts_with("/content/")`, query stripped | strips `_bg`, requires a head |
//! | 3 | `src/content_site/named_site.rs` | `contains("content/")` — **no leading slash** | hex run need not reach the extension |
//! | 4 | `PUBLISHING-QUICKSTART` §6.2 (what an operator copies) | `starts_with("/content/")` | hex must run to the extension |
//!
//! `GOTCHAS.md` asserted of 1 and 3 that they were *"duplicated deliberately …
//! so the config an operator copies and the server our tests trust cannot
//! disagree."* They already disagreed. That is the file's own warning happening
//! one layer up, and it is why this is a module and not a fifth careful copy.
//!
//! # The rule, and why it is shaped like this
//!
//! Immutable iff the bytes' **name is their hash**, in one of two shapes:
//!
//! * **The content store** — the path tail is `content/{aa}/{bb}/{hash}`, where
//!   `aa`/`bb` are the hash's own first four hex characters. Anything else under
//!   a `content/` segment is *not* content-addressed.
//! * **A hash-named bundle** — `…-<8+ lowercase hex>.js` or `…-<8+ hex>_bg.wasm`,
//!   trunk's output, where a rebuild is a new URL.
//!
//! **The shard test replaces both of the old content tests, and it is the whole
//! safety argument.** `contains("content/")` was the dangerous one: Hugo, Zola
//! and Lektor all name their source tree `content/`, so an ingested site
//! publishes `/{peer}/sites/<site>/content/about.html` — mutable bytes at a
//! stable URL, pinned for a year. `starts_with("/content/")` was safe but lost
//! every **prefixed** deployment (`dist-federation` emits `/docs/content/…`
//! across five prefixes; 92 blobs that would get no immutable caching and
//! nothing would say so). Matching the shard structure is exact in both
//! directions, and it is **self-verifying**: `aa`/`bb` must equal the hash's own
//! first four characters, so a directory that merely looks like the store cannot
//! satisfy it by accident.
//!
//! Measured across every published tree on the build box (`dist-real`,
//! `dist-ecdeos`, `dist-federation`, `dist-all`, `dist-preview`, `dist-prodtest`,
//! `dist-entity-church-foundation`, …): **7653 files under a `content/` segment,
//! 7653 matching, zero exceptions.**
//!
//! # Lowercase hex, deliberately
//!
//! The published hashes and trunk's filenames are lowercase. Accepting uppercase
//! would only ever *add* files to the immutable set, which is the unsafe
//! direction; a strangely-cased name falling to `no-store` costs a re-download.
//!
//! # The four call sites, and how they are held together
//!
//! This file is the rule. `src-tauri`'s asset server `include!`s it verbatim
//! (there is no `[lib]` target to depend on, and a second careful copy is the
//! thing being fixed). The Python dev/CDN reference server cannot include Rust,
//! and the CDN recipe in the quickstart is prose — so all four are pinned to one
//! **shared vector file**, `tools/cache-policy-vectors.txt`: this module's tests
//! read it, and `tools/cache-policy-lint.sh` (in `make lint`) runs the Python
//! expression over the same lines. Two implementations that cannot disagree
//! without a red gate is the achievable version of "one rule" across three
//! languages; a comment asking the next author to keep them in step is not, and
//! is exactly what drifted.

// THE RULE ITSELF lives in a separate file so `src-tauri/src/app_server.rs` can
// `include!` it verbatim from another directory (there is no `[lib]` target to
// depend on). Keep this an `include!` and not a `mod`: a `mod` here would make
// the file a module of THIS crate only, which is the arrangement that let the
// four expressions drift in the first place.
include!("cache_policy_rule.rs");

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared vector file — the same lines `tools/cache-policy-lint.sh`
    /// runs the Python expression over. This is what "one rule, four call
    /// sites" actually reduces to across three languages.
    const VECTORS: &str = include_str!("../tools/cache-policy-vectors.txt");

    fn vectors() -> Vec<(bool, String)> {
        VECTORS
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (expect, path) = l
                    .split_once(char::is_whitespace)
                    .unwrap_or_else(|| panic!("malformed vector line: {l:?}"));
                match expect {
                    "immutable" => (true, path.trim().to_string()),
                    "mutable" => (false, path.trim().to_string()),
                    other => panic!("vector expectation must be immutable|mutable, got {other:?}"),
                }
            })
            .collect()
    }

    #[test]
    fn the_rule_agrees_with_every_shared_vector() {
        let v = vectors();
        assert!(
            v.len() >= 20,
            "the vector file is the shared specification for FOUR expressions of this \
             rule; {} lines is not a specification. If vectors were deleted rather than \
             added, the Python half of the gate got weaker at the same moment.",
            v.len()
        );
        let mut wrong = Vec::new();
        for (expect, path) in &v {
            if is_immutable(path) != *expect {
                wrong.push(format!(
                    "{path:?}: expected {}, got {}",
                    if *expect { "immutable" } else { "mutable" },
                    if *expect { "mutable" } else { "immutable" }
                ));
            }
        }
        assert!(wrong.is_empty(), "cache policy disagrees with the vectors:\n{wrong:#?}");
    }

    /// The vector file must actually exercise both directions, or a rule that
    /// returned a constant would pass it.
    #[test]
    fn the_vectors_cover_both_answers_and_every_trap() {
        let v = vectors();
        assert!(v.iter().any(|(e, _)| *e), "no immutable vectors");
        assert!(v.iter().any(|(e, _)| !*e), "no mutable vectors");
        for needle in [
            "?",             // the query-string trap (cors-serve.py's divergence)
            "sites",         // an ingested site's own content/ dir (the Hugo trap)
            "_bg.wasm",      // the suffix one expression shipped without
        ] {
            assert!(
                v.iter().any(|(_, p)| p.contains(needle)),
                "the vectors no longer cover {needle:?} — a trap this rule was written \
                 for is unrepresented, so the gate cannot fail the way it failed before"
            );
        }
    }

    /// The dangerous direction, stated as its own test because it is the one
    /// that produces a deployment nobody can correct.
    #[test]
    fn an_ingested_sites_own_content_directory_is_not_the_blob_store() {
        // Hugo/Zola/Lektor all name the source tree `content/`.
        assert!(!is_immutable("/2K9hB/sites/blog/content/about.html"));
        assert!(!is_immutable("/2K9hB/sites/blog/content/2026/01/post.html"));
        // ...and the no-leading-slash form that one expression accepted.
        assert!(!is_immutable("/mycontent/ab/cd/abcd1234abcd1234abcd1234abcd1234"));
        assert!(!is_immutable("/oldcontent/ab/cd/abcd1234abcd1234abcd1234abcd1234"));
    }

    /// The safe direction has a real cost and it is the reason for the shard
    /// test rather than a prefix test: a prefixed deployment must keep its
    /// immutable caching.
    #[test]
    fn a_prefixed_deployment_still_gets_immutable_blobs() {
        let h = "00cae3408b6ed7ad12be0cde47e2957f768f252ac20e0afdf7b70dda5812b66ac0";
        assert!(is_immutable(&format!("/content/00/ca/{h}")));
        assert!(is_immutable(&format!("/docs/content/00/ca/{h}")));
        assert!(is_immutable(&format!("/protocol/content/00/ca/{h}")));
    }

    /// The shard must be the hash's own first four characters — otherwise this
    /// is a claim about a directory name, not about content-addressing.
    #[test]
    fn a_shard_that_does_not_match_the_hash_is_not_the_blob_store() {
        let h = "00cae3408b6ed7ad12be0cde47e2957f768f252ac20e0afdf7b70dda5812b66ac0";
        assert!(is_immutable(&format!("/content/00/ca/{h}")));
        assert!(!is_immutable(&format!("/content/ff/ff/{h}")));
        assert!(!is_immutable(&format!("/content/00/ff/{h}")));
    }

    #[test]
    fn the_query_string_cannot_smuggle_a_path_into_the_immutable_set() {
        let h = "00cae3408b6ed7ad12be0cde47e2957f768f252ac20e0afdf7b70dda5812b66ac0";
        assert!(!is_immutable(&format!("/index.html?x=/content/00/ca/{h}")));
        assert!(!is_immutable(&format!("/sw.js?v=/content/00/ca/{h}")));
        // And the query must not break a genuinely immutable path either.
        assert!(is_immutable(&format!("/content/00/ca/{h}?download=1")));
    }

    #[test]
    fn the_mutable_files_it_is_easy_to_forget_are_all_mutable() {
        for p in [
            "/",
            "/index.html",
            "/sw.js",
            "/sw-selfdestruct.js",
            "/entity-deployment.json",
            "/builds.json",
            "/entity-worker.js",
            "/entity-worker-loader.js",
            "/2K9hB/system/peer/published-root",
            "/2K9hB/sites.list",
            "/2K9hB/transport-profile",
        ] {
            assert!(!is_immutable(p), "{p} must not be cacheable as immutable");
        }
    }

    /// `entity-worker.js` is the counterexample worth naming: it is app CODE and
    /// it is NOT hash-named, so it is mutable here. `REVIEW-2026-08-25` §3.3 is
    /// about the service worker treating that pair as immutable anyway, on a
    /// borrowed identity; this rule does not, and must not start to.
    #[test]
    fn the_worker_pair_is_not_hash_named_and_so_is_not_immutable() {
        assert!(!is_immutable("/entity-worker.js"));
        assert!(!is_immutable("/entity-worker_bg.wasm"));
    }

    #[test]
    fn trunks_real_output_names_are_immutable_and_near_misses_are_not() {
        // Copied from `dist/`, not invented — the fixture mistake that let the
        // missing `_bg` ship.
        assert!(is_immutable("/entity-browser-ee8896686ebf0754_bg.wasm"));
        assert!(is_immutable("/entity-browser-ee8896686ebf0754.js"));
        // The hex run must reach the extension.
        assert!(!is_immutable("/foo-1a2b3c4dxyz.js"));
        // ...and there must be a name before the dash.
        assert!(!is_immutable("/-deadbeef12.js"));
        // ...and it must be long enough to be a hash.
        assert!(!is_immutable("/entity-browser-abc.js"));
        // Uppercase falls to the safe side.
        assert!(!is_immutable("/entity-browser-EE8896686EBF0754.js"));
    }
}
