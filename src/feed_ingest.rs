//! disk → tree ingest for the **third publish axis**: a directory of authored
//! posts, written into a peer's tree as `app/feed/entry` entities.
//!
//! The inverse of [`crate::feed_tree`], and the feed's answer to
//! [`content_site::ingest`](crate::content_site::ingest) — same seam, same
//! direction, same place in the pipeline:
//!
//! ```text
//! posts/*.md  →  INGEST  →  peer tree  →  read_owned_feed  →  publish (one projector)
//! ```
//!
//! **Which authored input is a workflow choice, not a durability model.** This
//! one is a directory of markdown files, because that is what a static-site
//! author, a render tool and a shell script can all produce. An in-tree editor
//! that writes entries directly, or a future *post* verb, are additional
//! front-doors onto the same tree and do not replace this one — the tree is the
//! authority and every input is a way into it.
//!
//! ## `created_at` is REQUIRED, and refusing is the whole reproducibility
//! argument
//!
//! The obvious default is the file's mtime. It is also the one choice that makes
//! a publish irreproducible: `git clone` sets mtimes to checkout time, so the
//! same posts on two machines would produce different entities, different
//! hashes, different page boundaries and a rewritten archive — and
//! `EXTENSION-TREE` §3.2's determinism rule 3 is explicit that a snapshot is
//! *"pure structural data"* with no timestamp in it. A post with no date has no
//! correct answer, so it fails here, where the person who can fix it is
//! standing. Same posture as the site ingest refusing two sources that claim one
//! slug.
//!
//! **The accepted spellings are TOML's own**, because the frontmatter block is
//! TOML and TOML has a date type:
//!
//! ```toml
//! +++
//! title = "A post"
//! created_at = 2026-09-10T14:30:00Z      # offset date-time
//! # created_at = 2026-09-10T14:30:00     # local date-time — read as UTC
//! # created_at = 2026-09-10              # local date — read as 00:00:00 UTC
//! # created_at = "2026-09-10T14:30:00Z"  # the same values as a string
//! +++
//! ```
//!
//! All four go through **one** parser — `toml::value::Datetime` — so the string
//! form cannot drift from the native one. An absent offset is read as **UTC**
//! and that is a decision: `created_at` is ms since the epoch on the wire, there
//! is no author timezone recorded anywhere to consult, and §2.3.2 makes the
//! field *"a display heuristic, never an ordering authority"*, so the cost of
//! being wrong by an offset is bounded. A time with no date is refused — it
//! names no day.
//!
//! ## A body too long to inline takes EMBED §3's pointer arm here, not later
//!
//! [`crate::feed_publish::check_body_is_publishable`] **refuses** an oversized
//! inline payload (`bstr .size (1..16384)`), so the pointer arm is the only
//! conformant way to carry a long post — which makes it the ingest's job, since
//! this is the authoring boundary. The range decision itself is **not
//! re-expressed here**: [`asset_store::stage`] owns it, and a second copy of
//! *"16384, and zero is outside it too"* is exactly the drift C15 exists to
//! stop.
//!
//! ⚠ That function is called `asset_store::stage` and lives under
//! `content_site/`, and neither is a site concern — it is EMBED §3's chunker
//! wearing the name of its first caller. `AGENTS.md` already records that the
//! transport and content layers under `content_site/` are misnamed; this is one
//! more instance and deliberately **not** fixed by writing a second chunker.

// Native-only, for [`crate::content_site::ingest`]'s two reasons: a publisher
// reads a directory and the browser has none, and the TOML parser is a non-wasm
// dependency.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // reached through the publish axis; the gate is native

use std::path::{Path, PathBuf};
use std::sync::Arc;

use entity_entity::Entity;
use entity_store::{ContentStore, MemoryContentStore};

use crate::content_site::asset_store;
use crate::content_site::format::AssetPayload;
use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
use crate::feed::{entry_key, FeedEntry};
use crate::peers::Peers;

