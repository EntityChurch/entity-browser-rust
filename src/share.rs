//! A **Share** — a titled grant.
//!
//! This module is the executable form of the claim in
//! `DESIGN-SHARE-FOLLOW-AND-THE-GRANT-AS-INTERFACE`: capability, peer
//! management and the extension roster are not three systems to integrate, they
//! are three projections of one tuple —
//!
//! ```text
//! GrantEntry { handlers, operations, resources, peers }
//!              which ext  which verb  what       who
//! ```
//!
//! A share is a **human title over one grant entry**. `resources` is what is
//! shared, `peers` is the audience. It is deliberately **not** a second,
//! softer permission system beside the real one: there is one authority model,
//! and a share is its label. That is why [`Share::grant`] exists and why every
//! audience question routes through [`Audience::peers_scope`] rather than
//! through a bespoke `is_visible_to` predicate — a predicate would be the
//! second system arriving by the back door.
//!
//! ## What this does and does not do today
//!
//! It derives and encodes. **Authoring the grant onto our own policy table is
//! the next slice** — `system/capability` is only registered on this peer as of
//! the Step 0 commit, and no surface writes a policy entry yet. Everything here
//! is therefore honest metadata that is *shaped* to become enforcement, which
//! is the point: when `debug_open_grants` goes away the cutover is a posture
//! flip, not a redesign. Do not read a `Peer` audience as a control today.
//!
//! ## Namespace — deliberately app-tier, deliberately provisional
//!
//! Shares live under `app/entity-browser/shares/`, **not** `system/share/`,
//! even though the design argues the latter is where they belong (a share only
//! one app's namespace can name is not shareable, and `entity-workbench-go`
//! put its own equivalent at `system/config/local/files/*`). Inventing a
//! `system/` convention unilaterally is exactly what AGENTS-STANDARD forbids —
//! *the spec is upstream; implementations implement, they don't define it.*
//! The question is routed as `ROUTING-2026-08-16-g` Q2. Until it is answered
//! the path lives in **one helper** ([`crate::app_paths::share_path`]) so the
//! migration is a single edit rather than a search.
//!
//! ## Groups are absent on purpose
//!
//! [`Audience`] has no `Group` variant. Whether a group in a `peers` scope
//! resolves at authoring time (expansion) or at check time (reference) is
//! arch Q7, and the two differ in whether leaving a group revokes access.
//! Shipping expansion would silently mean *"whoever was a member that day"* —
//! an absent option beats a lying one.

#![allow(dead_code)] // the writer/reader surfaces land in the next slice

use entity_capability::{GrantEntry, IdScope, PathScope};
use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;

use crate::file_offer::{FileOffer, NAMESPACE};

/// Entity type for a share manifest. A counterpart *lists this on our tree*, so
/// like the offer manifest it preceded, this is app-tier state read by
/// strangers — keep the decoder total (§[`decode_share`]).
pub const SHARE_TYPE: &str = "app/entity-browser/share";

// ---------------------------------------------------------------------------
// Kind / target / audience
// ---------------------------------------------------------------------------

/// What sort of thing is shared. A **rendering and listing hint, never an
/// authority input** — the grant is derived from [`ShareTarget`], so a peer
/// that does not recognise a kind can still list and follow the share. Same
/// forward-compat posture as the spec's unknown-binding rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    /// One file, hash-addressed — today's file offer.
    File,
    /// A content site: a tree prefix the receiver mirrors and browses.
    Site,
}

impl ShareKind {
    pub fn as_token(&self) -> &'static str {
        match self {
            ShareKind::File => "file",
            ShareKind::Site => "site",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "file" => Some(ShareKind::File),
            "site" => Some(ShareKind::Site),
            _ => None,
        }
    }
}

/// What the share points at. This — not [`ShareKind`] — is what the grant is
/// derived from, which is why an unknown kind is survivable and an unknown
/// target is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTarget {
    /// A content blob, by hash. The bytes live in `system/content`; the share
    /// is what gives the hash a filename.
    Blob(Hash),
    /// A tree prefix on our own peer, fully qualified and trailing-slashed.
    Prefix(String),
}

/// Who a share is for. Resolves to the `peers` axis of the grant, and nowhere
/// else — see the module doc on why there is no `is_visible_to` predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Anyone who can reach us. Today's offers are all of these.
    Public,
    /// Exactly one peer.
    Peer(String),
    /// Nobody but us — a published thing that is not yet handed out.
    SelfOnly,
}

impl Audience {
    /// The `peers` scope this audience becomes. `owner` is our own peer id,
    /// used only by [`Audience::SelfOnly`].
    pub fn peers_scope(&self, owner: &str) -> IdScope {
        match self {
            Audience::Public => IdScope::new(vec!["*".to_string()]),
            Audience::Peer(p) => IdScope::new(vec![p.clone()]),
            Audience::SelfOnly => IdScope::new(vec![owner.to_string()]),
        }
    }

