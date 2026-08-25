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
//! shared; **the audience is the `grantee`, reached through [`policy_key`]** —
//! *not* the grant's `peers` axis, which is the network dimension ("against
//! whose namespace may this be spent"). We had that wrong until arch ruled Q6
//! on 2026-08-17; the trap is that the two things are both informally called a
//! "peer pattern" and V7 §6.2 says so explicitly.
//!
//! It is deliberately **not** a second, softer permission system beside the
//! real one: there is one authority model, and a share is its label. That is
//! why [`Share::grant`] exists and why every audience question routes through
//! [`policy_key`] rather than through a bespoke `is_visible_to` predicate — a
//! predicate would be the second system arriving by the back door.
//!
//! ## Publishing is a write; authoring is a reconcile
//!
//! Two halves that must not be fused, and fusing them is the mistake this
//! module already made once and reverted:
//!
//! - [`publish_share`] / [`withdraw_share`] write (or drop) the manifest. That
//!   is all they do, which is why they are callable from a spawned ingest task.
//! - [`ShareSync`] watches the shares prefix and re-authors the
//!   `system/capability` policy entries the *complete* current set implies.
//!
//! Authoring needs the whole set (the union rule, [`policy_entries`]) and on
//! the Worker arm a tree read is a cache mirror seeded **asynchronously**. A
//! write-at-publish would race that seeding, author a union computed from a
//! partial read, and never correct itself. A reconcile is idempotent and
//! converges: each seed event re-dirties the watch, the next pass sees more.
//!
//! **Enforcement is still off.** `debug_open_grants` is the posture, so these
//! grants are inert — honest metadata *shaped* to become enforcement, so the
//! cutover is a posture flip rather than a redesign. Do not read a `Peer`
//! audience as a control today.
//!
//! ## Namespace — the TYPE converges, the PATH does not (Q2, ruled)
//!
//! Arch ruled Q2 on 2026-08-17: shares are an **L5 application convention**,
//! and what the convention specifies is a **type vocabulary and no tree path**.
//! So [`SHARE_TYPE`] is `app/share/*` — the cross-impl contract, and the index
//! key for the `type_filter` query that aggregates across peers — while the
//! *path* (`app/entity-browser/shares/`, [`crate::app_paths::share_path`])
//! stays application-local and is nobody else's business.
//!
//! That split is not a compromise, it is the model: V7 §1.4 requires a mirror
//! of a remote peer's data to sit at *their* path verbatim under `/{them}/…`,
//! so a peer mirroring us copies **our** prefix. Path convergence is therefore
//! neither required nor wanted — *"paths are convention; the entity graph is
//! coherence."* (`REVIEW-THE-UNIVERSAL-NAMESPACE-AND-THE-SHAPE-OF-A-SHARE-2026-08-17`.)
//!
//! ## Groups (Q7, ruled — shippable, not yet shipped)
//!
//! [`Audience`] still has no `Group` variant, but the reason has changed. Q7 is
//! **ruled**: check-time resolution is foreclosed (an id-scope is matched as
//! literal identifiers — F40), and the shape is **per-member minted tokens,
//! revoked on leave**. That falls out of what this module already writes: one
//! policy entry per member, each minting a token whose `grantee` is that member.
//! The honestly-open half is the **join** side — a member who joins after
//! authoring gets nothing until someone re-authors — and any surface offering
//! groups must say so rather than imply it away.

#![allow(dead_code)] // site shares + the read-side listing land in the next slice

use entity_capability::{GrantEntry, IdScope, PathScope, POLICY_FALLBACK_SEGMENT};
use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;

use crate::file_offer::{FileOffer, NAMESPACE};