/// The post source extension. One, not a list: the site ingest's second
/// extension (`.html`) exists because a Pandoc artifact is a whole document that
/// must survive verbatim, and a feed entry's body is an `embed-node` whose media
/// type says what it is — a second extension here would be a second media type
/// with no reader asking for one.
const POST_EXT: &str = "md";

/// The body media type every post here becomes.
///
/// ⭐ **Shared with the reader, not restated.** This module has minted
/// `text/markdown` since it shipped — every post in `entity-core-papers`' corpus
/// carries it — while the Feed window rendered every body's `fallback` and never
/// looked at the media type at all. So the *producer* was right for weeks and the
/// spelling was already on the wire; what was missing was a reader. Two `const`s
/// that must agree, in a producer and a consumer of the same bytes, is C15 with a
/// silent failure on the end of it (SHARE §2: a type-filtered query on the wrong
/// tag returns a correct, complete, EMPTY answer).
const POST_MEDIA_TYPE: &str = crate::feed_body::MARKDOWN_MEDIA_TYPE;

/// Ingest every post under `dir` into `peer_id`'s tree. Returns how many
/// entries were written.
///
/// Recursive, and **sorted**, so the ingest is a pure function of the directory
/// contents rather than of the order the filesystem happened to answer in. An
/// empty directory is an error rather than a silent zero: someone who passes
/// `--ingest-feed` meant to publish posts, and a publish that quietly carried
/// none is the *"nothing would be removed"* report that deleted every app
/// bundle.
pub fn ingest_path(peers: &Peers, peer_id: &str, dir: &Path) -> Result<Ingested, String> {
    let (posts, notes) = read_post_dir(dir, peer_id)?;

    // Chunk into a scratch store and hand the produced entities to the peer —
    // the site ingest's shape and its reason: `stage` returns the closure by
    // value precisely so the store it chunked into can be thrown away.
    for (entry, content) in &posts {
        for e in content {
            peers.seed_content(peer_id, e.clone());
        }
        let entity = entry.to_entity()?;
        // Keyed by its own content hash — §2.2.1 makes every reference to an
        // entry a pin, so the identity IS the hash. See `feed::entry_key`.
        let key = entry_key(&entity.content_hash);
        peers.seed_write(peer_id, format!("/{peer_id}/{key}"), entity);
    }
    Ok(Ingested { posts: posts.len(), notes })
}

/// What an ingest carried, and what it could not.
///
/// A count alone lets a caller report a clean `ingested N post(s)` over a set
/// that quietly lost an authored attribute — the same shape as `--plan`'s
/// reassuring zero (meta `B-5`), one verb over. The caller gets both or
/// neither.
#[derive(Debug)]
pub struct Ingested {
    /// Entries written into the tree.
    pub posts: usize,
    /// One line per authored key the entry could not carry, each naming the
    /// file. Empty is the ordinary case and prints nothing.
    pub notes: Vec<String>,
}

/// The disk→entity half, with no `Peers` in sight.
///
/// Split out for [`crate::content_site::ingest::read_site_dir`]'s reason, which
/// was earned rather than assumed: *a link you cannot evaluate without standing
/// up a peer is a link nobody evaluates.* Every refusal below is reachable from
/// a test that touches no tree.
///
/// Returns the posts **and** the per-file notes, for [`Ingested`]'s reason.
pub(crate) fn read_post_dir(
    dir: &Path,
    author: &str,
) -> Result<(Vec<(FeedEntry, Vec<Entity>)>, Vec<String>), String> {
    let mut files = Vec::new();
    collect_posts(dir, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("no *.{POST_EXT} posts at {} or anywhere below it", dir.display()));
    }

    let scratch: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let mut out = Vec::with_capacity(files.len());
    let mut notes = Vec::new();
    for path in &files {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let (entry, content, dropped) =
            read_post(&raw, author, &scratch).map_err(|e| format!("{}: {e}", path.display()))?;
        // The file name is the actionable half — a note naming a key and not a
        // post sends the author looking through the whole directory.
        notes.extend(dropped.into_iter().map(|d| format!("{}: {d}", path.display())));
        out.push((entry, content));
    }
    Ok((out, notes))
}

