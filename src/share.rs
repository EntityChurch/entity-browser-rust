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
//! So the type tags are `app/share/*` — the cross-impl contract, and the index
//! key for the `type_filter` query that aggregates across peers — while the
//! *path* (`app/entity-browser/shares/`, [`crate::app_paths::share_path`])
//! stays application-local and is nobody else's business.
//!
//! ## The audience model is a TYPE split, not a field value (§2.5, A-5/A-5b)
//!
//! We emitted `app/share/manifest` with a flat body for two months while
//! `APP-CONVENTION-SHARE` §2 declared `app/share/record`; that was the
//! application tier's only `divergent-family` — two seats, **zero tags in
//! common** — and it was ours to fix. Closed 2026-09-10.
//!
//! **It was never a rename.** The body diverged on four axes at once, and
//! shipping the new tag over the old body would have published entities that
//! *claim* to be `app/share/record` and fail to decode at a conformant reader —
//! strictly worse than the honest divergence, because the failure moves from
//! *"we cannot find your shares"* to *"your shares are corrupt"*:
//!
//! | axis | was | is |
//! |---|---|---|
//! | audience | a token string, `"public"` / `"peer:X"` / `"self"` | **the TYPE** ([`SHARE_PUBLICATION_TYPE`]) plus `[* audience-entry]` |
//! | target | flat sibling keys `blob` / `prefix` | one **tagged** union, §2.2 |
//! | provenance | a self-declared `from` field | the **namespace**, §4 |
//! | authorship time | absent | `created_at`, REQUIRED |
//!
//! **A public share is a different type, not an audience value** — and the
//! reason is that every in-field encoding breaks something already
//! load-bearing. The empty array is taken (*authored, no members yet* — which
//! is the **self-only** state, §2.2 / SHARE-7), a sentinel `grantee` makes that
//! field accept a value that is not a peer-id, and a sibling flag leaves two
//! fields able to disagree with no way for the schema to forbid it. §2.4
//! refused the identical move on the identical grounds when it declined to
//! unify `app/share/follow` with `app/feed/follow`.
//!
//! So [`Audience`] has **two** variants, and `Public` and *self-only* are
//! different variants rather than two values of one — SHARE-7 exists because
//! reading an empty audience as public is the intuitive-and-wrong answer, and a
//! type that cannot express the confusion cannot ship it.
//!
//! ## What we deliberately do NOT put on the wire
//!
//! **Only the declared fields are emitted.** V7 §2.6 makes unknown fields
//! MUST-ignore, so extension fields would interoperate — but SHARE-1/SHARE-2
//! assert that *"the record encoding is byte-stable cross-impl"*, and an
//! encoder that always adds a key of its own can never reproduce a joint
//! fixture's bytes. That is `G-PIN-4`'s lesson arriving one convention early:
//! whatever publishes first is the baseline, so publish the declared shape.
//!
//! Three fields left the wire and the first is an improvement, not a cost:
//!
//! - **`from`** — the publisher is the **namespace**. §4 requires a mirrored
//!   share to sit at `/{publisher}/…` verbatim, so the path is a *fact* where a
//!   `from` field is a stranger's *self-declared claim* about their own
//!   identity. [`decode_share`] therefore takes the publisher as an argument.
//! - **`kind`** — `File` / `Site` was derivable from the target tag in every
//!   case we construct, i.e. C15's two-expressions-of-one-fact. Callers match
//!   [`ShareTarget`].
//! - **`size`** — **a stated bound.** A blob share's size is no longer
//!   persisted. It is recoverable (the content store knows the blob's length)
//!   and a surface that wants it must look it up; today's live file-transfer
//!   listing reads `file_offer`'s own manifest, which is untouched.
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
use entity_ecf::{bytes as ecf_bytes, integer, text, to_ecf, uinteger, Value};
use entity_entity::Entity;
use entity_hash::Hash;

use crate::file_offer::{FileOffer, NAMESPACE};

/// `app/share/record` — a share **with** an audience (§2.2).
///
/// A counterpart *lists this on our tree*, so like the offer manifest it
/// preceded, this is app-tier state read by strangers — keep the decoder total
/// ([`decode_share`]).
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
pub const SHARE_RECORD_TYPE: &str = "app/share/record";

/// `app/share/publication` — a share with **no** audience (§2.5).
///
/// `share-record` minus `audience`, **and nothing else differs**: the audience
/// model is the only axis the split turns on, so it is the only field that
/// moves. Arch corrected their own first CDDL draft here, which had narrowed
/// `target` to a bare `content: content-hash` and would have re-blocked every
/// [`ShareTarget::Prefix`] share we publish.
///
/// **A publication carrying an `audience` field is invalid** and
/// [`decode_share`] refuses it — SHARE-8, and it is a refusal rather than a
/// tolerated extra key precisely because §2.6's MUST-ignore rule would
/// otherwise silently launder the one shape this type exists to exclude.
pub const SHARE_PUBLICATION_TYPE: &str = "app/share/publication";

/// `app/share/audience-entry` — one audience member's binding within a record
/// (§2.3). Carried **inline** in the record's `audience` array as an
/// entity-shaped `{type, data}` map, which is why it has a type tag despite
/// never being stored at a path of its own.
pub const AUDIENCE_ENTRY_TYPE: &str = "app/share/audience-entry";

// ---------------------------------------------------------------------------
// Target / audience
// ---------------------------------------------------------------------------

/// What the share points at, and what the grant is derived from — which is why
/// an unknown target is fatal to a decode where an unknown `via` is not.
///
/// **Tagged, per §2.2's `share-target`** — `{tag: "blob", hash}` /
/// `{tag: "prefix", path}`. The old encoding put `blob` and `prefix` at the top
/// level as sibling keys and inferred the variant from which one was present,
/// which is the untagged ambiguity the CDDL's comment rules out by hand.
///
/// It is deliberately **not** an `entity-ref`: a share is over the sharer's own
/// content on the sharer's own peer, so the authority term is already known and
/// naming it again would be a third place to put the wrong peer
/// (`APP-CONVENTION-REFERENCE` §3.4's implied-authority form, the rung `site:`
/// occupies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTarget {
    /// A content blob, by hash. The bytes live in `system/content`; the share
    /// is what gives the hash a filename.
    Blob(Hash),
    /// A tree prefix on our own peer, fully qualified and trailing-slashed.
    Prefix(String),
}

/// How an audience member came to be in the list. **Informative, for UI** —
/// §2.3 is explicit that `"group"` records that a group membership produced
/// this entry *at authoring time* and is **not** resolved at check time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudienceOrigin {
    Direct,
    Group,
}

impl AudienceOrigin {
    pub fn as_token(&self) -> &'static str {
        match self {
            AudienceOrigin::Direct => "direct",
            AudienceOrigin::Group => "group",
        }
    }

    /// `None` for an unrecognised token. Safe to drop because this field
    /// reaches no authority decision — it is a UI hint, and §2.3 closes the
    /// vocabulary at two values. Contrast the target, where an unreadable
    /// variant fails the whole decode.
    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "direct" => Some(AudienceOrigin::Direct),
            "group" => Some(AudienceOrigin::Group),
            _ => None,
        }
    }
}