/// Entity type for a share manifest. A counterpart *lists this on our tree*, so
/// like the offer manifest it preceded, this is app-tier state read by
/// strangers — keep the decoder total (§[`decode_share`]).
///
/// **`app/share/*`, not `app/entity-browser/share`** — the L5 convention
/// namespace ruled by arch's `PROPOSAL-SHARE-AS-GRANT-AND-THE-AUDIENCE-CARRIER`
/// §2.1, following `app/embed/{media_type}` and `app/site-*`. A cross-impl
/// convention under one app's prefix is the reinvention the applications domain
/// exists to prevent.
///
/// **This is a TYPE tag, not a tree path**, and the distinction is load-bearing:
/// aggregation across peers is a `type_filter` query with no peer filter (the
/// shape `content_site::discovery::list_all_sites` already uses), so the *type*
/// is the cross-peer index key while the *path* stays application-local. Arch's
/// ruling is explicitly a type vocabulary and no tree path — see
/// `REVIEW-THE-UNIVERSAL-NAMESPACE-AND-THE-SHAPE-OF-A-SHARE-2026-08-17` §4.
pub const SHARE_TYPE: &str = "app/share/manifest";

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

/// Who a share is for. Resolves to the **policy key** ([`policy_key`]) — which
/// the capability handler turns into the `grantee` of a minted token — and
/// nowhere else. It does **not** reach the grant's `peers` axis; see the module
/// doc, and on why there is no `is_visible_to` predicate.
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
    // NOTE: there was a `peers_scope()` here, and its removal is the point.
    // It turned an audience into the grant's `peers` axis, which is the
    // network dimension and not an audience carrier at all — see the comment
    // at [`Share::grant`]. Do not reintroduce it: an audience reaches a grant
    // through [`policy_key`] and nowhere else, exactly as this module's doc
    // says of `is_visible_to`.

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
            // **`peers` is NOT the audience — it is the network dimension.**
            // V7 §5.2 matches it against
            // `target_peer = extract_peer(execute.data.uri, local_peer_id)` —
            // the peer whose namespace the request *addresses*. For a grant
            // over our own resources that is always `owner`, so populating it
            // with the audience made every named-audience share DENY with a
            // `403` that names nothing (verified: `check_permission`,
            // core-rust `core/capability/src/lib.rs`, defaults this scope to
            // `[local_peer_id]`; the inbound dispatch in
            // `core/peer/src/connection.rs` passes `target_peer = local_pid`).
            //
            // **The audience is carried by the policy key** — see
            // [`policy_key`] — which becomes the `grantee` of the token the
            // capability handler mints (`handle_request` mints with
            // `grantee: author`, and the policy table is keyed on the caller).
            // That is arch's "one minted token per member", produced by the
            // mechanism rather than hand-rolled. Q6, ruled 2026-08-17.
            //
            // Omitted rather than set to `[owner]`: absent means "the local
            // peer", which is exactly right and cannot drift if `owner` is
            // ever threaded differently.
            peers: None,
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

// ---------------------------------------------------------------------------
// Policy authoring — the union rule
// ---------------------------------------------------------------------------

/// The policy-table key an audience is filed under, or `None` when the audience
/// authorizes nobody.
///
/// **Not the same string as the grant's `peers` scope**, and conflating the two
/// is a trap: inside a [`GrantEntry`] a public audience is `peers: ["*"]`, but
/// the *policy path segment* has its own grammar
/// (`extensions/capability::is_valid_peer_pattern` — the literal
/// [`POLICY_FALLBACK_SEGMENT`] `"default"`, a §3.5 invariant-pointer hex, or a
/// Base58 PeerID). `"*"` is **not** a valid segment, so a public share files
/// under `default`, the entry the kernel falls back to for a peer with no
/// entry of its own.
///
/// [`Audience::SelfOnly`] returns `None`: there is nobody to author a policy
/// for, and writing an entry keyed by our own id would be authorizing ourselves
/// against our own tree.
pub fn policy_key(audience: &Audience) -> Option<String> {
    match audience {
        Audience::Public => Some(POLICY_FALLBACK_SEGMENT.to_string()),
        Audience::Peer(p) => Some(p.clone()),
        Audience::SelfOnly => None,
    }
}