/// One post's text → an entry, the content closure its body needs, and **what
/// the entry could not carry** (see the `params` loop below — a caller cannot
/// take the entry without being handed the loss).
pub(crate) fn read_post(
    raw: &str,
    author: &str,
    store: &Arc<dyn ContentStore>,
) -> Result<(FeedEntry, Vec<Entity>, Vec<String>), String> {
    let (front, body) = split_frontmatter(raw)?;
    let created_at = created_at_ms(&front)?;

    let body = body.trim();
    // `EmbedData::fallback` is MANDATORY and non-empty — §6's degradation
    // ladder, §8's anti-graveyard rule. A post with no body and no title is not
    // a post with an empty fallback; it is nothing, and there is no honest
    // entity to build from it.
    let title = front.get("title").and_then(|v| v.as_str()).unwrap_or_default().trim();
    let fallback = if !title.is_empty() {
        title.to_string()
    } else {
        first_line(body).to_string()
    };
    if fallback.is_empty() {
        return Err(
            "a post needs a body or a `title` — an embed's fallback is mandatory and non-empty \
             (APP-CONVENTION-EMBED §6), and there is nothing here to make one from"
                .into(),
        );
    }

    let staged = asset_store::stage(POST_MEDIA_TYPE, body.as_bytes().to_vec(), store)?;
    // The two arms freshly staged bytes can produce, mapped onto the payload
    // union EMBED §3 owns. The others (`Unreadable`, `Unsupported`,
    // `InvalidForType`) are decode-side outcomes for somebody else's entity and
    // are unreachable from bytes we just chunked — so they are an error naming
    // the arm, never a silent default (AP40).
    let payload = match staged.asset.payload {
        AssetPayload::Inline(bytes) => EmbedPayload::Inline(bytes),
        AssetPayload::Pointer(hash) => EmbedPayload::Pointer(hash),
        other => {
            return Err(format!(
                "staging a post body produced {other:?}, a payload arm authoring cannot reach"
            ))
        }
    };

    let mut data = EmbedData::new(payload, fallback);
    // Everything else the author wrote, carried rather than dropped — `params`
    // is EMBED §3's open attribute bag and is exactly where a `title` or a tag
    // list belongs. **Strings only**, which is the bag's own rule.
    //
    // **`title` is deliberately kept here even though it is also the fallback**,
    // and that is not the untrusted-second-source shape SHARE's `from` was:
    // there the field competed with a *structural* fact (the publisher is in the
    // path, so a body claiming one is a stranger's self-declaration about their
    // own identity). Nothing structural carries a title, and the two slots
    // answer different questions — a renderer wanting a headline wants
    // `params.title`, while `fallback` is EMBED §6's degradation text, which for
    // an untitled post is the first line instead.
    //
    // ⭐ **A key the bag cannot hold is REPORTED, not silently dropped.**
    // `tags = ["a", "b"]` is the shape an author reaches for and the bag is
    // strings only, so the array goes nowhere — and until 2026-09-16 it went
    // nowhere *quietly*, at the one boundary where the person who can fix it is
    // standing. Found by comparing this module against
    // `entity-core-papers`' `tools/feed/emit.py`, which restates these refusals
    // at the authoring seat and **already warned about this loss while we said
    // nothing**: a counterpart's gate being louder than the authority it
    // restates is a defect in the authority.
    //
    // Reported rather than refused, deliberately. `created_at` has no correct
    // answer without the author, which is why that one is fatal; an attribute
    // the bag cannot carry has an obvious one — the post publishes, minus a key
    // nothing was going to read — and refusing would make a conformant post
    // unpublishable over an attribute that is not part of its meaning.
    let mut dropped = Vec::new();
    for (k, v) in front.iter() {
        if k == "created_at" {
            continue; // it is a field on the entry, not an attribute of the body
        }
        match v.as_str() {
            Some(s) => {
                data.params.insert(k.clone(), entity_ecf::text(s));
            }
            None => dropped.push(format!("`{k}` is {} and `params` carries strings only", v.type_str())),
        }
    }

    Ok((
        FeedEntry::new(author, created_at, EmbedNode::new(POST_MEDIA_TYPE, data)),
        staged.content,
        dropped,
    ))
}