/// One audience member — §2.3's `app/share/audience-entry`.
///
/// `grantee` is **the audience**: it matches the minted token's `grantee`, and
/// it is what [`policy_keys`] files the share under. There is no group
/// identifier anywhere in the check path (§3, and both alternatives are
/// foreclosed by landed core text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceMember {
    pub grantee: String,
    pub via: Option<AudienceOrigin>,
    /// ms since epoch, authoring peer's clock.
    pub added_at: u64,
}

impl AudienceMember {
    /// A directly-named member.
    pub fn direct(grantee: impl Into<String>, added_at: u64) -> Self {
        AudienceMember {
            grantee: grantee.into(),
            via: Some(AudienceOrigin::Direct),
            added_at,
        }
    }
}

/// Who a share is for — **and which of the two types it is**.
///
/// This is the axis the §2.5 split turns on, so it is not a field on the wire
/// at all: `Public` *is* [`SHARE_PUBLICATION_TYPE`] and `Direct` *is*
/// [`SHARE_RECORD_TYPE`]. An audience reaches the capability layer through
/// [`policy_keys`] and **nowhere else** — never the grant's `peers` axis, which
/// is the network dimension (see [`Share::grant`]), and never an
/// `is_visible_to` predicate, which would be a second authority system arriving
/// by the back door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Anyone who can reach us — **`app/share/publication`**. No audience, no
    /// per-member token, pull-only. Today's file offers are all of these.
    ///
    /// **The publisher cannot know who fetched it** and §2.5 forbids an
    /// implementation presenting it as though it can.
    Public,
    /// **`app/share/record`** — the members, in authored order.
    ///
    /// **EMPTY IS SELF-ONLY, NEVER PUBLIC.** §2.2 keeps the empty array's
    /// meaning as *an authored share with no members yet*, and SHARE-7 is the
    /// vector that makes reading it as public fail loudly. That is why `Public`
    /// is a sibling variant rather than a value in here: the confusion the
    /// vector guards against is not expressible in this type.
    Direct(Vec<AudienceMember>),
}

impl Audience {
    // NOTE: there was a `peers_scope()` here, and its removal is the point.
    // It turned an audience into the grant's `peers` axis, which is the
    // network dimension and not an audience carrier at all — see the comment
    // at [`Share::grant`]. Do not reintroduce it: an audience reaches a grant
    // through [`policy_keys`] and nowhere else, exactly as this module's doc
    // says of `is_visible_to`.

    /// Published, handed to nobody — a `record` with an empty audience.
    pub fn self_only() -> Self {
        Audience::Direct(Vec::new())
    }

    /// Exactly one named peer.
    pub fn peer(grantee: impl Into<String>, added_at: u64) -> Self {
        Audience::Direct(vec![AudienceMember::direct(grantee, added_at)])
    }

    /// Authored, with no members yet. **Distinct from [`Audience::Public`]**,
    /// and the whole reason SHARE-7 exists.
    pub fn is_self_only(&self) -> bool {
        matches!(self, Audience::Direct(m) if m.is_empty())
    }

    /// Which of the two type tags this audience is published under.
    pub fn entity_type(&self) -> &'static str {
        match self {
            Audience::Public => SHARE_PUBLICATION_TYPE,
            Audience::Direct(_) => SHARE_RECORD_TYPE,
        }
    }
}

// ---------------------------------------------------------------------------
// The share
// ---------------------------------------------------------------------------