/// Group every share into the policy entries they collectively imply:
/// `(policy_key, grants)`, sorted by key.
///
/// **This is the whole reason the function exists, and it is a silent-data-loss
/// guard.** `system/capability:configure` writes ONE entry per `peer_pattern`
/// and *replaces* it (`handle_configure` ends in `location_index.set`, storing
/// the params entity as-is). So a naive "author this share's grant" call is
/// last-write-wins: sharing a second file with the same audience would revoke
/// the first, and nothing would report it — the first share's row would still
/// be listed, still look shared, and no longer be readable.
///
/// The entry for a key is therefore the **union of the grants of every share
/// filed under it**, recomputed and rewritten whenever the share set changes —
/// on publish *and* on withdrawal. Callers must pass the complete share set,
/// never a delta.
pub fn policy_entries(shares: &[Share], owner: &str) -> Vec<(String, Vec<GrantEntry>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<GrantEntry>> =
        std::collections::BTreeMap::new();
    for share in shares {
        let Some(key) = policy_key(&share.audience) else {
            continue;
        };
        by_key.entry(key).or_default().push(share.grant(owner));
    }
    by_key.into_iter().collect()
}

/// Build the `configure`-op params entity for one policy key. Same body shape
/// as [`crate::backend_auth::build_authorize_params`] — a `grants` array plus
/// the `peer_pattern` the writer keys by — but authored on **our own** tree
/// rather than a backend's, which is the half buildout item 22 says is missing.
pub fn build_configure_params(
    policy_key: &str,
    grants: &[GrantEntry],
) -> Result<Entity, String> {
    let arr: Vec<Value> = grants.iter().map(entity_capability::encode_grant_entry).collect();
    let data = to_ecf(&Value::Map(vec![
        (text("grants"), Value::Array(arr)),
        (text("peer_pattern"), text(policy_key)),
        (text("ttl_ms"), integer(SHARE_TOKEN_TTL_MS as i64)),
    ]));
    Entity::new(entity_types::TYPE_CAP_POLICY_ENTRY, data)
        .map_err(|e| format!("configure params: {e}"))
}

/// How long a token minted from one of our share policy entries may live.
///
/// **This is the withdrawal latency on the `capability:request` path**, and it
/// is the only bound that exists there: V7 §6.2's `request` contract bounds a
/// minted token's `grants` and says nothing about its lifetime, §5.6's
/// child-must-not-outlive-parent rule does not reach a `request`-minted token
/// (it is a fresh root, `parent: null`), and a requester supplies its own
/// `ttl_ms`. So without a policy ceiling a peer whose own capability expires in
/// an hour can mint one valid for ten years — *temporal attenuation is the one
/// dimension a requester can escape*. Arch's
/// `PROPOSAL-CAPABILITY-MINT-TEMPORAL-CEILING-AND-THE-WITHDRAWAL-BOUND`.
///
/// **Honoured by core-py today and by nobody else** — py clamps with
/// `min(caller_expires, now + policy.ttl_ms, now + request.ttl_ms)`
/// (`capability.py`), which is §5.6's ROLE construction generalized; core-go
/// and core-rust do not clamp at all. Emitting it is therefore *free and
/// forward-compatible*: it binds on py now and on the others when they land the
/// ceiling. Emitting nothing binds nothing anywhere.
///
/// **One hour**, because a share is a read grant and re-requesting is cheap: a
/// whole transfer is bounded by `file_offer::MAX_OFFER_BYTES` (16 MiB) pulled
/// in ~4 MiB batches, so no legitimate operation comes close to the window,
/// while an hour keeps withdrawal latency short on the one path where the empty
/// policy entry cannot act retroactively.
///
/// **Do not quote this number in the UI** until the ceiling is normative — on
/// go and rust it currently bounds nothing, and a UI that promises an hour
/// while the substrate promises forever is the lie this constant exists to
/// avoid making. (`AGENTS.md`, share-withdrawal entry.)
pub const SHARE_TOKEN_TTL_MS: u64 = 60 * 60 * 1000;