/// Every `*.md` at any depth below `dir`, inclusive of a single file path.
fn collect_posts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if dir.is_file() {
        if has_post_ext(dir) {
            out.push(dir.to_path_buf());
        }
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    let mut children: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    children.sort();
    for c in &children {
        if c.is_dir() {
            collect_posts(c, out)?;
        } else if has_post_ext(c) {
            out.push(c.clone());
        }
    }
    Ok(())
}

fn has_post_ext(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(POST_EXT))
}

fn first_line(body: &str) -> &str {
    body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default()
}

/// Split a leading `+++ … +++` TOML block. **A post without one is refused**,
/// because `created_at` lives in it and there is no default for that field —
/// see the module doc.
fn split_frontmatter(raw: &str) -> Result<(toml::Table, String), String> {
    let missing = || {
        "a post needs a `+++` TOML frontmatter block carrying `created_at` — a publish that \
         guessed the date from the file's mtime would produce different entities on every \
         machine that checked the posts out"
            .to_string()
    };
    let rest = raw.strip_prefix("+++\n").ok_or_else(missing)?;
    let end = rest.find("\n+++\n").ok_or_else(|| {
        "the `+++` frontmatter block is opened and never closed".to_string()
    })?;
    let table: toml::Table = rest[..end]
        .parse()
        .map_err(|e| format!("the frontmatter block is not valid TOML: {e}"))?;
    Ok((table, rest[end + "\n+++\n".len()..].to_string()))
}

/// `created_at` → ms since the epoch, or a refusal that says which of the four
/// spellings failed.
fn created_at_ms(front: &toml::Table) -> Result<u64, String> {
    let dt: toml::value::Datetime = match front.get("created_at") {
        Some(toml::Value::Datetime(d)) => *d,
        // The string form goes through the SAME parser, so it cannot mean
        // something the native form does not.
        Some(toml::Value::String(s)) => s.parse().map_err(|e| {
            format!("`created_at = \"{s}\"` is not an RFC 3339 date or date-time: {e}")
        })?,
        Some(other) => {
            return Err(format!(
                "`created_at` must be a date or a date-time, not {}",
                other.type_str()
            ))
        }
        None => {
            return Err(
                "`created_at` is missing — a post's date is authored, never inferred from the \
                 filesystem (see this module's doc for why)"
                    .into(),
            )
        }
    };
    epoch_ms(&dt)
}

/// A TOML datetime → ms since the Unix epoch.
///
/// Hand-rolled because this crate carries no date library and adding one for a
/// single field would be the larger change. The day count is Howard Hinnant's
/// `days_from_civil`, which is the reference algorithm rather than an invention;
/// what makes it trustworthy here is the vector table in
/// [`tests::the_epoch_conversion_matches_the_reference_values`], every row of
/// which was produced by `date -u` and not by this function.
fn epoch_ms(dt: &toml::value::Datetime) -> Result<u64, String> {
    let Some(date) = dt.date else {
        return Err("`created_at` carries a time with no date, which names no instant".into());
    };
    let time = dt.time;
    let days = days_from_civil(date.year as i64, date.month as i64, date.day as i64);
    let secs_of_day = time.map_or(0, |t| {
        t.hour as i64 * 3600 + t.minute as i64 * 60 + t.second as i64
    });
    let millis_of_sec = time.map_or(0, |t| t.nanosecond as i64 / 1_000_000);
    // An absent offset is UTC — stated in the module doc, and the only reading
    // available when nothing records the author's timezone.
    let offset_minutes = match dt.offset {
        Some(toml::value::Offset::Z) | None => 0,
        Some(toml::value::Offset::Custom { minutes }) => minutes as i64,
    };
    let total = days * 86_400_000 + secs_of_day * 1000 + millis_of_sec - offset_minutes * 60_000;
    // `created_at` is unsigned ms on the wire, so a pre-epoch date has no
    // representation. Refused rather than clamped: a post dated 1969 is an
    // authoring mistake, and silently publishing it at the epoch would put it
    // first in the archive forever.
    u64::try_from(total).map_err(|_| {
        format!(
            "`created_at` is {}-{:02}-{:02}, before the Unix epoch — a feed entry's clock is \
             unsigned milliseconds since 1970-01-01",
            date.year, date.month, date.day
        )
    })
}