/// One published thing, with a title and an audience.
///
/// **Two fields are not on the wire** and the module doc says why: `from` is
/// read back from the *namespace* rather than trusted from the body, and
/// `size` is no longer persisted at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    /// Human name. For a file this is its filename; for a site, its title.
    /// **Not an identifier** (§2.2) — [`Share::id`] is.
    pub title: String,
    pub target: ShareTarget,
    pub audience: Audience,
    /// Optional human-facing description (§2.2's `? note`). Carried so a
    /// decode/re-encode of another peer's record does not silently drop it.
    pub note: Option<String>,
    /// ms since epoch, sharer's clock. **REQUIRED on the wire**, so it is a
    /// field the caller supplies rather than something the codec invents —
    /// which keeps [`share_entity`] pure and every fixture deterministic.
    /// `EXTENSION-CLOCK` is local state; this is not a coordination point.
    pub created_at: u64,
    /// Size in bytes where meaningful (files). `None` for a prefix share, whose
    /// size is not known without walking it.
    ///
    /// **Not persisted** — see the module doc. A decode always yields `None`;
    /// it survives only in a `Share` built in-process from a
    /// [`crate::file_offer::FileOffer`].
    pub size: Option<u64>,
    /// The peer that published it.
    ///
    /// **Read from the namespace, never from the body.** §4 requires a
    /// mirrored share to live at `/{publisher}/…` verbatim, so the path is a
    /// fact about who authored it while a `from` field would be a stranger's
    /// self-declared claim about their own identity. [`decode_share`] takes it
    /// as an argument for exactly that reason.
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
    /// construction rather than asserted.
    ///
    /// Offers are `Public` because that is what they have always been
    /// (`debug_open_grants`, buildout item 22), so under §2.5 they publish as
    /// **`app/share/publication`**: no audience, no per-member token, pull-only,
    /// which is precisely what an open offer already was.
    ///
    /// `created_at` is a parameter rather than a `Date::now()` inside the
    /// constructor so that the codec has no clock in it and every fixture is
    /// deterministic — the same discipline `published_at` taught us when it made
    /// two runs of one publisher produce different signed heads.
    pub fn from_file_offer(offer: &FileOffer, created_at: u64) -> Self {
        Share {
            title: offer.name.clone(),
            target: ShareTarget::Blob(offer.blob),
            audience: Audience::Public,
            note: None,
            created_at,
            size: Some(offer.size),
            from: offer.from.clone(),
        }
    }

    /// The inverse, where it exists. `None` for anything that is not a blob
    /// share — a site has no single hash and cannot be squeezed into the older
    /// shape, which is precisely why the older shape needed generalizing.
    ///
    /// **`size` is `0` for a decoded share**, because it is no longer on the
    /// wire; this round-trips only a `Share` still holding its in-process
    /// value. A surface needing the real length asks the content store.
    pub fn as_file_offer(&self) -> Option<FileOffer> {
        match &self.target {
            ShareTarget::Blob(h) => Some(FileOffer {
                name: self.title.clone(),
                size: self.size.unwrap_or(0),
                blob: *h,
                from: self.from.clone(),
                source: None,
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

/// Why a share entity could not be read.
///
/// Three facts, kept apart on purpose (AP40). *"That is not a share"* is
/// ordinary — the shares prefix is ours, but a decoder is also the thing a
/// mirrored foreign subtree meets. *"That is a share and its body is broken"*
/// is a publisher bug worth a warning. *"That is a well-formed body its own type
/// forbids"* is the third, and it is the one SHARE-8 exists to make loud:
/// collapsing it into `Malformed` would report a deliberate schema violation as
/// a corrupt byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareDecodeError {
    /// Neither share type.
    NotAShare { entity_type: String },
    /// The right type, but the body is unreadable or a required field is
    /// missing or ill-typed.
    Malformed(&'static str),
    /// The right type, a readable body, and the body contradicts the schema
    /// that type stands for.
    InvalidForType(&'static str),
}

impl std::fmt::Display for ShareDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareDecodeError::NotAShare { entity_type } => {
                write!(f, "not a share entity: {entity_type}")
            }
            ShareDecodeError::Malformed(what) => write!(f, "malformed share: {what}"),
            ShareDecodeError::InvalidForType(what) => write!(f, "invalid for its type: {what}"),
        }
    }
}

/// §2.2's `share-target`, tagged.
fn target_value(target: &ShareTarget) -> Value {
    match target {
        ShareTarget::Blob(h) => Value::Map(vec![
            (text("tag"), text("blob")),
            // `content-hash = bstr`, SELF-DESCRIBING: `Hash::to_bytes` emits
            // the format-code varint followed by the digest, and the digest
            // length follows the code. Never a fixed 32 bytes — SHARE-5 is the
            // vector, and baking a width re-locks the cage the hash-agility arc
            // removed.
            (text("hash"), ecf_bytes(h.to_bytes())),
        ]),
        ShareTarget::Prefix(p) => Value::Map(vec![
            (text("tag"), text("prefix")),
            (text("path"), text(p.clone())),
        ]),
    }
}

fn decode_target(v: &Value) -> Result<ShareTarget, ShareDecodeError> {
    let map = v
        .as_map()
        .ok_or(ShareDecodeError::Malformed("target is not a map"))?;
    let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);
    let tag = field("tag")
        .and_then(|v| v.as_text())
        .ok_or(ShareDecodeError::Malformed("target has no tag"))?;
    match tag {
        "blob" => {
            let bytes = field("hash")
                .and_then(|v| v.as_bytes())
                .ok_or(ShareDecodeError::Malformed("blob target has no hash"))?;
            let hash = Hash::from_bytes(bytes)
                .map_err(|_| ShareDecodeError::Malformed("blob target hash is not a hash"))?;
            Ok(ShareTarget::Blob(hash))
        }
        "prefix" => {
            let path = field("path")
                .and_then(|v| v.as_text())
                .ok_or(ShareDecodeError::Malformed("prefix target has no path"))?;
            Ok(ShareTarget::Prefix(path.to_string()))
        }
        // A tag we do not know is NOT survivable: the target is what the grant
        // is derived from, so rendering a share whose extent we cannot compute
        // would be showing a row we could not honour.
        _ => Err(ShareDecodeError::Malformed("unknown target tag")),
    }
}

/// §2.3's `app/share/audience-entry`, carried inline as an entity-shaped map.
fn audience_entry_value(member: &AudienceMember) -> Value {
    let mut data = vec![(text("grantee"), text(member.grantee.clone()))];
    if let Some(via) = member.via {
        data.push((text("via"), text(via.as_token())));
    }
    data.push((text("added_at"), uinteger(member.added_at)));
    Value::Map(vec![
        (text("type"), text(AUDIENCE_ENTRY_TYPE)),
        (text("data"), Value::Map(data)),
    ])
}

/// One malformed member fails the **whole** record rather than yielding a short
/// audience. A partial audience is not a smaller truth: it either under-reports
/// who was granted access or over-reports it, and both directions author the
/// wrong policy. Same rule the window index already runs on.
fn decode_audience_entry(v: &Value) -> Result<AudienceMember, ShareDecodeError> {
    let outer = v
        .as_map()
        .ok_or(ShareDecodeError::Malformed("audience entry is not a map"))?;
    let field = |m: &[(Value, Value)], name: &str| {
        m.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v.clone())
    };
    let ty = field(outer, "type")
        .and_then(|v| v.as_text().map(str::to_string))
        .ok_or(ShareDecodeError::Malformed("audience entry has no type"))?;
    if ty != AUDIENCE_ENTRY_TYPE {
        return Err(ShareDecodeError::Malformed(
            "audience entry is not an app/share/audience-entry",
        ));
    }
    let data = field(outer, "data")
        .ok_or(ShareDecodeError::Malformed("audience entry has no data"))?;
    let data = data
        .as_map()
        .ok_or(ShareDecodeError::Malformed("audience entry data is not a map"))?;
    let grantee = field(data, "grantee")
        .and_then(|v| v.as_text().map(str::to_string))
        .ok_or(ShareDecodeError::Malformed("audience entry has no grantee"))?;
    if grantee.is_empty() {
        return Err(ShareDecodeError::Malformed("audience entry grantee is empty"));
    }
    let via = field(data, "via")
        .and_then(|v| v.as_text().and_then(AudienceOrigin::from_token));
    let added_at = field(data, "added_at")
        .and_then(|v| v.as_integer())
        .and_then(|i| u64::try_from(i128::from(i)).ok())
        .unwrap_or(0);
    Ok(AudienceMember {
        grantee,
        via,
        added_at,
    })
}

/// Encode a share as its entity — **`app/share/record` or
/// `app/share/publication`, decided by the audience** (§2.5).
///
/// Emits the declared fields and nothing else. See the module doc on why an
/// extension field would be interoperable and still wrong: SHARE-1/SHARE-2
/// assert byte-stability across implementations, and an encoder that always
/// adds a key of its own can never reproduce a joint fixture's bytes.
///
/// `to_ecf` canonicalizes map key order (length, then lexical) and the encoder
/// gets no say, so the order the fields are pushed here is not the order on the
/// wire — and must not be relied on by any test that means to pin the schema.
pub fn share_entity(share: &Share) -> Result<Entity, String> {
    let mut fields = vec![
        (text("title"), text(share.title.clone())),
        (text("target"), target_value(&share.target)),
    ];
    if let Audience::Direct(members) = &share.audience {
        fields.push((
            text("audience"),
            Value::Array(members.iter().map(audience_entry_value).collect()),
        ));
    }
    if let Some(note) = &share.note {
        fields.push((text("note"), text(note.clone())));
    }
    fields.push((text("created_at"), uinteger(share.created_at)));

    Entity::new(share.audience.entity_type(), to_ecf(&Value::Map(fields)))
        .map_err(|e| format!("share entity: {e}"))
}