/// The policy writes a share-set change implies for `keys` — **one entry per
/// requested key, including keys with no shares left**.
///
/// [`policy_entries`] only yields keys that still have shares, which makes it
/// exactly wrong for a withdrawal: the withdrawn share's key simply disappears
/// from the map, nothing rewrites it, and the stale entry keeps granting access
/// to a thing we just "stopped sharing". Same shape as the listing lesson this
/// repo already paid for — *a listing that only inserts cannot show a
/// withdrawal* (`FsBrowseCache::apply_offers`, which must apply an empty answer
/// too).
///
/// So callers name the affected keys and get a write for each, empty or not. An
/// empty `grants` array is a real, meaningful policy entry: *this peer pattern
/// is granted nothing.*
pub fn policy_writes_for(
    shares: &[Share],
    owner: &str,
    keys: &[String],
) -> Vec<(String, Vec<GrantEntry>)> {
    let live: std::collections::BTreeMap<String, Vec<GrantEntry>> =
        policy_entries(shares, owner).into_iter().collect();
    keys.iter()
        .map(|k| (k.clone(), live.get(k).cloned().unwrap_or_default()))
        .collect()
}

// ---------------------------------------------------------------------------
// Read / publish / withdraw
// ---------------------------------------------------------------------------

/// Every share this peer publishes, sorted by title (so the list does not
/// reshuffle under the cursor when one is added — tree order is by content
/// hash, which is effectively random).
///
/// **Worker-arm note:** a caller must subscribe [`crate::app_paths::shares_prefix`]
/// for this to be populated, because a tree read there is a cache mirror seeded
/// only for subscribed prefixes. The monolith cannot catch a missing
/// subscription (every window is open and two subscribe the whole tree), so a
/// surface reading this owes a lone-window test.
pub fn read_own_shares(peers: &crate::peers::Peers, peer_id: &str) -> Vec<Share> {
    let prefix = crate::app_paths::shares_prefix(crate::app_paths::APP_ID, peer_id);
    let mut out: Vec<Share> = peers
        .tree_listing(peer_id, &prefix)
        .into_iter()
        .filter_map(|entry| {
            let id = entry.path.strip_prefix(&prefix)?;
            if id.is_empty() || id.contains('/') {
                return None; // one level; defensive — it never nests today
            }
            decode_share(&peers.get_entity(peer_id, &entry.path)?)
        })
        .collect();
    out.sort_by(|a, b| a.title.cmp(&b.title).then(a.id().cmp(&b.id())));
    out
}

/// Publish a share: write its manifest. **That is all it does** — the grant is
/// authored by [`ShareSync`]'s reconcile, which the manifest write dirties.
///
/// The split is deliberate and it is what makes this callable from a spawned
/// ingest task. Authoring needs `&Peers` and the *complete* share set; a write
/// needs neither. Trying to do both here was the version that had to be
/// reverted (it read a possibly-unseeded Worker mirror and authored a union
/// computed from nothing). Publishing is a write; authoring is a reconcile.
pub fn publish_share(
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    share: &Share,
) -> Result<(), String> {
    writer.put(
        crate::app_paths::share_path(crate::app_paths::APP_ID, peer_id, &share.id()),
        share_entity(share)?,
    );
    Ok(())
}

/// Withdraw a share: drop its manifest. As with [`publish_share`], the policy
/// half is the reconcile's — the removal dirties the same prefix, the next pass
/// recomputes the union without this share, and a key whose last share just
/// went writes empty (which is what revocation consists of).
///
/// Withdrawal removes the *name* and, once the reconcile lands, the *grant*. It
/// does not unpublish the bytes: content is hash-addressed and `handle_get`
/// serves by hash without consulting the §6.4.2 presence binding, so a peer
/// holding the content id can still fetch it (buildout item 21). The surface
/// must keep saying "stop sharing", never "delete".
pub fn withdraw_share(
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    share_id: &str,
) {
    writer.remove(crate::app_paths::share_path(
        crate::app_paths::APP_ID,
        peer_id,
        share_id,
    ));
}