/// Days from 1970-01-01 to `y-m-d` (proleptic Gregorian). Howard Hinnant's
/// `days_from_civil`, transcribed — not derived here.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> Arc<dyn ContentStore> {
        Arc::new(MemoryContentStore::new())
    }

    fn post(front: &str, body: &str) -> String {
        format!("+++\n{front}\n+++\n{body}")
    }

    /// **The vectors, and none of them was computed by the code under test** —
    /// every row is `date -u -d <spelling> +%s`, times a thousand. A conversion
    /// checked against its own arithmetic is checking that a function is
    /// deterministic.
    #[test]
    fn the_epoch_conversion_matches_the_reference_values() {
        let ms = |s: &str| {
            let dt: toml::value::Datetime = s.parse().expect("the vector parses");
            epoch_ms(&dt).expect("the vector converts")
        };
        assert_eq!(ms("1970-01-01T00:00:00Z"), 0, "the epoch itself");
        assert_eq!(ms("2026-09-10T00:00:00Z"), 1_788_998_400_000);
        assert_eq!(ms("2024-02-29T12:34:56Z"), 1_709_210_096_000, "a leap day");
        assert_eq!(ms("2000-03-01T00:00:00Z"), 951_868_800_000, "the day after a leap-century day");
        assert_eq!(ms("2099-12-31T23:59:59Z"), 4_102_444_799_000);
        // An offset is subtracted, not ignored.
        assert_eq!(ms("2026-09-10T14:30:00+02:00"), 1_789_043_400_000);
        // A bare date is midnight UTC; a local date-time is read as UTC.
        assert_eq!(ms("2026-09-10"), 1_788_998_400_000);
        assert_eq!(ms("2026-09-10T00:00:00"), 1_788_998_400_000);
    }

    /// **The refusal that carries the reproducibility argument.** The tempting
    /// default is the file's mtime, and `git clone` sets that to checkout time
    /// — so the same posts on two machines would hash differently, move a page
    /// boundary and rewrite the archive.
    #[test]
    fn a_post_with_no_date_is_refused_rather_than_dated_from_the_filesystem() {
        let err = read_post(&post("title = \"Hello\"", "hi"), "QmAuthor", &scratch()).unwrap_err();
        assert!(err.contains("created_at"), "and it names the field: {err}");

        // No frontmatter at all is the same refusal, not a silent success.
        let err = read_post("just a body\n", "QmAuthor", &scratch()).unwrap_err();
        assert!(err.contains("created_at"), "{err}");

        // The control: with a date it reads.
        assert!(read_post(
            &post("created_at = 2026-09-10T00:00:00Z", "hi"),
            "QmAuthor",
            &scratch()
        )
        .is_ok());
    }

    #[test]
    fn a_pre_epoch_date_is_refused_because_the_field_is_unsigned() {
        let err = read_post(&post("created_at = 1969-07-20", "moon"), "QmAuthor", &scratch())
            .unwrap_err();
        assert!(err.contains("1969"), "it names the date it refused: {err}");
        assert!(err.contains("unsigned"), "and why: {err}");
    }

    #[test]
    fn a_time_with_no_date_names_no_instant() {
        let front = toml::Table::new();
        let mut front = front;
        front.insert("created_at".into(), toml::Value::String("12:30:00".into()));
        let err = created_at_ms(&front).unwrap_err();
        assert!(err.contains("no date"), "{err}");
    }

    /// The string spelling and the native TOML one go through one parser, so
    /// they cannot mean different instants.
    #[test]
    fn the_string_spelling_and_the_native_one_are_one_parser() {
        let a = read_post(&post("created_at = 2026-09-10T14:30:00Z", "x"), "QmA", &scratch())
            .unwrap()
            .0;
        let b = read_post(&post("created_at = \"2026-09-10T14:30:00Z\"", "x"), "QmA", &scratch())
            .unwrap()
            .0;
        assert_eq!(a.created_at, b.created_at);
        assert_eq!(a.to_entity().unwrap().content_hash, b.to_entity().unwrap().content_hash);
    }

    /// EMBED §6: the fallback is mandatory and non-empty. A post with nothing in
    /// it has no honest entity, so it is refused rather than published with an
    /// empty one.
    #[test]
    fn a_post_with_neither_body_nor_title_is_refused() {
        let err = read_post(&post("created_at = 2026-09-10", "   \n\n "), "QmA", &scratch())
            .unwrap_err();
        assert!(err.contains("fallback"), "{err}");

        // A title alone is enough — a photo post is a real shape.
        assert!(read_post(
            &post("created_at = 2026-09-10\ntitle = \"Just a title\"", ""),
            "QmA",
            &scratch()
        )
        .is_ok());
    }

    /// **A long post takes EMBED §3's pointer arm at the authoring boundary**,
    /// because `feed_publish` refuses an oversized inline payload — so an ingest
    /// that always inlined would make a long post unpublishable. The range
    /// decision is `asset_store::stage`'s; this asserts we consult it.
    #[test]
    fn a_body_over_the_inline_ceiling_becomes_a_pointer_and_brings_its_bytes() {
        let long = "x".repeat(crate::embed::INLINE_PAYLOAD_MAX + 1);
        let (entry, content, _) =
            read_post(&post("created_at = 2026-09-10", &long), "QmA", &scratch()).unwrap();
        let hash = match entry.body.data.payload {
            EmbedPayload::Pointer(h) => h,
            other => panic!("an oversized body must not inline: {other:?}"),
        };
        assert!(content.iter().any(|e| e.content_hash == hash), "the blob came with it");

        // …and the last conformant inline size still inlines, so this is a
        // boundary and not a policy of always chunking.
        let at_ceiling = "x".repeat(crate::embed::INLINE_PAYLOAD_MAX);
        let (entry, content, _) =
            read_post(&post("created_at = 2026-09-10", &at_ceiling), "QmA", &scratch()).unwrap();
        assert!(matches!(entry.body.data.payload, EmbedPayload::Inline(_)));
        assert!(content.is_empty());
    }

    /// Authored attributes ride in EMBED §3's open bag rather than being
    /// dropped; `created_at` does not, because it is a field on the entry and a
    /// second copy is a second source of truth.
    #[test]
    fn authored_attributes_are_carried_and_the_date_is_not_duplicated_into_the_body() {
        let (entry, _, dropped) = read_post(
            &post("created_at = 2026-09-10\ntitle = \"A post\"\ntags = \"rust,feeds\"", "body"),
            "QmA",
            &scratch(),
        )
        .unwrap();
        assert_eq!(entry.body.data.fallback, "A post", "the title is the fallback");
        assert!(entry.body.data.params.contains_key("tags"));
        assert!(
            !entry.body.data.params.contains_key("created_at"),
            "the clock is a field on the entry, not an attribute of its body"
        );
        assert!(dropped.is_empty(), "nothing was lost, so there is nothing to report: {dropped:?}");
    }

    /// **A key `params` cannot hold is REPORTED, and the post still
    /// publishes.** `tags = ["a", "b"]` is the shape an author reaches for and
    /// the bag is strings only (EMBED §3), so the array goes nowhere — and
    /// until 2026-09-16 it went nowhere silently, at the authoring boundary,
    /// while `entity-core-papers`' own gate warned about the same loss. *A
    /// counterpart's restatement being louder than the authority it restates is
    /// a defect in the authority.*
    ///
    /// Both halves are asserted, because each has a different wrong
    /// implementation: report and refuse (a conformant post made unpublishable
    /// over an attribute that is not part of its meaning), or carry and say
    /// nothing (today's defect).
    #[test]
    fn an_attribute_the_bag_cannot_hold_is_reported_and_the_post_still_publishes() {
        let (entry, _, dropped) = read_post(
            &post("created_at = 2026-09-10\ntags = [\"a\", \"b\"]\ndraft = false", "body"),
            "QmA",
            &scratch(),
        )
        .unwrap();
        assert_eq!(entry.body.data.fallback, "body", "the post is still an entry");
        assert!(!entry.body.data.params.contains_key("tags"));

        // Named, and named individually — a count would tell an author that
        // something was lost without telling them what.
        assert_eq!(dropped.len(), 2, "one line per key, not one per post: {dropped:?}");
        assert!(dropped.iter().any(|d| d.contains("`tags`") && d.contains("array")), "{dropped:?}");
        assert!(
            dropped.iter().any(|d| d.contains("`draft`") && d.contains("boolean")),
            "{dropped:?}"
        );
    }

    /// With no title the fallback is the post's first line — a real sentence,
    /// which is what §6's degradation ladder is for.
    #[test]
    fn a_post_with_no_title_falls_back_to_its_first_line() {
        let (entry, _, _) = read_post(
            &post("created_at = 2026-09-10", "\n\n# A heading\n\nand then a paragraph."),
            "QmA",
            &scratch(),
        )
        .unwrap();
        assert_eq!(entry.body.data.fallback, "# A heading");
    }

    /// The whole seam, through a real directory and a real tree, read back by
    /// the axis reader that publish consumes. **Nested and unsorted on disk**,
    /// because a publish is a function of the posts and not of readdir order.
    #[test]
    fn a_directory_of_posts_becomes_a_feed_the_axis_reader_can_publish() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("2026/09")).unwrap();
        std::fs::write(
            dir.path().join("zeta.md"),
            post("created_at = 2026-09-10T09:00:00Z\ntitle = \"Third\"", "c"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("2026/09/alpha.md"),
            post("created_at = 2026-09-08T09:00:00Z\ntitle = \"First\"", "a"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("2026/09/beta.md"),
            post("created_at = 2026-09-09T09:00:00Z\ntitle = \"Second\"", "b"),
        )
        .unwrap();
        // Not a post: must be ignored, not refused.
        std::fs::write(dir.path().join("README.txt"), "not a post").unwrap();

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert_eq!(ingest_path(&peers, &pid, dir.path()).unwrap().posts, 3);

        let feed = crate::feed_tree::read_owned_feed(&peers, &pid).expect("the posts are a feed");
        let titles: Vec<&str> =
            feed.entries.iter().map(|e| e.body.data.fallback.as_str()).collect();
        assert_eq!(
            titles,
            vec!["First", "Second", "Third"],
            "oldest-first by the authored date, whatever the directory layout"
        );
        assert!(feed.entries.iter().all(|e| e.author == pid), "FEED-R1 through the seam");
    }

    #[test]
    fn a_directory_with_no_posts_is_an_error_and_not_a_silent_zero() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "nothing to publish").unwrap();
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let err = ingest_path(&peers, &pid, dir.path()).unwrap_err();
        assert!(err.contains("no *.md posts"), "{err}");
    }
}