/// Decode a share entity. A stranger's tree is untrusted input, so this never
/// panics and never half-fills a share.
///
/// **`publisher` comes from the namespace, not the body** — see the module doc.
/// §4 requires a mirrored share to sit at `/{publisher}/…` verbatim, so the
/// caller always has it, and taking it as an argument removes a field a
/// stranger could have lied in.
///
/// Two refusals worth knowing:
///
/// - **A publication carrying an `audience` is refused** (§2.5, SHARE-8). §2.6
///   would otherwise have us skip it silently as an unknown field, which is
///   exactly how the one shape this type exists to exclude would get laundered
///   into an ordinary-looking row.
/// - **An empty `audience` on a record decodes to self-only, never public**
///   (§2.2, SHARE-7). It is the same value the type carries for *authored, no
///   members yet*, and it is the intuitive-and-wrong reading that vector names.
pub fn decode_share(entity: &Entity, publisher: &str) -> Result<Share, ShareDecodeError> {
    let is_publication = match entity.entity_type.as_str() {
        SHARE_RECORD_TYPE => false,
        SHARE_PUBLICATION_TYPE => true,
        other => {
            return Err(ShareDecodeError::NotAShare {
                entity_type: other.to_string(),
            })
        }
    };

    let value: Value = ciborium::from_reader(entity.data.as_slice())
        .map_err(|_| ShareDecodeError::Malformed("body is not CBOR"))?;
    let map = value
        .as_map()
        .ok_or(ShareDecodeError::Malformed("body is not a map"))?;
    let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);

    let title = field("title")
        .and_then(|v| v.as_text().map(str::to_string))
        .ok_or(ShareDecodeError::Malformed("no title"))?;
    let target = decode_target(field("target").ok_or(ShareDecodeError::Malformed("no target"))?)?;
    let note = field("note").and_then(|v| v.as_text().map(str::to_string));
    let created_at = field("created_at")
        .and_then(|v| v.as_integer())
        .and_then(|i| u64::try_from(i128::from(i)).ok())
        .ok_or(ShareDecodeError::Malformed("no created_at"))?;

    let audience = match (is_publication, field("audience")) {
        (true, Some(_)) => {
            return Err(ShareDecodeError::InvalidForType(
                "an app/share/publication must not carry an audience",
            ))
        }
        (true, None) => Audience::Public,
        (false, Some(v)) => {
            let items = v
                .as_array()
                .ok_or(ShareDecodeError::Malformed("audience is not an array"))?;
            Audience::Direct(
                items
                    .iter()
                    .map(decode_audience_entry)
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        // §2.2 lists `audience` as required. An absent one is read as the same
        // state an empty one names — self-only — rather than refused, because
        // the fail-closed direction here is the one that shows the row to
        // nobody, and refusing outright would drop a publisher's own share off
        // their own listing over a field that carries no members either way.
        (false, None) => Audience::self_only(),
    };

    Ok(Share {
        title,
        target,
        audience,
        note,
        created_at,
        // Never on the wire. See the struct's field docs.
        size: None,
        from: publisher.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Policy authoring — the union rule
// ---------------------------------------------------------------------------

/// The policy-table keys an audience is filed under — **one per member**, and
/// empty when the audience authorizes nobody.
///
/// **Plural because §3 is plural.** *"A group audience is materialized at
/// authoring time as one `audience-entry` per member, each with its own minted
/// token whose `grantee` is that member"* — so a record with three members
/// implies three policy entries, and the singular version this replaced could
/// only ever express one. There is no group identifier anywhere in the check
/// path, and both alternatives that would put one there are foreclosed by
/// landed core text.
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
/// **A publication still files under `default`, and §2.5 is not contradicted by
/// that.** *"No audience and no grant"* is a statement about the audience
/// model — there is no per-member token and nobody to enumerate — not an
/// instruction to author nothing. On this substrate a read still passes
/// `check_permission`, so `default` is *how* the promise that follows it
/// (*"authorization at fetch is none required, pull-only"*) is delivered:
/// the fallback entry is what makes a stranger holding no token of their own
/// able to fetch. Authoring nothing would leave the bytes unreachable under
/// real enforcement, which is the opposite of what the type means.
///
/// A **self-only** audience returns no keys at all: there is nobody to author a
/// policy for, and writing an entry keyed by our own id would be authorizing
/// ourselves against our own tree.
pub fn policy_keys(audience: &Audience) -> Vec<String> {
    match audience {
        Audience::Public => vec![POLICY_FALLBACK_SEGMENT.to_string()],
        Audience::Direct(members) => members.iter().map(|m| m.grantee.clone()).collect(),
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
pub fn policy_entries(
    shares: &[Share],
    owner: &str,
    request_baseline: &[GrantEntry],
) -> Vec<(String, Vec<GrantEntry>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<GrantEntry>> =
        std::collections::BTreeMap::new();
    for share in shares {
        // One grant per member: a record naming three peers authorizes each of
        // them separately, which is §3's per-member-token model and not a
        // group identifier smuggled into a key.
        for key in policy_keys(&share.audience) {
            by_key.entry(key).or_default().push(share.grant(owner));
        }
    }
    union_request_baseline(&mut by_key, request_baseline);
    by_key.into_iter().collect()
}

/// **`APP-CONVENTION-SHARE` v0.2.1 §2.5's `[SHOULD]`, and it runs in the
/// opposite direction from everything else in this module.**
///
/// Every other rule here is about *granting* — this one is about not silently
/// *revoking* something we never granted.
///
/// `ENTITY-CORE-PROTOCOL` §6.2 makes the **same** `default` entry a **union
/// term** at §4.4's authenticate-response and a **per-peer ceiling** at
/// `system/capability:request`. Both are intended, and §6.2 also says that with
/// **no** entry the request-time flow *"works by skipping the policy ceiling —
/// step 3 only enforces bounds that exist."*
///
/// > **So writing a `default` entry where a deployment had none converts *no
/// > request-time ceiling* into *this request-time ceiling*, for every peer
/// > holding no entry of its own.**
///
/// **The failure is silent in the direction that hides it:** the publication
/// becomes fetchable, so the change looks right, while unrelated `request`s
/// from unlisted peers start failing subset-validation. That is the
/// 13/13 → 7 P/6 F shape the kernel's own comment records, arriving through the
/// fix rather than through the ambiguity. `SHARE-10` is the vector, and **both
/// its halves are required** — a run asserting only *the publication is
/// fetchable* reports success while the regression is live.
///
/// **The rule, stated minimally: if we write the fallback key at all, we must
/// not write it NARROWER than what the deployment already intends to allow.**
/// We do not author the entry when we have nothing to say, because an absent
/// entry is a ceiling of *nothing at all*, which is the safe direction.
///
/// **Nothing supplies a baseline today, and that is measured rather than
/// assumed:** `share::author_policy` is the only writer of any policy entry on
/// our own peer (`backend_auth` writes to a *backend* peer, keyed by that
/// peer's id, never `default`). A deployment that grows a second writer must
/// thread its intent through here — this is the seam, and the alternative
/// (reading the live entry back before replacing it) is a sync read of a cache
/// mirror on the Worker arm, which is AP41's shape, plus an unanswerable
/// question about which grants in it were ours.
fn union_request_baseline(
    by_key: &mut std::collections::BTreeMap<String, Vec<GrantEntry>>,
    request_baseline: &[GrantEntry],
) {
    if request_baseline.is_empty() {
        return;
    }
    // ONLY when the fallback key is already being written. Authoring it
    // otherwise would install a ceiling on a deployment that had none — the
    // exact defect this function exists to avoid, committed by the guard
    // meant to prevent it.
    let Some(grants) = by_key.get_mut(POLICY_FALLBACK_SEGMENT) else {
        return;
    };
    for entry in request_baseline {
        if !grants.contains(entry) {
            grants.push(entry.clone());
        }
    }
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
/// **The withdrawal path is where §2.5's `[SHOULD]` bites hardest, and it is
/// sharper than `SHARE-10` as written.** `SHARE-10` is about *creating* a
/// `default` entry; this function *empties* one, which is the same hazard at
/// full strength — an empty grants array is a ceiling that permits **nothing**,
/// so every unlisted peer's `request` fails subset-validation.
///
/// That is right for a named peer (an empty entry **is** the revocation) and
/// wrong for the fallback segment, where *revoked* and *no ceiling* are
/// different states and only one of them is expressible by writing.
/// [`union_request_baseline`] is what keeps the floor: with a baseline, the
/// last withdrawal writes the baseline rather than nothing.
///
/// **Stated bound, and it is routed rather than decided here:** with **no**
/// baseline the last withdrawal still writes an empty `default`. The correct
/// end state is *no entry at all* — which restores both properties — and
/// `system/capability:configure` has no removal verb, so reaching it is a
/// convention question rather than a call-site fix.
pub fn policy_writes_for(
    shares: &[Share],
    owner: &str,
    keys: &[String],
    request_baseline: &[GrantEntry],
) -> Vec<(String, Vec<GrantEntry>)> {
    let live: std::collections::BTreeMap<String, Vec<GrantEntry>> =
        policy_entries(shares, owner, request_baseline).into_iter().collect();
    let mut out: std::collections::BTreeMap<String, Vec<GrantEntry>> = keys
        .iter()
        .map(|k| (k.clone(), live.get(k).cloned().unwrap_or_default()))
        .collect();
    // Re-apply to the keys the caller named, so a `default` being written
    // EMPTY (every publication withdrawn) still carries the deployment's
    // request baseline. `policy_entries` above cannot do it — with no public
    // shares left, `default` is not one of its keys at all.
    union_request_baseline(&mut out, request_baseline);
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Read / publish / withdraw
// ---------------------------------------------------------------------------

/// Wall-clock ms since the epoch, for [`Share::created_at`].
///
/// Deliberately **not** `app.rs`'s `now_ms`, which is `performance.now()` —
/// milliseconds since navigation start, not since the epoch. §2.2 says *ms
/// since epoch, sharer's clock*, and the two differ by about fifty-five years.
///
/// Same cfg pair as `selection::now_epoch_ms` and `connections::now_epoch_ms`.
/// Three copies of a two-line clock read is not C15's drift risk — there is no
/// rule here to express two ways — but it is the shape that becomes one, so if
/// a fourth appears it should be hoisted rather than copied again.
#[cfg(target_arch = "wasm32")]
pub fn now_epoch_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
            let entity = peers.get_entity(peer_id, &entry.path)?;
            // The publisher is the namespace we just read, which is what makes
            // it a fact rather than a claim (§4).
            match decode_share(&entity, peer_id) {
                Ok(share) => Some(share),
                // Reported rather than swallowed, and the three outcomes stay
                // apart. A share that will not decode is a share whose policy
                // this pass is about to omit — silence here is a grant quietly
                // not authored.
                Err(ShareDecodeError::NotAShare { entity_type }) => {
                    tracing::debug!(
                        path = %entry.path,
                        entity_type = %entity_type,
                        "shares: skipping a non-share entity under the shares prefix"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(path = %entry.path, error = %e, "shares: undecodable");
                    None
                }
            }
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
    request_baseline: &[GrantEntry],
) {
    for (key, grants) in policy_writes_for(shares, peer_id, keys, request_baseline) {
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
            .flat_map(|s| policy_keys(&s.audience))
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
        // **Empty, and that is a measured statement rather than a placeholder.**
        // §2.5's `[SHOULD]` says a `default` entry must be the union of the
        // publication's read grants with what the deployment already intends to
        // allow at `request`. On our own peer nothing else writes a policy
        // entry — `author_policy` here is the only writer, and `backend_auth`
        // writes to a *backend* peer keyed by that peer's id — so there is no
        // other intent to union in. `union_request_baseline` is the seam if a
        // second writer ever appears.
        author_policy(peers, &self.peer_id, &shares, &keys, &[]);
        self.authored = live;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATED: u64 = 1_760_000_000_000;
    const ADDED: u64 = 1_760_000_000_001;
    const ME: &str = "MEPID";

    fn blob_hash() -> Hash {
        Hash::compute("system/content/blob", b"some file bytes")
    }

    /// A public file share — so, an `app/share/publication`.
    fn file_share() -> Share {
        Share {
            title: "notes.txt".to_string(),
            target: ShareTarget::Blob(blob_hash()),
            audience: Audience::Public,
            note: None,
            created_at: CREATED,
            size: Some(4096),
            from: ME.to_string(),
        }
    }

    /// A site share to one named peer — so, an `app/share/record`.
    fn site_share() -> Share {
        Share {
            title: "My blog".to_string(),
            target: ShareTarget::Prefix("/MEPID/sites/blog/".to_string()),
            audience: Audience::peer("ALICE", ADDED),
            note: None,
            created_at: CREATED,
            size: None,
            from: ME.to_string(),
        }
    }

    /// Build a `{type, data}` entity from `data` field pairs, the way a
    /// stranger's publisher would.
    fn entity_of(ty: &str, data: Vec<(Value, Value)>) -> Entity {
        Entity::new(ty, to_ecf(&Value::Map(data))).expect("entity")
    }

    fn a_target() -> Value {
        Value::Map(vec![
            (text("tag"), text("prefix")),
            (text("path"), text("/P/x/")),
        ])
    }

    #[test]
    fn the_audience_decides_the_type_not_a_field() {
        // The §2.5 split, asserted at the only place it is observable from
        // outside: the entity's own type tag.
        assert_eq!(
            share_entity(&file_share()).expect("encode").entity_type,
            SHARE_PUBLICATION_TYPE,
            "a public share is a publication"
        );
        assert_eq!(
            share_entity(&site_share()).expect("encode").entity_type,
            SHARE_RECORD_TYPE,
            "an audience-bearing share is a record"
        );

        let mut mine = file_share();
        mine.audience = Audience::self_only();
        assert_eq!(
            share_entity(&mine).expect("encode").entity_type,
            SHARE_RECORD_TYPE,
            "self-only is a record with an empty audience, NOT a publication"
        );
    }

    #[test]
    fn a_share_round_trips_through_its_entity() {
        for share in [file_share(), site_share()] {
            let entity = share_entity(&share).expect("encode");
            let back = decode_share(&entity, ME).expect("decode");

            // Every declared field survives...
            assert_eq!(back.title, share.title);
            assert_eq!(back.target, share.target);
            assert_eq!(back.audience, share.audience);
            assert_eq!(back.note, share.note);
            assert_eq!(back.created_at, share.created_at);
            // ...and the publisher comes back, from the namespace.
            assert_eq!(back.from, ME);
        }
    }

    #[test]
    fn a_note_survives_the_round_trip_and_an_absent_one_stays_absent() {
        // `? note` is optional, so an absent note must not come back as
        // `Some("")` — that is a different value and it would render as an
        // empty description rather than none at all.
        let mut noted = site_share();
        noted.note = Some("drafts, not the live site".to_string());
        let back = decode_share(&share_entity(&noted).expect("encode"), ME).expect("decode");
        assert_eq!(back.note.as_deref(), Some("drafts, not the live site"));

        let back = decode_share(&share_entity(&site_share()).expect("encode"), ME).expect("decode");
        assert_eq!(back.note, None);
    }

    #[test]
    fn the_publisher_is_the_namespace_and_never_a_field_in_the_body() {
        // The old encoding carried `from` in the body, which made a stranger's
        // claim about their own identity indistinguishable from a fact. Assert
        // both halves: the field is gone from the wire, and a body that tries
        // to smuggle one back cannot override the namespace.
        let entity = share_entity(&file_share()).expect("encode");
        let value: Value = ciborium::from_reader(entity.data.as_slice()).expect("cbor");
        let keys: Vec<&str> = value
            .as_map()
            .expect("map")
            .iter()
            .filter_map(|(k, _)| k.as_text())
            .collect();
        assert!(!keys.contains(&"from"), "`from` must not be on the wire: {keys:?}");

        let liar = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
                (text("from"), text("SOMEONE-ELSE")),
            ],
        );
        let decoded = decode_share(&liar, "REAL-PUBLISHER").expect("decodes");
        assert_eq!(
            decoded.from, "REAL-PUBLISHER",
            "the namespace outranks anything the body says about who wrote it"
        );
    }

    #[test]
    fn only_the_declared_fields_reach_the_wire() {
        // SHARE-1/SHARE-2 assert the record encoding is byte-stable
        // cross-impl, which an encoder that always adds a key of its own can
        // never satisfy. This is the gate on that: the key SET, exactly.
        //
        // Spelled as literals rather than via the module's own constants — a
        // test written in the constants follows a rename and can never catch
        // one, which is the same reason the window-index schema is pinned by
        // literal.
        let publication = share_entity(&file_share()).expect("encode");
        let value: Value = ciborium::from_reader(publication.data.as_slice()).expect("cbor");
        let mut keys: Vec<&str> = value
            .as_map()
            .expect("map")
            .iter()
            .filter_map(|(k, _)| k.as_text())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["created_at", "target", "title"],
            "a publication carries exactly §2.5's fields — no kind, no from, no size"
        );

        let record = share_entity(&site_share()).expect("encode");
        let value: Value = ciborium::from_reader(record.data.as_slice()).expect("cbor");
        let mut keys: Vec<&str> = value
            .as_map()
            .expect("map")
            .iter()
            .filter_map(|(k, _)| k.as_text())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["audience", "created_at", "target", "title"],
            "a record is the same plus `audience` — §2.5's 'nothing else differs'"
        );
    }

    #[test]
    fn a_targets_variant_is_carried_by_a_tag_not_inferred_from_which_key_is_present() {
        // §2.2's `share-target` is TAGGED — "no untagged ambiguity". The old
        // encoding put `blob` and `prefix` at the top level as siblings and
        // guessed from which one it found.
        let entity = share_entity(&site_share()).expect("encode");
        let value: Value = ciborium::from_reader(entity.data.as_slice()).expect("cbor");
        let target = value
            .as_map()
            .expect("map")
            .iter()
            .find(|(k, _)| k.as_text() == Some("target"))
            .map(|(_, v)| v.clone())
            .expect("target");
        let fields: Vec<&str> = target
            .as_map()
            .expect("target is a map")
            .iter()
            .filter_map(|(k, _)| k.as_text())
            .collect();
        assert!(fields.contains(&"tag"), "the target must be tagged: {fields:?}");
        assert!(fields.contains(&"path"), "a prefix target names a path: {fields:?}");

        // And a tag we do not know is fatal, because the target is what the
        // grant is derived from — unlike `via`, which is a UI hint.
        let unknown = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![
                (text("title"), text("x")),
                (text("created_at"), uinteger(CREATED)),
                (
                    text("target"),
                    Value::Map(vec![
                        (text("tag"), text("hologram")),
                        (text("path"), text("/P/x/")),
                    ]),
                ),
            ],
        );
        assert_eq!(
            decode_share(&unknown, ME),
            Err(ShareDecodeError::Malformed("unknown target tag"))
        );
    }

    #[test]
    fn a_blob_targets_hash_is_self_describing_and_not_a_fixed_width() {
        // SHARE-5. `content-hash = bstr` carries the format code first and the
        // digest length follows from it; a 32-byte assumption re-locks the cage
        // the hash-agility arc removed.
        let entity = share_entity(&file_share()).expect("encode");
        let value: Value = ciborium::from_reader(entity.data.as_slice()).expect("cbor");
        let target = value
            .as_map()
            .expect("map")
            .iter()
            .find(|(k, _)| k.as_text() == Some("target"))
            .map(|(_, v)| v.clone())
            .expect("target");
        let hash_bytes = target
            .as_map()
            .expect("map")
            .iter()
            .find(|(k, _)| k.as_text() == Some("hash"))
            .and_then(|(_, v)| v.as_bytes())
            .expect("hash bstr")
            .to_vec();
        assert_ne!(
            hash_bytes.len(),
            32,
            "a bare 32-byte digest would mean the format code was dropped"
        );
        assert_eq!(
            Hash::from_bytes(&hash_bytes).expect("round-trips through the format code"),
            blob_hash()
        );
    }

    #[test]
    fn a_wrong_type_or_malformed_body_is_refused_rather_than_panicking() {
        let wrong = entity_of("app/entity-browser/file-offer", vec![]);
        assert_eq!(
            decode_share(&wrong, ME),
            Err(ShareDecodeError::NotAShare {
                entity_type: "app/entity-browser/file-offer".to_string()
            }),
            "not-a-share is its own outcome, not a malformed one"
        );

        // Right type, no target at all — a share pointing at nothing is not a
        // share, and a stranger can write exactly this.
        let targetless = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![
                (text("title"), text("x")),
                (text("created_at"), uinteger(CREATED)),
            ],
        );
        assert_eq!(
            decode_share(&targetless, ME),
            Err(ShareDecodeError::Malformed("no target"))
        );

        // `created_at` is REQUIRED, so its absence is malformed rather than a
        // silent zero — a share dated 1970 is a fact nobody authored.
        let undated = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![(text("title"), text("x")), (text("target"), a_target())],
        );
        assert_eq!(
            decode_share(&undated, ME),
            Err(ShareDecodeError::Malformed("no created_at"))
        );
    }

    #[test]
    fn a_publication_carrying_an_audience_is_refused_as_invalid_for_its_type() {
        // SHARE-8, and the reason it must be a REFUSAL: V7 §2.6 makes unknown
        // fields MUST-ignore, so the tolerant reading silently launders the one
        // shape this type exists to exclude into an ordinary-looking row.
        let smuggled = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
                (
                    text("audience"),
                    Value::Array(vec![audience_entry_value(&AudienceMember::direct(
                        "ALICE", ADDED,
                    ))]),
                ),
            ],
        );
        assert_eq!(
            decode_share(&smuggled, ME),
            Err(ShareDecodeError::InvalidForType(
                "an app/share/publication must not carry an audience"
            )),
            "and it is INVALID-FOR-TYPE, never merged into malformed — a \
             deliberate schema violation is not a corrupt byte"
        );

        // The other half of SHARE-8: a publication of a subtree is ordinary.
        let prefix_publication = entity_of(
            SHARE_PUBLICATION_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
            ],
        );
        let decoded = decode_share(&prefix_publication, ME).expect("accepted as ordinary");
        assert_eq!(decoded.audience, Audience::Public);
        assert_eq!(decoded.target, ShareTarget::Prefix("/P/x/".to_string()));
    }

    #[test]
    fn an_empty_audience_is_self_only_and_never_public() {
        // SHARE-7 — the state the split exists to keep distinct, and the
        // intuitive-and-wrong reading this vector is named for.
        let empty = entity_of(
            SHARE_RECORD_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
                (text("audience"), Value::Array(vec![])),
            ],
        );
        let decoded = decode_share(&empty, ME).expect("decodes");
        assert!(decoded.audience.is_self_only());
        assert_ne!(
            decoded.audience,
            Audience::Public,
            "an authored share with no members yet is NOT offered to everyone"
        );
        // And the consequence that makes it matter: nobody is authorized.
        assert!(policy_keys(&decoded.audience).is_empty());
    }

    #[test]
    fn a_malformed_audience_member_fails_the_whole_record() {
        // A partial audience is not a smaller truth — it either under-reports
        // or over-reports who was granted access, and both author the wrong
        // policy. Same rule the window index runs on.
        let mixed = entity_of(
            SHARE_RECORD_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
                (
                    text("audience"),
                    Value::Array(vec![
                        audience_entry_value(&AudienceMember::direct("ALICE", ADDED)),
                        // Right shape, wrong type tag.
                        Value::Map(vec![
                            (text("type"), text("app/share/record")),
                            (
                                text("data"),
                                Value::Map(vec![(text("grantee"), text("BOB"))]),
                            ),
                        ]),
                    ]),
                ),
            ],
        );
        assert!(
            matches!(decode_share(&mixed, ME), Err(ShareDecodeError::Malformed(_))),
            "one bad member must fail the record, not yield a shorter audience"
        );
    }

    #[test]
    fn an_unknown_via_is_dropped_but_the_member_survives() {
        // The asymmetry against the target tag: `via` is informative-for-UI and
        // reaches no authority decision, so an unreadable one costs a hint.
        // An unreadable target would cost us the extent the grant is built from.
        let entry = Value::Map(vec![
            (text("type"), text(AUDIENCE_ENTRY_TYPE)),
            (
                text("data"),
                Value::Map(vec![
                    (text("grantee"), text("ALICE")),
                    (text("via"), text("telepathy")),
                    (text("added_at"), uinteger(ADDED)),
                ]),
            ),
        ]);
        let e = entity_of(
            SHARE_RECORD_TYPE,
            vec![
                (text("title"), text("x")),
                (text("target"), a_target()),
                (text("created_at"), uinteger(CREATED)),
                (text("audience"), Value::Array(vec![entry])),
            ],
        );
        let decoded = decode_share(&e, ME).expect("decodes");
        let Audience::Direct(members) = &decoded.audience else {
            panic!("expected a record audience");
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].grantee, "ALICE");
        assert_eq!(members[0].via, None, "an unreadable hint is dropped");
        assert_eq!(
            policy_keys(&decoded.audience),
            vec!["ALICE".to_string()],
            "and the member is still authorized — the hint was never the audience"
        );
    }

    #[test]
    fn a_records_audience_authorizes_every_member_separately() {
        // §3: a group audience is materialized as one entry per member, each
        // with its own minted token. The singular `policy_key` this replaced
        // could only ever express one of them.
        let mut share = site_share();
        share.audience = Audience::Direct(vec![
            AudienceMember::direct("ALICE", ADDED),
            AudienceMember::direct("BOB", ADDED),
            AudienceMember {
                grantee: "CAROL".to_string(),
                via: Some(AudienceOrigin::Group),
                added_at: ADDED,
            },
        ]);
        let back = decode_share(&share_entity(&share).expect("encode"), ME).expect("decode");
        assert_eq!(back.audience, share.audience, "all three survive the wire");

        let entries = policy_entries(&[share], ME, &[]);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["ALICE", "BOB", "CAROL"], "one entry per member");
        for (_, grants) in &entries {
            assert_eq!(grants.len(), 1, "each member gets the share's grant");
        }
    }

    /// An entity **exactly as the pre-0b build wrote it** — `app/share/manifest`
    /// with a flat body, a `kind` token, an audience token, a `from` field and
    /// sibling `blob`/`prefix` keys.
    ///
    /// Hand-built from literals on purpose. The standing check the `app/site-asset`
    /// arc earned is *decode something a PREVIOUS BUILD wrote before you believe
    /// any gate*, and its root cause was that **every fixture in every gate had
    /// been written by the new encoder**. A fixture produced by calling anything
    /// in this module today would reproduce that hole exactly.
    fn pre_0b_manifest() -> Entity {
        Entity::new(
            "app/share/manifest",
            to_ecf(&Value::Map(vec![
                (text("title"), text("notes.txt")),
                (text("kind"), text("file")),
                (text("audience"), text("public")),
                (text("from"), text(ME)),
                (text("blob"), ecf_bytes(blob_hash().to_bytes())),
                (text("size"), integer(4096)),
            ])),
        )
        .expect("entity")
    }

    #[test]
    fn a_share_written_by_the_previous_build_is_refused_not_silently_emptied() {
        // THE FAILURE THIS RULES OUT is the one the asset audit found: a stale
        // shape decoding to a well-formed value carrying nothing, on the
        // SUCCESS path, indistinguishable from a real empty. Here the old tag
        // is not a share type at all, so it is a refusal — and `NotAShare` is
        // its own outcome, so a log can say which of the three happened.
        assert_eq!(
            decode_share(&pre_0b_manifest(), ME),
            Err(ShareDecodeError::NotAShare {
                entity_type: "app/share/manifest".to_string()
            })
        );

        // DELIBERATELY NO LEGACY READ, and the discriminating question is the
        // asset audit's own: WHICH ARM SERVES THE STALE COPY? Here, none.
        // Nothing in the product decodes a share entity — the read-side listing
        // is the next slice — so an old row is inert rather than stale, and the
        // live file-transfer listing reads `file_offer`'s own manifest, which
        // this change does not touch. Contrast the asset case, where a
        // cache-before-network arm meant a returning visitor could never heal.
        //
        // The cost of a legacy read would be concrete: the retired tag would
        // stay in product code, and `implemented-undeclared app/share/manifest`
        // is exactly the debt this phase exists to pay.
    }

    #[test]
    fn an_old_row_is_skipped_by_the_reconcile_rather_than_authoring_a_wrong_union() {
        // The consequence that matters, since the policy union is the one thing
        // a share entity still drives. An undecodable row contributes NO key,
        // so it cannot widen anyone's access — the fail-closed direction.
        //
        // STATED BOUND, not a claim of cleanliness: a policy entry an old build
        // already authored is not swept by this, because `ShareSync::authored`
        // starts empty each session and only rewrites keys it has seen. That is
        // unchanged by this commit and it is inert under `debug_open_grants`,
        // which is the posture — do not read it as a live grant.
        assert!(
            decode_share(&pre_0b_manifest(), ME).is_err(),
            "precondition: the old row does not decode"
        );
        assert!(
            policy_entries(&[], ME, &[]).is_empty(),
            "a share set that skipped the old row authors nothing for it"
        );
    }

    /// A stand-in for "what this deployment already intends to allow at
    /// `request`" — deliberately unlike any grant a share produces, so a test
    /// cannot pass by coincidence.
    fn request_baseline() -> Vec<GrantEntry> {
        vec![GrantEntry {
            handlers: PathScope::new(vec!["system/query".to_string()]),
            resources: PathScope::new(vec!["/MEPID/public/*".to_string()]),
            operations: IdScope::new(vec!["query".to_string()]),
            peers: None,
            constraints: None,
            allowances: None,
        }]
    }

    #[test]
    fn the_fallback_entry_is_never_written_narrower_than_the_deployments_own_intent() {
        // SHARE v0.2.1 §2.5's `[SHOULD]`. The `default` entry is a UNION TERM
        // at §4.4 and a CEILING at `request`, so an entry carrying only the
        // publication's read grants silently narrows what every unlisted peer
        // may request. Both facts have to hold at once, which is why this
        // asserts the publication's grant is STILL there.
        let base = request_baseline();
        let entries = policy_entries(&[file_share()], ME, &base);
        let (key, grants) = entries.iter().find(|(k, _)| k == "default").expect("default written");
        assert_eq!(key, "default");
        assert!(
            grants.contains(&base[0]),
            "the deployment's request baseline must survive: {grants:?}"
        );
        assert!(
            grants.iter().any(|g| g.handlers.include.contains(&"system/content".to_string())),
            "and the publication is still reachable — SHARE-10 needs BOTH halves, and a run \
             asserting only the fetch reports success while the regression is live"
        );
    }

    #[test]
    fn a_named_peers_entry_never_picks_up_the_fallback_baseline() {
        // The baseline is the FALLBACK segment's, because that is the entry
        // §6.2 resolves for a peer with no entry of its own. Unioning it into
        // ALICE's entry would hand a named peer authority the deployment
        // scoped to strangers — a widening, where the whole rule is about not
        // narrowing.
        let entries = policy_entries(&[site_share()], ME, &request_baseline());
        let (key, grants) = &entries[0];
        assert_eq!(key, "ALICE");
        assert!(
            !grants.contains(&request_baseline()[0]),
            "the fallback baseline must not reach a named peer's entry"
        );
    }

    #[test]
    fn withdrawing_the_last_publication_leaves_the_request_path_where_it_found_it() {
        // **The withdrawal path is SHARE-10 at full strength**, and it is
        // sharper than the vector as written: `SHARE-10` is about CREATING a
        // `default` entry, and this EMPTIES one. An empty grants array is a
        // ceiling that permits nothing, so every unlisted peer's `request`
        // fails subset-validation — right for a named peer, where empty IS the
        // revocation, and wrong here.
        let key = POLICY_FALLBACK_SEGMENT.to_string();
        let base = request_baseline();

        let after = policy_writes_for(&[], ME, &[key.clone()], &base);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].0, key);
        assert_eq!(
            after[0].1, base,
            "the last publication going must revoke the publication and NOTHING ELSE"
        );

        // **STATED BOUND, pinned rather than hidden.** With no baseline the
        // same withdrawal still writes an empty `default`. The correct end
        // state is *no entry at all* — absent is a ceiling of nothing, which
        // is the safe direction — and `system/capability:configure` has no
        // removal verb, so reaching it is a convention question, routed rather
        // than decided here. This assertion exists so the day it is answered,
        // the answer has a test to change.
        let unmitigated = policy_writes_for(&[], ME, &[key.clone()], &[]);
        assert!(
            unmitigated[0].1.is_empty(),
            "today's behaviour, and the reason the baseline matters"
        );
    }

    #[test]
    fn a_shares_size_is_not_persisted_and_the_decode_says_so() {
        // A STATED BOUND, pinned rather than hidden: `size` left the wire so
        // the encoding could be byte-stable cross-impl. A surface that needs a
        // blob's length asks the content store; the live file-transfer listing
        // reads `file_offer`'s own manifest, which is untouched.
        let share = file_share();
        assert_eq!(share.size, Some(4096), "it survives in-process");
        let back = decode_share(&share_entity(&share).expect("encode"), ME).expect("decode");
        assert_eq!(back.size, None, "and it does not survive the wire");
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
        narrowed.audience = Audience::peer("ALICE", ADDED);
        let g = narrowed.grant("MEPID");

        // The audience changes NOTHING in the grant — not `peers`, not
        // `resources`, not `operations`. A share's grant says *what may be
        // read*; *who may read it* is the policy key below.
        assert!(g.peers.is_none(), "an audience must not populate `peers`");
        assert_eq!(g.resources.include, public.resources.include);
        assert_eq!(g.operations.include, public.operations.include);

        // ...and the audience is not lost — it lands on the policy key, which
        // is what the capability handler turns into a token's `grantee`.
        assert_eq!(policy_keys(&Audience::peer("ALICE", ADDED)), vec!["ALICE"]);
        assert_eq!(
            policy_keys(&Audience::Public),
            vec![POLICY_FALLBACK_SEGMENT.to_string()]
        );
        assert!(policy_keys(&Audience::self_only()).is_empty());

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
            source: None,
        };
        let share = Share::from_file_offer(&offer, CREATED);
        assert!(
            matches!(share.target, ShareTarget::Blob(_)),
            "the target IS the kind — the separate `kind` field was a second \
             expression of one fact and left with the old encoding"
        );
        assert_eq!(
            share.audience,
            Audience::Public,
            "an open offer is pull-only with no per-member token, which is \
             what §2.5 calls a publication"
        );
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

        let entries = policy_entries(&[a.clone(), b.clone()], "MEPID", &[]);
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
        let alice = site_share(); // a record naming ALICE
        let entries = policy_entries(&[public, alice], "MEPID", &[]);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["ALICE", "default"], "sorted, and distinct");
        for (_, grants) in &entries {
            assert_eq!(grants.len(), 1);
        }
    }

    #[test]
    fn a_self_only_share_authors_no_policy_at_all() {
        let mut private = file_share();
        private.audience = Audience::self_only();
        assert!(policy_keys(&private.audience).is_empty());
        assert!(
            policy_entries(&[private], "MEPID", &[]).is_empty(),
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
        assert_eq!(policy_keys(&share.audience), vec!["default".to_string()]);

        // And note what this says about §2.5's "no grant": the sentence is
        // about the AUDIENCE MODEL — no member to enumerate, no token to mint —
        // not an instruction to author nothing. `default` is how the promise
        // that follows it, "none required, pull-only", is delivered on a
        // substrate where a read still passes `check_permission`.
        assert!(
            policy_entries(&[share.clone()], "MEPID", &[])
                .iter()
                .any(|(k, g)| k == "default" && !g.is_empty()),
            "a publication is still reachable — authoring nothing would leave \
             the bytes unreadable under real enforcement"
        );

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
        let entries = policy_entries(&[file_share(), site_share()], "MEPID", &[]);
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
        let alice = site_share(); // a record naming ALICE
        let key = policy_keys(&alice.audience).remove(0);

        // Before: one grant for ALICE.
        let before = policy_writes_for(&[alice], "MEPID", &[key.clone()], &[]);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].1.len(), 1);

        // After withdrawing it, the share set is empty — but the write must
        // still happen, and it must say "granted nothing".
        let after = policy_writes_for(&[], "MEPID", &[key.clone()], &[]);
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

        let after = policy_writes_for(&[a.clone()], "MEPID", &[key.clone()], &[]);
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