/// EXECUTE `system/capability:configure` on **our own** peer for each affected
/// key. Fire-and-forget per key: a failed policy write is logged loudly and
/// leaves the previous entry in place, which is the safe direction (the old
/// entry is never wider than what we already published).
///
/// ## Not yet called from a surface, and the reason is a design finding
///
/// Wiring this to the offer path was attempted and **reverted**, because the
/// union rule has a precondition the Worker arm does not meet: it needs the
/// **complete** share set, and [`read_own_shares`] there reads a cache mirror
/// seeded only for subscribed prefixes. Nothing subscribes
/// [`crate::app_paths::shares_prefix`] — the File Transfer window subscribes
/// `offers_prefix`, and only while it is open. So a publish-time call would
/// read an empty set, author a union computed from nothing, and silently drop
/// every previously published share's grant: **exactly the last-write-wins bug
/// the union exists to prevent**, reintroduced one layer up. Worse than not
/// wiring it.
///
/// Subscribing is necessary but not sufficient. Even with a watch in place the
/// Worker mirror seeds *asynchronously*, so the first read after subscribing
/// races it and would author a too-small union that never self-corrects — a
/// one-shot write derived from a converging read.
///
/// **The corrected shape: policy authoring is a watch-driven reconcile, not a
/// publish-time one-shot.** Hold an app-lifetime [`crate::window_watch::WindowWatch`]
/// on the shares prefix (the `user_themes::UserThemes` precedent — an
/// app-level, non-window watch), and re-author the affected keys whenever it is
/// dirty. Idempotent, converges as the mirror seeds, and self-heals a partial
/// read instead of freezing one into the policy table. That is the next slice,
/// and it is why this function is reachable only from tests today.
fn author_policy(
    peers: &crate::peers::Peers,
    peer_id: &str,
    shares: &[Share],
    keys: &[String],
) {
    for (key, grants) in policy_writes_for(shares, peer_id, keys) {
        let params = match build_configure_params(&key, &grants) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "share policy: encode failed");
                continue;
            }
        };
        let fut = peers.execute(
            peer_id,
            "system/capability".to_string(),
            "configure".to_string(),
            params,
            Default::default(),
        );
        let key_for_log = key.clone();
        let n = grants.len();
        crate::share::spawn_policy_write(fut, key_for_log, n);
    }
}

/// Spawn the configure future on whichever executor this target has. Split out
/// so the arm-specific spawn is in one place rather than at every call site.
#[cfg(target_arch = "wasm32")]
fn spawn_policy_write(
    fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>>>>,
    key: String,
    grants: usize,
) {
    wasm_bindgen_futures::spawn_local(async move {
        match fut.await {
            Ok(r) if r.status < 300 => {
                tracing::debug!(key = %key, grants, "share policy: authored")
            }
            Ok(r) => tracing::warn!(key = %key, status = r.status, "share policy: refused"),
            Err(e) => tracing::warn!(key = %key, error = %e, "share policy: failed"),
        }
    });
}

/// Native counterpart — a **no-op, and that is a stated coverage boundary, not
/// an oversight.** `EntityApp` is wasm-only; natively there is no frame loop to
/// spawn onto, so dropping the future here is the honest thing to do rather
/// than block a test thread on a peer that may not exist.
///
/// What this costs: the native suite covers the **pure** half — which keys get
/// written, which grants each carries, and that a withdrawal writes an empty
/// entry — and does **not** cover the EXECUTE itself landing on our own
/// `system/capability`. That half wants an e2e assertion reading
/// `system/capability/policy/*` back after a share, and it does not exist yet.
/// Do not read the green native tests as evidence the policy write happens.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_policy_write(
    fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + Send>>,
    key: String,
    grants: usize,
) {
    let _ = (fut, key, grants);
}

// ---------------------------------------------------------------------------
// The reconcile — policy authoring driven by the shares-prefix watch
// ---------------------------------------------------------------------------