    pub fn as_token(&self) -> String {
        match self {
            Audience::Public => "public".to_string(),
            Audience::Peer(p) => format!("peer:{p}"),
            Audience::SelfOnly => "self".to_string(),
        }
    }

    /// Decode an audience token. An **unrecognised token falls back to
    /// [`Audience::SelfOnly`]**, not to `Public`: a token we cannot read is a
    /// token we cannot honour, and the fail-closed direction is the one that
    /// does not hand a stranger a grant we did not understand.
    pub fn from_token(s: &str) -> Self {
        match s {
            "public" => Audience::Public,
            "self" => Audience::SelfOnly,
            other => match other.strip_prefix("peer:") {
                Some(p) if !p.is_empty() => Audience::Peer(p.to_string()),
                _ => Audience::SelfOnly,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The share
// ---------------------------------------------------------------------------

/// One published thing, with a title and an audience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    /// Human name. For a file this is its filename; for a site, its title.
    pub title: String,
    pub kind: ShareKind,
    pub target: ShareTarget,
    pub audience: Audience,
    /// Size in bytes where meaningful (files). `None` for a prefix share,
    /// whose size is not known without walking it.
    pub size: Option<u64>,
    /// The peer that published it — carried in the manifest so a pulled thing
    /// remembers where it came from after the listing is gone.
    pub from: String,
}

impl Share {
    /// The path segment this share is published under.
    ///
    /// A blob share keys by **hex of the blob hash** — content-derived, so
    /// re-sharing identical bytes is an idempotent overwrite rather than a
    /// duplicate row (the property today's offers already rely on). A prefix
    /// share keys by a slug of the prefix, for the same reason: re-sharing the
    /// same prefix must land on the same row.
    pub fn id(&self) -> String {
        match &self.target {
            ShareTarget::Blob(h) => h.to_hex(),
            ShareTarget::Prefix(p) => slug(p),
        }
    }

    /// **The interface, executable.** Derive the grant entry this share *is*.
    ///
    /// `owner` is our own peer id — the peer whose tree holds both the share
    /// manifest and the thing it points at.
    ///
    /// The scopes are exactly the two reads a puller performs, and they are the
    /// pair buildout item 22 names as uncovered: `system/tree` to find and read
    /// the manifest, `system/content` to fetch the bytes. Read-only by
    /// construction — there is no write operation in the scope and a share must
    /// never grow one, because "share" means "you may read this", and a
    /// stranger writing into our tree is a different feature with a different
    /// name.
    pub fn grant(&self, owner: &str) -> GrantEntry {
        let mut resources = vec![
            // The manifest itself, so a counterpart can read this row...
            crate::app_paths::share_path(crate::app_paths::APP_ID, owner, &self.id()),
            // ...and list the prefix to discover it in the first place.
            crate::app_paths::shares_prefix(crate::app_paths::APP_ID, owner),
        ];
        match &self.target {
            // Content is hash-addressed and served out of the namespace, so the
            // blob and its chunk closure are reachable through this one entry.
            ShareTarget::Blob(_) => {
                resources.push(format!("/{owner}/system/content/{NAMESPACE}"));
            }
            // A prefix share grants the subtree — and ONLY the subtree. The
            // `*` is appended to the trailing slash, so `/p/sites/blog/` does
            // not authorize `/p/sites/blog-private/`.
            ShareTarget::Prefix(p) => {
                let p = if p.ends_with('/') {
                    p.clone()
                } else {
                    format!("{p}/")
                };
                resources.push(format!("{p}*"));
                resources.push(format!("/{owner}/system/content/{NAMESPACE}"));
            }
        }

        GrantEntry {
            handlers: PathScope::new(vec![
                "system/tree".to_string(),
                "system/content".to_string(),
            ]),
            resources: PathScope::new(resources),
            operations: IdScope::new(vec!["get".to_string(), "list".to_string()]),
            peers: Some(self.audience.peers_scope(owner)),
            constraints: None,
            allowances: None,
        }
    }

    /// Express today's file offer as a share — the generalization, proven by
    /// construction rather than asserted. Offers are `Public` because that is
    /// what they have always been (`debug_open_grants`, buildout item 22).
    pub fn from_file_offer(offer: &FileOffer) -> Self {
        Share {
            title: offer.name.clone(),
            kind: ShareKind::File,
            target: ShareTarget::Blob(offer.blob),
            audience: Audience::Public,
            size: Some(offer.size),
            from: offer.from.clone(),
        }
    }

    /// The inverse, where it exists. `None` for anything that is not a blob
    /// share — a site has no single hash and cannot be squeezed into the older
    /// shape, which is precisely why the older shape needed generalizing.
    pub fn as_file_offer(&self) -> Option<FileOffer> {
        match &self.target {
            ShareTarget::Blob(h) => Some(FileOffer {
                name: self.title.clone(),
                size: self.size.unwrap_or(0),
                blob: *h,
                from: self.from.clone(),
            }),
            ShareTarget::Prefix(_) => None,
        }
    }
}

/// A tree prefix reduced to one safe path segment. Lowercase; every run of
/// non-alphanumerics collapses to a single hyphen. Same shape as
/// `entity-workbench-go`'s `sanitizeChainSlug`, deliberately — the two need to
/// agree if a share is ever to be named the same way on both sides.
fn slug(prefix: &str) -> String {
    let mut out = String::new();
    let mut last_hyphen = true;
    for c in prefix.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_hyphen = false;
        } else if !last_hyphen {
            out.push('-');
            last_hyphen = true;
        }
    }
    out.trim_matches('-').to_string()
}

// ---------------------------------------------------------------------------
// Manifest codec
// ---------------------------------------------------------------------------

/// Encode a share as its manifest entity.
pub fn share_entity(share: &Share) -> Result<Entity, String> {
    let mut fields = vec![
        (text("title"), text(share.title.clone())),
        (text("kind"), text(share.kind.as_token())),
        (text("audience"), text(share.audience.as_token())),
        (text("from"), text(share.from.clone())),
    ];
    match &share.target {
        ShareTarget::Blob(h) => fields.push((text("blob"), ecf_bytes(h.to_bytes()))),
        ShareTarget::Prefix(p) => fields.push((text("prefix"), text(p.clone()))),
    }
    if let Some(size) = share.size {
        fields.push((text("size"), integer(size as i64)));
    }
    Entity::new(SHARE_TYPE, to_ecf(&Value::Map(fields)))
        .map_err(|e| format!("share manifest: {e}"))
}

/// Decode a manifest entity. `None` for a wrong type or any malformed body —
/// a stranger's tree is untrusted input, so this never panics and never
/// half-fills a share.
///
/// An **unknown `kind` decodes to `None`** while an unknown *audience token*
/// falls back to `SelfOnly`. The asymmetry is deliberate: we cannot render or
/// follow a thing whose shape we do not know, but we can always narrow an
/// audience we do not understand. Widening that to "ignore unknown kinds and
/// show them anyway" needs the forward-compat listing behaviour designed
/// first, so it is left for when a third kind actually exists.
pub fn decode_share(entity: &Entity) -> Option<Share> {
    if entity.entity_type != SHARE_TYPE {
        return None;
    }
    let value: Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    let (mut title, mut kind, mut audience, mut from) = (None, None, None, None);
    let (mut blob, mut prefix, mut size) = (None, None, None);
    for (k, v) in map {
        match k.as_text() {
            Some("title") => title = v.as_text().map(str::to_string),
            Some("kind") => kind = v.as_text().and_then(ShareKind::from_token),
            Some("audience") => audience = v.as_text().map(Audience::from_token),
            Some("from") => from = v.as_text().map(str::to_string),
            Some("blob") => blob = v.as_bytes().and_then(|b| Hash::from_bytes(b).ok()),
            Some("prefix") => prefix = v.as_text().map(str::to_string),
            Some("size") => size = v.as_integer().and_then(|i| u64::try_from(i128::from(i)).ok()),
            _ => {}
        }
    }
    let target = match (blob, prefix) {
        (Some(h), _) => ShareTarget::Blob(h),
        (None, Some(p)) => ShareTarget::Prefix(p),
        (None, None) => return None,
    };
    Some(Share {
        title: title?,
        kind: kind?,
        target,
        audience: audience.unwrap_or(Audience::SelfOnly),
        size,
        from: from.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_hash() -> Hash {
        Hash::compute("system/content/blob", b"some file bytes")
    }

    fn file_share() -> Share {
        Share {
            title: "notes.txt".to_string(),
            kind: ShareKind::File,
            target: ShareTarget::Blob(blob_hash()),
            audience: Audience::Public,
            size: Some(4096),
            from: "MEPID".to_string(),
        }
    }

    fn site_share() -> Share {
        Share {
            title: "My blog".to_string(),
            kind: ShareKind::Site,
            target: ShareTarget::Prefix("/MEPID/sites/blog/".to_string()),
            audience: Audience::Peer("ALICE".to_string()),
            size: None,
            from: "MEPID".to_string(),
        }
    }

    #[test]
    fn a_share_round_trips_through_its_manifest() {
        for share in [file_share(), site_share()] {
            let entity = share_entity(&share).expect("encode");
            let back = decode_share(&entity).expect("decode");
            assert_eq!(back, share);
        }
    }

    #[test]
    fn a_wrong_type_or_malformed_body_decodes_to_none_rather_than_panicking() {
        let wrong = Entity::new("app/entity-browser/file-offer", to_ecf(&Value::Map(vec![])))
            .expect("entity");
        assert_eq!(decode_share(&wrong), None);

        // Right type, no target at all — a share pointing at nothing is not a
        // share, and a stranger can write exactly this.
        let targetless = Entity::new(
            SHARE_TYPE,
            to_ecf(&Value::Map(vec![
                (text("title"), text("x")),
                (text("kind"), text("file")),
            ])),
        )
        .expect("entity");
        assert_eq!(decode_share(&targetless), None);
    }

    #[test]
    fn an_unknown_kind_is_refused_but_an_unknown_audience_narrows_to_self() {
        let unknown_kind = Entity::new(
            SHARE_TYPE,
            to_ecf(&Value::Map(vec![
                (text("title"), text("x")),
                (text("kind"), text("hologram")),
                (text("prefix"), text("/P/x/")),
            ])),
        )
        .expect("entity");
        assert_eq!(decode_share(&unknown_kind), None, "unknown kind is refused");

        let unknown_audience = Entity::new(
            SHARE_TYPE,
            to_ecf(&Value::Map(vec![
                (text("title"), text("x")),
                (text("kind"), text("site")),
                (text("prefix"), text("/P/x/")),
                (text("audience"), text("everyone-forever")),
            ])),
        )
        .expect("entity");
        let decoded = decode_share(&unknown_audience).expect("decodes");
        assert_eq!(
            decoded.audience,
            Audience::SelfOnly,
            "an audience we cannot read must narrow, never widen"
        );
    }

    #[test]
    fn a_grant_is_read_only_and_names_both_handlers() {
        let g = file_share().grant("MEPID");
        assert!(g.handlers.include.contains(&"system/tree".to_string()));
        assert!(g.handlers.include.contains(&"system/content".to_string()));
        assert!(g.operations.include.contains(&"get".to_string()));
        assert!(g.operations.include.contains(&"list".to_string()));
        // The property that must never regress: sharing is reading.
        for forbidden in ["put", "write", "remove", "ingest", "*"] {
            assert!(
                !g.operations.include.contains(&forbidden.to_string()),
                "a share must not grant {forbidden}"
            );
        }
    }

    #[test]
    fn the_audience_is_the_peers_axis_and_nothing_else() {
        let public = file_share().grant("MEPID");
        assert_eq!(
            public.peers.as_ref().expect("peers scope").include,
            vec!["*".to_string()]
        );

        let mut narrowed = file_share();
        narrowed.audience = Audience::Peer("ALICE".to_string());
        let g = narrowed.grant("MEPID");
        assert_eq!(
            g.peers.as_ref().expect("peers scope").include,
            vec!["ALICE".to_string()]
        );
        // Narrowing the audience must change ONLY the peers axis — if it moved
        // resources too, the audience would have become a second scoping
        // mechanism, which is the thing this design exists to prevent.
        assert_eq!(g.resources.include, public.resources.include);
        assert_eq!(g.operations.include, public.operations.include);
    }

    #[test]
    fn a_prefix_share_does_not_authorize_a_sibling_prefix() {
        let g = site_share().grant("MEPID");
        assert!(
            g.resources.include.contains(&"/MEPID/sites/blog/*".to_string()),
            "the shared subtree is granted: {:?}",
            g.resources.include
        );
        // The trailing slash is load-bearing: `/sites/blog*` would also match
        // `/sites/blog-private/`. Assert the sibling is NOT reachable by any
        // granted resource pattern.
        for r in &g.resources.include {
            assert!(
                !"/MEPID/sites/blog-private/secret".starts_with(r.trim_end_matches('*')),
                "sibling prefix reachable via {r}"
            );
        }
    }

    #[test]
    fn a_file_offer_is_expressible_as_a_share_and_back() {
        let offer = FileOffer {
            name: "photo.jpg".to_string(),
            size: 900_000,
            blob: blob_hash(),
            from: "MEPID".to_string(),
        };
        let share = Share::from_file_offer(&offer);
        assert_eq!(share.kind, ShareKind::File);
        assert_eq!(share.id(), offer.id(), "same row, so a re-share overwrites");
        assert_eq!(share.as_file_offer().as_ref(), Some(&offer));
    }

    #[test]
    fn a_site_share_has_no_file_offer_form() {
        assert_eq!(
            site_share().as_file_offer(),
            None,
            "a prefix has no single hash — the reason the old shape needed generalizing"
        );
    }

    #[test]
    fn a_prefix_share_keys_by_a_stable_slug() {
        let a = site_share().id();
        let b = site_share().id();
        assert_eq!(a, b, "re-sharing the same prefix must land on the same row");
        assert!(!a.contains('/'), "the id must be one safe path segment: {a}");
        assert_eq!(a, "mepid-sites-blog");
    }
}