/// Which policy keys a reconcile pass must write: every key that has shares
/// **now**, plus every key we authored **last time** that no longer does.
///
/// The second half is the whole point. A key whose last share was withdrawn
/// disappears from the live set, and if the pass only wrote live keys the old
/// entry would survive — still granting access to a thing that is no longer
/// shared. It has to be written empty, and only the previous pass knows it
/// existed.
///
/// Sorted and deduped, so a pass is deterministic and diffable in a log.
pub fn keys_to_author(
    live: &std::collections::BTreeSet<String>,
    previously_authored: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    live.union(previously_authored).cloned().collect()
}

/// App-held sync state: the shares-prefix watch plus the keys the last pass
/// authored. One instance for the app lifetime (its subscription lives as long
/// as it does — D9: that lifetime is the design, not a leak).
///
/// **Why a reconcile and not a write at publish time.** Authoring the policy
/// needs the *complete* share set (the union rule), and on the Worker arm a
/// tree read is a cache mirror seeded only for subscribed prefixes — seeded
/// **asynchronously**. A one-shot write at publish would race the seeding,
/// author a union computed from a partial read, and never correct itself. A
/// reconcile driven by the watch is idempotent and converges: each seed event
/// re-dirties, the next pass sees more, and the final pass sees everything.
/// This is the `user_themes::UserThemes` shape, for the same substrate reason.
///
/// **Scope, stated:** one peer — the one whose shares this app publishes. A
/// second local peer publishing its own shares would need its own watch, and
/// nothing does that today; when something does, this becomes a per-peer sweep
/// like `sync_peer_engines` rather than a second call site.
pub struct ShareSync {
    watch: crate::window_watch::WindowWatch,
    peer_id: String,
    /// Keys the previous pass wrote. Reconcile bookkeeping, not app state —
    /// losing it costs one redundant (idempotent) write, never a stale grant,
    /// because a fresh instance authors every live key anyway.
    authored: std::collections::BTreeSet<String>,
}

impl ShareSync {
    /// Subscribe the shares prefix. The watch starts dirty, so the first
    /// [`sync`](Self::sync) performs the boot pass.
    pub fn new(peers: &crate::peers::Peers, peer_id: &str) -> Self {
        let mut watch = crate::window_watch::WindowWatch::new();
        peers.watch_prefix(
            &mut watch,
            peer_id,
            crate::app_paths::shares_prefix(crate::app_paths::APP_ID, peer_id),
        );
        Self {
            watch,
            peer_id: peer_id.to_string(),
            authored: std::collections::BTreeSet::new(),
        }
    }

    /// Re-author the policy from the tree, if the shares prefix changed since
    /// the last call. Cheap no-op (one atomic check) otherwise.
    pub fn sync(&mut self, peers: &crate::peers::Peers) {
        if !self.watch.take_dirty() {
            return;
        }
        let shares = read_own_shares(peers, &self.peer_id);
        let live: std::collections::BTreeSet<String> = shares
            .iter()
            .filter_map(|s| policy_key(&s.audience))
            .collect();
        let keys = keys_to_author(&live, &self.authored);
        if keys.is_empty() {
            return;
        }
        tracing::debug!(
            peer = %self.peer_id,
            shares = shares.len(),
            keys = ?keys,
            "share policy: reconciling"
        );
        author_policy(peers, &self.peer_id, &shares, &keys);
        self.authored = live;
    }
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
    fn the_audience_never_reaches_the_grant_it_reaches_the_policy_key() {
        // This test previously asserted the OPPOSITE — that the audience *is*
        // the `peers` axis — and pinned a latent `403`. V7 §5.2 matches `peers`
        // against the peer the request ADDRESSES, which for a grant over our
        // own tree is always us; `peers: ["ALICE"]` therefore matches nothing
        // and denies every read it was meant to allow. Arch ruled Q6 on
        // 2026-08-17. Kept inverted rather than deleted, because a gate that
        // once asserted the defect is the one that must assert the fix.
        let public = file_share().grant("MEPID");
        assert!(
            public.peers.is_none(),
            "peers is the network dimension; absent means the local peer, which \
             is what a grant over our own resources means"
        );

        let mut narrowed = file_share();
        narrowed.audience = Audience::Peer("ALICE".to_string());
        let g = narrowed.grant("MEPID");

        // The audience changes NOTHING in the grant — not `peers`, not
        // `resources`, not `operations`. A share's grant says *what may be
        // read*; *who may read it* is the policy key below.
        assert!(g.peers.is_none(), "an audience must not populate `peers`");
        assert_eq!(g.resources.include, public.resources.include);
        assert_eq!(g.operations.include, public.operations.include);

        // ...and the audience is not lost — it lands on the policy key, which
        // is what the capability handler turns into a token's `grantee`.
        assert_eq!(policy_key(&Audience::Peer("ALICE".into())).as_deref(), Some("ALICE"));
        assert_eq!(
            policy_key(&Audience::Public).as_deref(),
            Some(POLICY_FALLBACK_SEGMENT)
        );
        assert_eq!(policy_key(&Audience::SelfOnly), None);

        // The grant must not carry ALICE anywhere at all — the encoding-level
        // statement of "the audience is not in the grant". This is what would
        // have caught the original defect.
        let encoded = format!("{:?}", g);
        assert!(
            !encoded.contains("ALICE"),
            "audience leaked into the grant entry: {encoded}"
        );
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
    fn two_shares_with_the_same_audience_share_one_policy_entry_and_both_survive() {
        // The guard against the trap in `policy_entries`' doc: `configure`
        // REPLACES the entry for a peer_pattern, so authoring share B's grant
        // alone would revoke share A. Both must appear in one entry.
        let a = file_share(); // Public
        let mut b = site_share();
        b.audience = Audience::Public;

        let entries = policy_entries(&[a.clone(), b.clone()], "MEPID");
        assert_eq!(entries.len(), 1, "one audience ⇒ one policy entry");
        let (key, grants) = &entries[0];
        assert_eq!(key, "default", "public files under the fallback segment");
        assert_eq!(grants.len(), 2, "both shares' grants are present");

        // And concretely: the site's subtree is still reachable after the file
        // share joined the same entry.
        let all: Vec<String> = grants.iter().flat_map(|g| g.resources.include.clone()).collect();
        assert!(all.contains(&"/MEPID/sites/blog/*".to_string()));
    }

    #[test]
    fn audiences_are_filed_under_separate_policy_keys() {
        let public = file_share();
        let alice = site_share(); // Audience::Peer("ALICE")
        let entries = policy_entries(&[public, alice], "MEPID");
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["ALICE", "default"], "sorted, and distinct");
        for (_, grants) in &entries {
            assert_eq!(grants.len(), 1);
        }
    }

    #[test]
    fn a_self_only_share_authors_no_policy_at_all() {
        let mut private = file_share();
        private.audience = Audience::SelfOnly;
        assert_eq!(policy_key(&private.audience), None);
        assert!(
            policy_entries(&[private], "MEPID").is_empty(),
            "nobody to authorize ⇒ no entry, not an entry keyed by ourselves"
        );
    }

    #[test]
    fn the_public_policy_key_is_default_not_the_grants_star() {
        // The two vocabularies must not be conflated. They are separately
        // named in V7 §6.2 ("they collide only in the informal name 'peer
        // pattern'"), and reaching for the wrong one is what produced the Q6
        // defect: `*` is a legal `peers` scope inside a grant and an ILLEGAL
        // policy path segment, where public is spelled `default`.
        let share = file_share();
        assert_eq!(policy_key(&share.audience).as_deref(), Some("default"));

        // And the resolution: we no longer populate `peers` at all, so the
        // only place a public audience is expressible is the policy key. This
        // assertion used to read `peers == ["*"]`; it is inverted for the same
        // reason as `the_audience_never_reaches_the_grant_it_reaches_the_policy_key`.
        assert!(
            share.grant("MEPID").peers.is_none(),
            "a public audience must reach the policy key, never the grant"
        );
    }

    #[test]
    fn configure_params_carry_the_key_and_every_grant() {
        let entries = policy_entries(&[file_share(), site_share()], "MEPID");
        for (key, grants) in &entries {
            let params = build_configure_params(key, grants).expect("encode");
            assert_eq!(params.entity_type, entity_types::TYPE_CAP_POLICY_ENTRY);
            // Round-trip through the same decoder the backend-auth path uses,
            // so a drift between the two authoring sites shows up here.
            let value: Value = ciborium::from_reader(params.data.as_slice()).expect("cbor");
            let map = value.as_map().expect("map");
            let pattern = map
                .iter()
                .find(|(k, _)| k.as_text() == Some("peer_pattern"))
                .and_then(|(_, v)| v.as_text())
                .expect("peer_pattern");
            assert_eq!(pattern, key);
            let n = map
                .iter()
                .find(|(k, _)| k.as_text() == Some("grants"))
                .and_then(|(_, v)| v.as_array())
                .map(|a| a.len())
                .expect("grants array");
            assert_eq!(n, grants.len());
        }
    }

    #[test]
    fn withdrawing_the_last_share_for_an_audience_still_writes_an_empty_entry() {
        // The trap: `policy_entries` drops a key with no shares, so a
        // withdrawal that relied on it would leave the OLD entry granting
        // access to the thing we just stopped sharing.
        let alice = site_share(); // Audience::Peer("ALICE")
        let key = policy_key(&alice.audience).expect("key");

        // Before: one grant for ALICE.
        let before = policy_writes_for(&[alice], "MEPID", &[key.clone()]);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].1.len(), 1);

        // After withdrawing it, the share set is empty — but the write must
        // still happen, and it must say "granted nothing".
        let after = policy_writes_for(&[], "MEPID", &[key.clone()]);
        assert_eq!(
            after.len(),
            1,
            "the key must still be written — a vanished key is a stale grant"
        );
        assert_eq!(after[0].0, key);
        assert!(
            after[0].1.is_empty(),
            "an empty grants array IS the revocation"
        );
    }

    #[test]
    fn withdrawing_one_of_two_shares_leaves_the_other_granted() {
        let a = file_share(); // Public
        let mut b = site_share();
        b.audience = Audience::Public;
        let key = "default".to_string();

        let after = policy_writes_for(&[a.clone()], "MEPID", &[key.clone()]);
        assert_eq!(after[0].1.len(), 1, "b withdrawn, a survives");
        let res: Vec<String> = after[0].1.iter().flat_map(|g| g.resources.include.clone()).collect();
        assert!(
            !res.contains(&"/MEPID/sites/blog/*".to_string()),
            "the withdrawn site must no longer be granted"
        );
    }

    fn keyset(items: &[&str]) -> std::collections::BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_reconcile_pass_rewrites_a_key_whose_last_share_was_withdrawn() {
        // Pass 1 authored `ALICE`; pass 2 has no shares for her. The key must
        // still be written (empty ⇒ revoked). If the pass only wrote live keys,
        // ALICE would keep the grant forever.
        let live = keyset(&["default"]);
        let previously = keyset(&["ALICE", "default"]);
        assert_eq!(
            keys_to_author(&live, &previously),
            vec!["ALICE".to_string(), "default".to_string()]
        );
    }

    #[test]
    fn a_first_pass_authors_every_live_key_with_no_history() {
        let live = keyset(&["ALICE", "default"]);
        let previously = std::collections::BTreeSet::new();
        assert_eq!(
            keys_to_author(&live, &previously),
            vec!["ALICE".to_string(), "default".to_string()],
            "losing the bookkeeping must never lose a grant"
        );
    }

    #[test]
    fn a_quiet_pass_with_nothing_ever_authored_writes_nothing() {
        let empty = std::collections::BTreeSet::new();
        assert!(
            keys_to_author(&empty, &empty).is_empty(),
            "no shares and no history ⇒ no policy traffic at all"
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
