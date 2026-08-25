//! Chat data model — the `app/chat` L5 format convention, as data.
//!
//! Tracks arch's DRAFT `PROPOSAL-APP-CONVENTION-CHAT.md`: a conversation is a
//! distributed, append-only, content-addressed message log. Each participant
//! authors their **own** messages into their **own** namespace
//! (`/{author}/app/chat/{conversation_id}/messages/{message_hash}`) and is the
//! sole authority for them — so there is no shared-write object and no
//! write-contention. A participant's *view* is the union of every participant's
//! messages for a `conversation_id` (§1.4 local view): complete for its own,
//! cached for others (that caching is the delivery layer — a later slice).
//!
//! This module is the **data layer only** (entity shapes + path convention +
//! round-trip), native-testable under `make test`. The window/UI and the
//! delivery wiring build on it in later slices.
//!
//! The paths use the shared `app/chat/` **convention** namespace — NOT
//! browser's own `app/entity-browser/` workspace — so any chat client that
//! follows the convention interoperates. That is why they live here, with the
//! convention, rather than in `app_paths.rs` (browser-specific namespaces).

use std::collections::HashSet;

use entity_entity::Entity;

use crate::peers::Peers;

/// The chat message entity type, per the convention (`app/chat/message`).
pub const MESSAGE_TYPE: &str = "app/chat/message";

/// Upper bound on one message body (chars). A chat line is short; this only
/// exists so a pathological multi-megabyte paste can't be hashed, stored,
/// delivered and rendered at full size. Generous enough never to clip real use.
const MAX_BODY_CHARS: usize = 8192;

/// The conversation genesis entity type (`app/chat/conversation`, §2).
// The genesis identity layer lands ahead of the delivery slice that binds a
// real conversation (mirrors how slice 1 landed the message model before the
// window) — so it is dead in the wasm binary until then.
#[allow(dead_code)]
pub const CONVERSATION_TYPE: &str = "app/chat/conversation";

/// Membership policy (§2 / §6). `closed` = a fixed roster (pure 1:1 or a fixed
/// group) — the only policy this slice exercises. `invite` / `open` are the
/// group growth path and ride EXTENSION-ROLE/GROUP authority (§6); not wired yet.
#[allow(dead_code)]
pub const POLICY_CLOSED: &str = "closed";

/// The slice-2 default conversation — a peer's own local scratch conversation,
/// so the window is useful standalone (compose → see your messages) before a
/// delivered multi-peer conversation is bound. It has a single participant (the
/// bound peer) and a fixed, non-content-addressed id, so it never collides with
/// a real genesis hash (which is a `content-hash` string, §2).
pub const DEFAULT_CONVERSATION: &str = "self";

/// Prefix under an author's namespace holding their messages for one
/// conversation. Trailing slash — a subscribe/list prefix.
///
/// `/{author}/app/chat/{conversation_id}/messages/`
pub fn conversation_messages_prefix(author: &str, conversation_id: &str) -> String {
    format!("/{}/app/chat/{}/messages/", author, conversation_id)
}

/// Path to one message in an author's namespace, keyed by its content hash.
///
/// `/{author}/app/chat/{conversation_id}/messages/{message_hash}`
pub fn message_path(author: &str, conversation_id: &str, message_hash: &str) -> String {
    format!(
        "{}{}",
        conversation_messages_prefix(author, conversation_id),
        message_hash
    )
}

/// The conversation genesis (§2 LOCKED shape) — the immutable identity object.
/// Content-addressed: [`conversation_id`](Conversation::conversation_id) is the
/// hash of [`to_entity`](Conversation::to_entity), stable forever. Two peers
/// agree on a conversation by agreeing on this hash (the SITE `site-root`
/// discipline). It deliberately carries **no message collection** (§4) —
/// messages are found by enumerating each participant's namespace, never a
/// central mutable list, so there is no shared-write object and no contention.
///
/// The creator authors the genesis once and shares it (content-addressed, so
/// the exact bytes travel); a joining peer reads its `content_hash` as the id.
/// The bytes are encoded as constructed (participants in the given order) — the
/// id is the hash of *those* bytes, faithful to what the creator authored.
//
// Identity layer — lands ahead of the delivery slice (see `CONVERSATION_TYPE`);
// dead in the wasm binary until a real conversation is bound.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The creating peer-id (§1.5 Base58).
    pub creator: String,
    /// Genesis wall-clock ms since epoch (informational; NOT an ordering
    /// authority — §4.3, the same clock-skew reality as `ChatMessage::sent_at`).
    pub created_at: u64,
    /// Membership policy: `closed` / `invite` / `open` (§2). This slice ships
    /// [`POLICY_CLOSED`] (fixed 1:1 roster).
    pub policy: String,
    /// The genesis roster (§2). A 1:1 is exactly two participants.
    pub initial_participants: Vec<String>,
    /// Optional human title.
    pub title: Option<String>,
}

#[allow(dead_code)]
impl Conversation {
    /// A `closed`-policy 1:1 conversation between two peers, stamped now. The
    /// roster is `[a, b]` in the given order (the creator's authored bytes).
    pub fn one_to_one(
        creator: impl Into<String>,
        other: impl Into<String>,
        created_at: u64,
    ) -> Self {
        let creator = creator.into();
        let other = other.into();
        Self {
            initial_participants: vec![creator.clone(), other],
            creator,
            created_at,
            policy: POLICY_CLOSED.to_string(),
            title: None,
        }
    }

    /// A `closed`-policy **room** with a fixed roster of N members (§6: a 1:1 is
    /// the degenerate two-member case of the same model). The creator is always
    /// a member and leads the roster; `others` follow in the given order,
    /// de-duplicated. A `closed` room needs **no** membership-op log and **no**
    /// ROLE/GROUP authority — every member is named at genesis, so it ships
    /// today and the §1.4 union read already unions all N namespaces. Dynamic
    /// join (a peer added *after* genesis) is the `invite`/`open` growth path —
    /// the membership-op log (§6, [`MembershipOp`]/[`fold_roster`]), which layers
    /// on top without changing this shape.
    pub fn room(
        creator: impl Into<String>,
        others: impl IntoIterator<Item = impl Into<String>>,
        created_at: u64,
    ) -> Self {
        let creator = creator.into();
        let mut initial_participants = vec![creator.clone()];
        for other in others {
            let other = other.into();
            if !initial_participants.contains(&other) {
                initial_participants.push(other);
            }
        }
        Self {
            initial_participants,
            creator,
            created_at,
            policy: POLICY_CLOSED.to_string(),
            title: None,
        }
    }

    /// Encode as an `app/chat/conversation` entity. The entity's content hash is
    /// the `conversation_id`. Keys are written in a fixed order (the same
    /// insertion-order discipline `ChatMessage::to_entity` uses), so encoding is
    /// deterministic for a given value.
    pub fn to_entity(&self) -> Entity {
        let participants = ciborium::Value::Array(
            self.initial_participants
                .iter()
                .cloned()
                .map(ciborium::Value::Text)
                .collect(),
        );
        let mut map: Vec<(ciborium::Value, ciborium::Value)> = vec![
            (
                ciborium::Value::Text("creator".into()),
                ciborium::Value::Text(self.creator.clone()),
            ),
            (
                ciborium::Value::Text("created_at".into()),
                ciborium::Value::Integer(self.created_at.into()),
            ),
            (
                ciborium::Value::Text("policy".into()),
                ciborium::Value::Text(self.policy.clone()),
            ),
            (
                ciborium::Value::Text("initial_participants".into()),
                participants,
            ),
        ];
        // `title` is optional in the CDDL — present only when set, so an
        // untitled conversation hashes the same whether `title` is `None` or
        // was never a field.
        if let Some(title) = &self.title {
            map.push((
                ciborium::Value::Text("title".into()),
                ciborium::Value::Text(title.clone()),
            ));
        }
        let mut buf = Vec::new();
        ciborium::into_writer(&ciborium::Value::Map(map), &mut buf)
            .expect("CBOR encode of Conversation");
        Entity::new(CONVERSATION_TYPE, buf).expect("Conversation entity well-formed")
    }

    /// The immutable `conversation_id` — the content hash of the genesis entity
    /// (§2). Stable forever for a given genesis value.
    pub fn conversation_id(&self) -> String {
        self.to_entity().content_hash.to_string()
    }

    /// Decode a `Conversation` from an entity. `None` on a type mismatch or a
    /// missing/ill-typed required field (a foreign/malformed entity is skipped,
    /// never panics — a reader's cache can hold anything).
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        if entity.entity_type != CONVERSATION_TYPE {
            return None;
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;

        let mut creator = None;
        let mut created_at = None;
        let mut policy = None;
        let mut initial_participants = None;
        let mut title = None;
        for (k, v) in map {
            match k.as_text() {
                Some("creator") => creator = v.as_text().map(str::to_string),
                Some("created_at") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        if n >= 0 {
                            created_at = Some(n as u64);
                        }
                    }
                }
                Some("policy") => policy = v.as_text().map(str::to_string),
                Some("initial_participants") => {
                    initial_participants = v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.as_text().map(str::to_string))
                            .collect::<Vec<_>>()
                    });
                }
                Some("title") => title = v.as_text().map(str::to_string),
                _ => {}
            }
        }
        Some(Self {
            creator: creator?,
            created_at: created_at?,
            policy: policy?,
            initial_participants: initial_participants?,
            title,
        })
    }
}

/// The **well-known** `conversation_id` for a 1:1 between two peers — a stable,
/// content-addressed id BOTH peers derive independently, with no genesis
/// exchange. The pair is sorted (so order doesn't matter) and `creator`/
/// `created_at` are canonicalized (first-sorted peer / 0), so "chat with peer X"
/// needs no round-trip to agree on the id. A *group* still uses a real shared
/// genesis (`Conversation::room`, real creator + time); this convenience is for
/// the 1:1 case only.
#[allow(dead_code)]
pub fn wellknown_one_to_one_id(a: &str, b: &str) -> String {
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    Conversation {
        creator: x.to_string(),
        created_at: 0,
        policy: POLICY_CLOSED.to_string(),
        initial_participants: vec![x.to_string(), y.to_string()],
        title: None,
    }
    .conversation_id()
}

/// The membership-op entity type (`app/chat/membership-op`, §3 LOCKED shape).
#[allow(dead_code)]
pub const MEMBERSHIP_OP_TYPE: &str = "app/chat/membership-op";

/// Prefix under an actor's namespace holding their membership-ops for one
/// conversation (§3 dispatch keys). Trailing slash — a subscribe/list prefix.
///
/// `/{actor}/app/chat/{conversation_id}/membership/`
#[allow(dead_code)]
pub fn conversation_membership_prefix(actor: &str, conversation_id: &str) -> String {
    format!("/{}/app/chat/{}/membership/", actor, conversation_id)
}

/// What a [`MembershipOp`] does to the roster (§3 `op` field).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipChange {
    /// Add `subject` to the roster.
    Add,
    /// Remove `subject` from the roster.
    Remove,
}

impl MembershipChange {
    fn as_str(self) -> &'static str {
        match self {
            MembershipChange::Add => "add",
            MembershipChange::Remove => "remove",
        }
    }
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "add" => Some(MembershipChange::Add),
            "remove" => Some(MembershipChange::Remove),
            _ => None,
        }
    }
}

/// One append-only membership operation (§3/§6) — how a roster *evolves* after
/// genesis, so 1:1 and group are the same model plus a log. Authored by `actor`
/// under `/{actor}/app/chat/{conversation_id}/membership/{op_hash}`; the current
/// roster is [`fold_roster`] over the genesis + the observed ops.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipOp {
    /// The conversation this op belongs to (the genesis content-hash).
    pub conversation_id: String,
    /// Add or remove.
    pub op: MembershipChange,
    /// The peer added or removed.
    pub subject: String,
    /// The peer that performed the op. MUST be authorized per the conversation
    /// `policy` (§6) — an unauthorized actor's op is fail-closed-ignored by the
    /// fold (§9), which is what keeps a roster fold deterministic (§5.10).
    pub actor: String,
    /// Author-clock ms (a display/ordering heuristic only — §4.3).
    pub at: u64,
    /// The actor's previous membership-op (per-actor causal chain, §3). Optional.
    pub prev: Option<String>,
}

#[allow(dead_code)]
impl MembershipOp {
    /// Encode as an `app/chat/membership-op` entity (deterministic insertion
    /// order, as elsewhere in this module). The content hash is the op id.
    pub fn to_entity(&self) -> Entity {
        let mut map: Vec<(ciborium::Value, ciborium::Value)> = vec![
            (
                ciborium::Value::Text("conversation_id".into()),
                ciborium::Value::Text(self.conversation_id.clone()),
            ),
            (
                ciborium::Value::Text("op".into()),
                ciborium::Value::Text(self.op.as_str().into()),
            ),
            (
                ciborium::Value::Text("subject".into()),
                ciborium::Value::Text(self.subject.clone()),
            ),
            (
                ciborium::Value::Text("actor".into()),
                ciborium::Value::Text(self.actor.clone()),
            ),
            (
                ciborium::Value::Text("at".into()),
                ciborium::Value::Integer(self.at.into()),
            ),
        ];
        if let Some(prev) = &self.prev {
            map.push((
                ciborium::Value::Text("prev".into()),
                ciborium::Value::Text(prev.clone()),
            ));
        }
        let mut buf = Vec::new();
        ciborium::into_writer(&ciborium::Value::Map(map), &mut buf)
            .expect("CBOR encode of MembershipOp");
        Entity::new(MEMBERSHIP_OP_TYPE, buf).expect("MembershipOp entity well-formed")
    }

    /// Decode a `MembershipOp`. `None` on type mismatch or a missing/ill-typed
    /// required field (a foreign/malformed entity under the prefix is skipped).
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        if entity.entity_type != MEMBERSHIP_OP_TYPE {
            return None;
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;

        let mut conversation_id = None;
        let mut op = None;
        let mut subject = None;
        let mut actor = None;
        let mut at = None;
        let mut prev = None;
        for (k, v) in map {
            match k.as_text() {
                Some("conversation_id") => conversation_id = v.as_text().map(str::to_string),
                Some("op") => op = v.as_text().and_then(MembershipChange::from_str),
                Some("subject") => subject = v.as_text().map(str::to_string),
                Some("actor") => actor = v.as_text().map(str::to_string),
                Some("at") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        if n >= 0 {
                            at = Some(n as u64);
                        }
                    }
                }
                Some("prev") => prev = v.as_text().map(str::to_string),
                _ => {}
            }
        }
        Some(Self {
            conversation_id: conversation_id?,
            op: op?,
            subject: subject?,
            actor: actor?,
            at: at?,
            prev,
        })
    }
}

/// The current roster (§6) — `fold(initial_participants, membership-ops)` — as
/// an ordered, de-duplicated participant list. This is the concrete answer to
/// "how do 3+ peers join a room": the genesis names the founding members, and
/// each subsequent join/leave is one appended, signed, content-addressed
/// `MembershipOp`; the roster is their fold, never a shared mutable object.
///
/// **Authorization floor (fail-closed, §6/§9).** Full `invite`/admin authority
/// is a conversation-scoped EXTENSION-ROLE/GROUP grant ([ASK-ARCH-CHAT-1],
/// deferred — not wired here). Until it lands, this fold applies only the ops
/// whose authority needs **no** grant, and ignores the rest (never a silent
/// over-grant):
/// - **`closed`** — no post-genesis authority at all; the roster is exactly
///   `initial_participants`, every op ignored.
/// - **`open`** — any peer may self-`add` (a broad self-grant) and self-`remove`
///   (leave). Acting on *another* peer needs the admin grant → ignored.
/// - **`invite`** — adds need the invite grant (not yet available) → ignored;
///   only self-`remove` (leave, needs no grant) applies.
///
/// Ops are applied in a deterministic order — `(at, op-hash)` — so the fold is
/// convergent regardless of the order ops were observed/delivered (§5.10), the
/// same discipline as revocation. `at` is only a heuristic (§4.3); the op-hash
/// tiebreak makes the order total and observation-independent.
#[allow(dead_code)]
pub fn fold_roster(genesis: &Conversation, ops: &[MembershipOp]) -> Vec<String> {
    // Deterministic, observation-independent order: (at, then op content-hash).
    // Precompute each op's content hash ONCE (B8) — recomputing it inside the
    // comparator re-encoded + hashed both operands on every comparison (O(n log n)
    // CBOR+hash).
    let mut ordered: Vec<(String, &MembershipOp)> = ops
        .iter()
        .filter(|o| o.conversation_id == genesis.conversation_id())
        .map(|o| (o.to_entity().content_hash.to_string(), o))
        .collect();
    ordered.sort_by(|a, b| a.1.at.cmp(&b.1.at).then_with(|| a.0.cmp(&b.0)));

    let mut roster: Vec<String> = genesis.initial_participants.clone();
    let closed = genesis.policy == POLICY_CLOSED;
    for (_, op) in ordered {
        if closed {
            continue; // no post-genesis authority
        }
        // The grant-free floor: an actor may only act on itself. Acting on
        // another peer needs an admin/invite grant (ROLE/GROUP, deferred), so it
        // is fail-closed-ignored here.
        if op.actor != op.subject {
            continue;
        }
        match op.op {
            MembershipChange::Add => {
                // `open` permits self-add; `invite` does not (needs the grant).
                if genesis.policy == "open" && !roster.contains(&op.subject) {
                    roster.push(op.subject.clone());
                }
            }
            MembershipChange::Remove => {
                // A self-leave needs no grant under any policy.
                roster.retain(|p| p != &op.subject);
            }
        }
    }
    roster
}

/// One chat message, as authored. Content-addressed: the hash of its
/// [`to_entity`](ChatMessage::to_entity) is its identity and its path key, so a
/// message is immutable — an edit is a new message (the append-only log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// The authoring peer-id (§1.5 Base58). The message lives under this peer's
    /// namespace, so `author` is redundant with the path — carried in the body
    /// too so a message stays self-describing when cached under a reader's tree.
    pub author: String,
    /// The conversation this belongs to (the immutable genesis content-hash).
    pub conversation_id: String,
    /// The message text. The convention allows a rich EMBED node here; this
    /// slice carries plain text (an EMBED `text` node is the trivial case).
    pub body: String,
    /// Author-stamped send time, ms since epoch. Informational, NOT an ordering
    /// authority (§4.3) — display may sort by it, but causality does not depend
    /// on it (clocks disagree across peers).
    pub sent_at: u64,
}

impl ChatMessage {
    pub fn new(
        author: impl Into<String>,
        conversation_id: impl Into<String>,
        body: impl Into<String>,
        sent_at: u64,
    ) -> Self {
        Self {
            author: author.into(),
            conversation_id: conversation_id.into(),
            body: body.into(),
            sent_at,
        }
    }

    /// Encode as an `app/chat/message` entity (deterministic CBOR map, the same
    /// shape `ShellState` uses). The entity's content hash is the message id.
    pub fn to_entity(&self) -> Entity {
        let map: Vec<(ciborium::Value, ciborium::Value)> = vec![
            (
                ciborium::Value::Text("author".into()),
                ciborium::Value::Text(self.author.clone()),
            ),
            (
                ciborium::Value::Text("conversation_id".into()),
                ciborium::Value::Text(self.conversation_id.clone()),
            ),
            (
                ciborium::Value::Text("body".into()),
                ciborium::Value::Text(self.body.clone()),
            ),
            (
                ciborium::Value::Text("sent_at".into()),
                ciborium::Value::Integer(self.sent_at.into()),
            ),
        ];
        let mut buf = Vec::new();
        ciborium::into_writer(&ciborium::Value::Map(map), &mut buf)
            .expect("CBOR encode of ChatMessage");
        Entity::new(MESSAGE_TYPE, buf).expect("ChatMessage entity well-formed")
    }

    /// Decode a `ChatMessage` from an entity. Returns `None` if the type is
    /// wrong or a required field is missing/ill-typed — a foreign or malformed
    /// entity under the messages prefix is skipped, never panics (a reader's
    /// cache can hold anything).
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        if entity.entity_type != MESSAGE_TYPE {
            return None;
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;

        let mut author = None;
        let mut conversation_id = None;
        let mut body = None;
        let mut sent_at = None;
        for (k, v) in map {
            match k.as_text() {
                Some("author") => author = v.as_text().map(str::to_string),
                Some("conversation_id") => conversation_id = v.as_text().map(str::to_string),
                Some("body") => body = v.as_text().map(str::to_string),
                Some("sent_at") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        if n >= 0 {
                            sent_at = Some(n as u64);
                        }
                    }
                }
                _ => {}
            }
        }
        Some(Self {
            author: author?,
            conversation_id: conversation_id?,
            body: body?,
            sent_at: sent_at?,
        })
    }
}

/// Author-stamped wall-clock time, ms since epoch. `web_time` is the
/// cross-platform shim (real `SystemTime` natively, `Date.now()` on wasm), so
/// this compiles and runs on both the native test path and in the browser.
fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The Chat window's model — the bound peer + the active conversation + the
/// in-progress compose draft. State that must survive a page reload (which
/// conversation) is structural and belongs in the tree; the messages
/// themselves ARE the tree, so this model holds no message copies — it reads
/// them live (subscription-driven) on each render.
pub struct ChatModel {
    peer_id: String,
    conversation_id: String,
    /// Every participant whose messages compose this conversation's view (§1.4
    /// union). The bound `peer_id` is always a member of its own view. For the
    /// default self-conversation this is just `[peer_id]`; a delivered 1:1
    /// genesis has both peers.
    participants: Vec<String>,
}

impl ChatModel {
    /// The default single-peer self-conversation (slice 2 behavior): one
    /// participant (the bound peer), the fixed [`DEFAULT_CONVERSATION`] id.
    pub fn new(peer_id: String) -> Self {
        Self {
            conversation_id: DEFAULT_CONVERSATION.to_string(),
            participants: vec![peer_id.clone()],
            peer_id,
        }
    }

    /// Bind a real (delivered) conversation: its genesis `conversation_id` and
    /// the participant roster (§2). The bound peer is always a participant of
    /// its own view, so it is folded in if the roster omits it — the union then
    /// covers our own complete namespace plus every other participant's cached
    /// copies (§1.4).
    // Consumed by the delivery slice (a bound genesis) + the native union tests;
    // no production caller binds a real conversation yet.
    #[allow(dead_code)]
    pub fn with_conversation(
        peer_id: String,
        conversation_id: String,
        participants: Vec<String>,
    ) -> Self {
        // Dedup the roster (order-preserving) and fold in the bound peer. A
        // self-bind (`other == peer_id`) would otherwise yield `[me, me]`, whose
        // duplicate prefix makes the §1.4 union list every message twice; the
        // dedup collapses it to a single-participant self-view.
        let mut deduped: Vec<String> = Vec::with_capacity(participants.len() + 1);
        for p in participants {
            if !deduped.contains(&p) {
                deduped.push(p);
            }
        }
        if !deduped.iter().any(|p| p == &peer_id) {
            deduped.push(peer_id.clone());
        }
        Self {
            peer_id,
            conversation_id,
            participants: deduped,
        }
    }

    /// The active conversation id.
    #[allow(dead_code)]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// The per-participant subscribe/list prefixes composing this conversation's
    /// view — one `/{participant}/app/chat/{conv}/messages/` per participant, all
    /// read from the BOUND peer's own store: our complete namespace plus the
    /// cached copies delivery lands under each other participant's prefix (§1.4).
    ///
    /// **Worker-mode load-bearing (AGENTS.md):** the window MUST subscribe
    /// exactly these prefixes — on the Worker arm `tree_listing`/`get_entity`
    /// only see prefixes the window has subscribed, so an unsubscribed
    /// participant's messages read as silently absent and the union drops them.
    pub fn subscription_prefixes(&self) -> Vec<String> {
        self.participants
            .iter()
            .map(|p| conversation_messages_prefix(p, &self.conversation_id))
            .collect()
    }

    /// Load the conversation's messages, oldest-first (by author send time —
    /// display order only, not a causal authority, §4.3). The §1.4 union: every
    /// participant's messages, each read from the bound peer's own store (ours
    /// complete; others as delivered under their prefix). Foreign/garbage
    /// entities under any prefix are skipped by `from_entity`.
    pub fn load_messages(&self, peers: &Peers) -> Vec<ChatMessage> {
        // Collect (content-hash, message). The content hash is the message
        // identity AND the stable, non-forgeable tiebreak for the display sort.
        let mut collected: Vec<(String, ChatMessage)> = Vec::new();
        // Content-addressed paths are the message identity — dedup across
        // prefixes so a roster that (defensively) still overlaps never yields the
        // same message twice.
        let mut seen_paths: HashSet<String> = HashSet::new();
        for participant in &self.participants {
            let prefix = conversation_messages_prefix(participant, &self.conversation_id);
            for entry in peers.tree_listing(&self.peer_id, &prefix) {
                if !seen_paths.insert(entry.path.clone()) {
                    continue;
                }
                if let Some(ent) = peers.get_entity(&self.peer_id, &entry.path) {
                    if let Some(mut m) = ChatMessage::from_entity(&ent) {
                        // Defence (B7): a message whose body-declared
                        // conversation_id disagrees with the prefix it was found
                        // under is cache-poison / malformed — drop it. The path
                        // already scopes the conversation; the body must agree.
                        if m.conversation_id != self.conversation_id {
                            continue;
                        }
                        // Authorship authority is the PATH — the participant
                        // namespace the message was authored into / delivered
                        // under — NOT the self-declared body `author` field, which
                        // a peer could forge to impersonate another participant
                        // (and flip the `mine` flag). Stamp the trusted author; a
                        // body that disagreed was a spoof attempt and is
                        // overridden, never trusted.
                        m.author = participant.clone();
                        collected.push((ent.content_hash.to_string(), m));
                    }
                }
            }
        }
        // Oldest-first by author send time (a display heuristic, §4.3), tiebroken
        // by content hash — stable and observation-independent, unlike the body
        // text this used to tiebreak on (B4: two equal-`sent_at` messages ordered
        // by their text is meaningless and lets a crafted body pin position).
        collected.sort_by(|a, b| a.1.sent_at.cmp(&b.1.sent_at).then_with(|| a.0.cmp(&b.0)));
        collected.into_iter().map(|(_, m)| m).collect()
    }

    /// Author + write a message into the bound peer's namespace, keyed by its
    /// content hash (immutable, idempotent). No-op on empty/whitespace. Returns
    /// whether a write was dispatched — the messages subscription re-renders
    /// when it lands, so the caller need not force a repaint. (Compose-draft
    /// clearing is the DOM atom's job via `ctx.drafts`, not the model's.)
    pub fn send(&self, peers: &Peers, body: &str) -> bool {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return false;
        }
        // Defensive clamp (B5): a chat line is short; this only stops a
        // pathological multi-megabyte paste from being hashed, stored, delivered
        // and rendered. Truncated on a char boundary (never mid-codepoint), so
        // the message still sends rather than silently vanishing.
        let body: String = if trimmed.chars().count() > MAX_BODY_CHARS {
            trimmed.chars().take(MAX_BODY_CHARS).collect()
        } else {
            trimmed.to_string()
        };
        let msg = ChatMessage::new(
            self.peer_id.clone(),
            self.conversation_id.clone(),
            body,
            now_ms(),
        );
        let entity = msg.to_entity();
        let path = message_path(
            &self.peer_id,
            &self.conversation_id,
            &entity.content_hash.to_string(),
        );
        peers.dispatch_write(&self.peer_id, path, entity);
        true
    }

    /// Build the render output — the message list (with a `mine` flag per
    /// message) + the live draft. Pure data (no `web_sys`), so it is
    /// native-testable and the DOM layer stays a thin projector.
    pub fn render_output(
        &self,
        peers: &Peers,
        dials: &crate::dial_markers::DialMarkers,
    ) -> super::output::ChatOutput {
        use super::output::{ChatMessageView, ChatOutput, ParticipantReach, StartablePeer};
        let me = self.peer_id.as_str();
        let messages = self
            .load_messages(peers)
            .into_iter()
            .map(|m| ChatMessageView {
                mine: m.author == me,
                author_label: crate::views::display_name(peers, &m.author),
                author: m.author,
                body: m.body,
                sent_at: m.sent_at,
            })
            .collect();
        // "Bound" = a real conversation (a content-hash id), not the default
        // single-peer scratch. Only then do we hide the start-a-chat picker.
        let bound = self.conversation_id != DEFAULT_CONVERSATION;
        let startable = if bound {
            Vec::new()
        } else {
            crate::connections::read_connections(peers)
                .into_iter()
                .map(|c| StartablePeer {
                    name: crate::views::display_name(peers, &c.remote_pid),
                    peer_id: c.remote_pid,
                })
                .collect()
        };
        // Reachability of everyone *else* in the conversation. Unbound means the
        // self-conversation, whose only participant is us — so this stays empty
        // and the header paints no chip.
        //
        // The kernel read-model is the authority; the in-memory dial marker
        // contributes only the transient the kernel deliberately does not model
        // (a dial in flight, or one that gave up before ever connecting), and
        // only while the kernel is silent. Identical resolution to Peer
        // Connections — deliberately, so the two windows can never disagree
        // about the same link.
        let reachability: Vec<ParticipantReach> = if bound {
            self.participants
                .iter()
                .filter(|p| p.as_str() != me)
                .map(|p| ParticipantReach {
                    label: crate::views::display_name(peers, p),
                    status: crate::peer_liveness::conn_display(
                        crate::peer_liveness::liveness_of(peers, p),
                        dials.hint(p),
                    ),
                    peer_id: p.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        // The reason line, and only when it is the *live* reason: we install no
        // establisher AND nothing here is currently reachable. `peer_has_webrtc`
        // answers for the window's own bound peer on both arms — which is the
        // peer that matters, because `ChatDelivery` dispatches as `self.peer_id`
        // and that is the id a counterpart came away with from a `meet`.
        let nothing_reachable = !reachability
            .iter()
            .any(|r| r.status == crate::peer_liveness::ConnDisplay::Connected);
        let no_establisher = bound && nothing_reachable && !peers.peer_has_webrtc(me);

        // Why, when our own ICE agent can say. Same relevance rule as the line
        // above — bound, and nothing currently reachable — because a diagnosis
        // beside a working conversation is the kind of notice users learn to
        // ignore, and the WebRTC gates assert twice a run that we raise no false
        // unreachable note. `verdict_for` is `Unknown` (non-advisory) for any
        // peer whose negotiation has not actually failed, so the guard against
        // speaking mid-establishment lives in the classifier, not here.
        let reachability_advice = crate::reachability::advice_for_conversation(
            bound,
            nothing_reachable,
            reachability
                .iter()
                .map(|r| crate::reachability::verdict_for(&r.peer_id)),
        );

        ChatOutput {
            conversation_id: self.conversation_id.clone(),
            messages,
            bound,
            startable,
            reachability,
            no_establisher,
            reachability_advice,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trips_through_its_entity() {
        let msg = ChatMessage::new("2KAbc", "conv-hash-1", "hello there", 1_723_000_000_000);
        let entity = msg.to_entity();
        assert_eq!(entity.entity_type, MESSAGE_TYPE);
        let back = ChatMessage::from_entity(&entity).expect("decodes");
        assert_eq!(back, msg);
    }

    #[test]
    fn distinct_bodies_hash_distinctly_same_body_is_stable() {
        // Content-addressed: identity is the hash, so two identical messages
        // collapse to one entity (idempotent), two different ones do not.
        let a = ChatMessage::new("2KAbc", "c1", "same", 1).to_entity();
        let a2 = ChatMessage::new("2KAbc", "c1", "same", 1).to_entity();
        let b = ChatMessage::new("2KAbc", "c1", "different", 1).to_entity();
        assert_eq!(a.content_hash, a2.content_hash, "same message → same id");
        assert_ne!(a.content_hash, b.content_hash, "different body → different id");
    }

    #[test]
    fn wrong_type_or_garbage_is_skipped_not_panicked() {
        let foreign = Entity::new("app/state/shell", vec![0x01, 0x02]).unwrap();
        assert!(ChatMessage::from_entity(&foreign).is_none());
        let garbage = Entity::new(MESSAGE_TYPE, vec![0xff, 0xff, 0xff]).unwrap();
        assert!(ChatMessage::from_entity(&garbage).is_none());
    }

    #[test]
    fn conversation_genesis_id_is_content_addressed_and_stable() {
        // Same genesis value → same id (content-addressed identity, §2). A
        // joining peer reads THIS id off the shared genesis entity.
        let a = Conversation::one_to_one("2KAlice", "2KBob", 1_723_000_000_000);
        let a2 = Conversation::one_to_one("2KAlice", "2KBob", 1_723_000_000_000);
        assert_eq!(a.conversation_id(), a2.conversation_id());
        assert!(
            a.conversation_id().starts_with("ecfv1-"),
            "id is a self-describing content-hash, not a fixed width: {}",
            a.conversation_id()
        );
        assert_ne!(
            a.conversation_id(),
            DEFAULT_CONVERSATION,
            "a real genesis id never collides with the default self-conversation"
        );

        // Any genesis-distinguishing field changes the id (distinct conversation).
        let diff_roster = Conversation::one_to_one("2KAlice", "2KCarol", 1_723_000_000_000);
        let diff_time = Conversation::one_to_one("2KAlice", "2KBob", 1_723_000_000_001);
        assert_ne!(a.conversation_id(), diff_roster.conversation_id());
        assert_ne!(a.conversation_id(), diff_time.conversation_id());
    }

    #[test]
    fn conversation_round_trips_and_rejects_foreign() {
        let mut conv = Conversation::one_to_one("2KAlice", "2KBob", 42);
        conv.title = Some("standup".into());
        let entity = conv.to_entity();
        assert_eq!(entity.entity_type, CONVERSATION_TYPE);
        assert_eq!(Conversation::from_entity(&entity), Some(conv));

        // A message entity is not a conversation — decode must decline, not panic.
        let msg = ChatMessage::new("2KAlice", "c1", "hi", 1).to_entity();
        assert!(Conversation::from_entity(&msg).is_none());
        let garbage = Entity::new(CONVERSATION_TYPE, vec![0xff, 0xff]).unwrap();
        assert!(Conversation::from_entity(&garbage).is_none());
    }

    #[test]
    fn room_genesis_is_a_fixed_n_member_roster() {
        // A 3-person room: creator + two others, closed policy, no op-log needed.
        let room = Conversation::room("2KAlice", ["2KBob", "2KCarol"], 100);
        assert_eq!(
            room.initial_participants,
            vec![
                "2KAlice".to_string(),
                "2KBob".to_string(),
                "2KCarol".to_string()
            ],
            "creator leads, others follow in order"
        );
        assert_eq!(room.policy, POLICY_CLOSED);

        // De-dupes a repeated member (incl. the creator repeated in `others`).
        let dedup = Conversation::room("2KAlice", ["2KBob", "2KBob", "2KAlice"], 100);
        assert_eq!(
            dedup.initial_participants,
            vec!["2KAlice".to_string(), "2KBob".to_string()]
        );

        // A distinct roster is a distinct conversation.
        let two = Conversation::room("2KAlice", ["2KBob"], 100);
        assert_ne!(room.conversation_id(), two.conversation_id());
    }

    #[test]
    fn membership_op_round_trips_and_rejects_foreign() {
        let op = MembershipOp {
            conversation_id: "conv1".into(),
            op: MembershipChange::Add,
            subject: "2KDave".into(),
            actor: "2KAlice".into(),
            at: 7,
            prev: Some("ecfv1-sha256:beef".into()),
        };
        let entity = op.to_entity();
        assert_eq!(entity.entity_type, MEMBERSHIP_OP_TYPE);
        assert_eq!(MembershipOp::from_entity(&entity), Some(op));

        // A conversation entity is not a membership-op.
        let conv = Conversation::one_to_one("2KAlice", "2KBob", 1).to_entity();
        assert!(MembershipOp::from_entity(&conv).is_none());
        // An unknown `op` string is a decode failure, not a panic.
        let bad = Entity::new(MEMBERSHIP_OP_TYPE, vec![0xff]).unwrap();
        assert!(MembershipOp::from_entity(&bad).is_none());
    }

    fn add_op(conv: &str, actor: &str, subject: &str, at: u64) -> MembershipOp {
        MembershipOp {
            conversation_id: conv.into(),
            op: MembershipChange::Add,
            subject: subject.into(),
            actor: actor.into(),
            at,
            prev: None,
        }
    }
    fn remove_op(conv: &str, actor: &str, subject: &str, at: u64) -> MembershipOp {
        MembershipOp {
            conversation_id: conv.into(),
            op: MembershipChange::Remove,
            subject: subject.into(),
            actor: actor.into(),
            at,
            prev: None,
        }
    }

    #[test]
    fn fold_closed_room_ignores_every_op() {
        // `closed` grants no post-genesis authority — the roster is the genesis,
        // whatever ops are observed.
        let room = Conversation::room("2KAlice", ["2KBob"], 1); // closed
        let cid = room.conversation_id();
        let ops = vec![
            add_op(&cid, "2KAlice", "2KCarol", 2),
            add_op(&cid, "2KCarol", "2KCarol", 3),
        ];
        assert_eq!(
            fold_roster(&room, &ops),
            vec!["2KAlice".to_string(), "2KBob".to_string()]
        );
    }

    #[test]
    fn fold_open_room_allows_self_add_and_self_leave() {
        let mut room = Conversation::room("2KAlice", ["2KBob"], 1);
        room.policy = "open".into();
        let cid = room.conversation_id();

        // Carol self-adds (open self-grant); Dave self-adds; Bob leaves.
        // An add of *another* peer (Alice adding Eve) is unauthorized → ignored.
        let ops = vec![
            add_op(&cid, "2KCarol", "2KCarol", 5),
            add_op(&cid, "2KAlice", "2KEve", 6), // acting on another → ignored
            add_op(&cid, "2KDave", "2KDave", 7),
            remove_op(&cid, "2KBob", "2KBob", 8), // self-leave
        ];
        let roster = fold_roster(&room, &ops);
        assert!(roster.contains(&"2KCarol".to_string()));
        assert!(roster.contains(&"2KDave".to_string()));
        assert!(!roster.contains(&"2KBob".to_string()), "Bob left");
        assert!(!roster.contains(&"2KEve".to_string()), "cross-peer add ignored");
    }

    #[test]
    fn fold_invite_room_only_self_leave_at_the_floor() {
        let mut room = Conversation::room("2KAlice", ["2KBob"], 1);
        room.policy = "invite".into();
        let cid = room.conversation_id();
        // An invite (Alice adding Carol) needs the ROLE/GROUP grant we don't have
        // yet → ignored at this floor; a self-leave needs no grant → applies.
        let ops = vec![
            add_op(&cid, "2KAlice", "2KCarol", 2),
            remove_op(&cid, "2KBob", "2KBob", 3),
        ];
        let roster = fold_roster(&room, &ops);
        assert!(!roster.contains(&"2KCarol".to_string()), "invite deferred");
        assert!(!roster.contains(&"2KBob".to_string()), "self-leave applies");
        assert_eq!(roster, vec!["2KAlice".to_string()]);
    }

    #[test]
    fn fold_is_order_independent() {
        // Same op set, two observation orders → same roster (convergent, §5.10).
        let mut room = Conversation::room("2KAlice", [] as [&str; 0], 1);
        room.policy = "open".into();
        let cid = room.conversation_id();
        let a = add_op(&cid, "2KBob", "2KBob", 5);
        let b = add_op(&cid, "2KCarol", "2KCarol", 5); // same `at` → hash tiebreak
        let one = fold_roster(&room, &[a.clone(), b.clone()]);
        let two = fold_roster(&room, &[b, a]);
        assert_eq!(one, two);
    }

    #[test]
    fn paths_follow_the_convention_namespace() {
        assert_eq!(
            conversation_messages_prefix("2KAbc", "conv1"),
            "/2KAbc/app/chat/conv1/messages/"
        );
        assert_eq!(
            message_path("2KAbc", "conv1", "ecfv1-sha256:deadbeef"),
            "/2KAbc/app/chat/conv1/messages/ecfv1-sha256:deadbeef"
        );
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    #[tokio::test]
    async fn send_then_load_round_trips_through_the_tree() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ChatModel::new(pid.clone());

        assert!(model.send(&peers, "first"));
        assert!(model.send(&peers, "second"));
        // Empty/whitespace is a no-op, not a blank message.
        assert!(!model.send(&peers, "   "));
        flush_writes().await;

        let msgs = model.load_messages(&peers);
        assert_eq!(msgs.len(), 2, "two real messages landed");
        assert!(msgs.iter().all(|m| m.author == pid), "authored under us");
        let bodies: Vec<&str> = msgs.iter().map(|m| m.body.as_str()).collect();
        assert!(bodies.contains(&"first") && bodies.contains(&"second"));
    }

    #[tokio::test]
    async fn render_output_flags_our_own_messages_as_mine() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ChatModel::new(pid);
        model.send(&peers, "hi");
        flush_writes().await;

        let out = model.render_output(&peers, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.conversation_id, DEFAULT_CONVERSATION);
        assert_eq!(out.messages.len(), 1);
        assert!(out.messages[0].mine, "our own message is flagged mine");
        assert_eq!(out.messages[0].body, "hi");
    }

    #[tokio::test]
    async fn union_view_merges_our_messages_with_a_delivered_peers() {
        // The §1.4 union read side: our own complete messages PLUS another
        // participant's messages, cached under their prefix in OUR store — which
        // is exactly what the delivery layer (a later slice) lands there. Here we
        // seed the "delivered" copy directly to prove the read/merge is
        // delivery-ready without needing the transport yet.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        // A real, valid-format peer id (46-char Base58) for the other
        // participant — the tree handler validates the leading path segment, so
        // the cached-copy path must carry a well-formed peer id. Borrow a fresh
        // peer's id purely as that string.
        let other = Peers::new_direct().primary_peer_id().to_string();
        let conv = "conv-genesis-hash".to_string();

        let model =
            ChatModel::with_conversation(me.clone(), conv.clone(), vec![me.clone(), other.clone()]);

        // Our own message goes through the normal send path (our namespace).
        assert!(model.send(&peers, "hi from me"));

        // The peer's message: authored by `other`, cached under `other`'s prefix
        // in our store (the shape delivery produces). content-hash keyed, so it
        // is idempotent even if delivered twice.
        let their_msg = ChatMessage::new(other.clone(), conv.clone(), "hi from bob", 5);
        let their_entity = their_msg.to_entity();
        let their_path = message_path(&other, &conv, &their_entity.content_hash.to_string());
        // Land the cached copy through the SAME L1 write path a real delivery
        // uses, so the sync read mirror reflects it — an L0 `put_entity` bypasses
        // the notifying index the read path observes. The leading segment must be
        // a well-formed peer id (the tree handler validates it), which is why
        // `other` is a real 46-char id, not a placeholder.
        peers.dispatch_write(&me, their_path, their_entity);

        flush_writes().await;

        let out = model.render_output(&peers, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages.len(), 2, "both participants' messages appear");
        let mine: Vec<&str> = out
            .messages
            .iter()
            .filter(|m| m.mine)
            .map(|m| m.body.as_str())
            .collect();
        let theirs: Vec<&str> = out
            .messages
            .iter()
            .filter(|m| !m.mine)
            .map(|m| m.body.as_str())
            .collect();
        assert_eq!(mine, vec!["hi from me"], "our own flagged mine");
        assert_eq!(theirs, vec!["hi from bob"], "the peer's flagged not-mine");
    }

    #[tokio::test]
    async fn self_bind_does_not_duplicate_messages() {
        // Binding a 1:1 with your OWN id yields participants `[me, me]` before
        // dedup; the §1.4 union would then list every message twice.
        // `with_conversation` must collapse the roster, and `load_messages` must
        // dedup by content-addressed path — so a self-conversation shows each
        // message exactly once. (Audit finding B3.)
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let conv = "self-conv-hash".to_string();

        let model =
            ChatModel::with_conversation(me.clone(), conv.clone(), vec![me.clone(), me.clone()]);
        assert_eq!(
            model.subscription_prefixes().len(),
            1,
            "self-bind collapses to a single participant/prefix"
        );

        assert!(model.send(&peers, "just me"));
        flush_writes().await;

        let out = model.render_output(&peers, &crate::dial_markers::DialMarkers::new());
        assert_eq!(
            out.messages.len(),
            1,
            "the message appears exactly once, not duplicated"
        );
        assert_eq!(out.messages[0].body, "just me");
        assert!(out.messages[0].mine);
    }

    #[tokio::test]
    async fn authorship_is_the_path_authority_not_the_forged_body_field() {
        // A malicious participant authors a message under their OWN prefix but
        // sets the body `author` field to US, to spoof it as our own message.
        // The union read must attribute by the PATH authority (the prefix owner),
        // never the body field — so it renders as theirs, not `mine`. (Audit B2.)
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let other = Peers::new_direct().primary_peer_id().to_string();
        let conv = "conv-spoof-hash".to_string();
        let model =
            ChatModel::with_conversation(me.clone(), conv.clone(), vec![me.clone(), other.clone()]);

        // Forge: body author = `me`, but the entity is cached under `other`'s
        // prefix (the shape a hostile peer's delivered log would take).
        let forged = ChatMessage::new(me.clone(), conv.clone(), "you said this", 7);
        let entity = forged.to_entity();
        let path = message_path(&other, &conv, &entity.content_hash.to_string());
        peers.dispatch_write(&me, path, entity);
        flush_writes().await;

        let out = model.render_output(&peers, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages.len(), 1);
        assert_eq!(
            out.messages[0].author, other,
            "attributed to the path owner, not the forged body `author`"
        );
        assert!(
            !out.messages[0].mine,
            "a forged `author=me` under another peer's prefix is NOT mine"
        );
    }

    #[tokio::test]
    async fn union_view_merges_three_authors_in_a_room() {
        // A 3-person room: our own messages + two other participants' messages,
        // each cached under its author's prefix in our store (what delivery
        // lands). Proves the union scales past 1:1 with correct mine-vs-others.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let bob = Peers::new_direct().primary_peer_id().to_string();
        let carol = Peers::new_direct().primary_peer_id().to_string();
        let conv = "room-genesis-hash".to_string();

        let model = ChatModel::with_conversation(
            me.clone(),
            conv.clone(),
            vec![me.clone(), bob.clone(), carol.clone()],
        );

        assert!(model.send(&peers, "hi from me"));
        for (author, body, at) in [(&bob, "hi from bob", 5u64), (&carol, "hi from carol", 6)] {
            let ent = ChatMessage::new(author.clone(), conv.clone(), body, at).to_entity();
            let path = message_path(author, &conv, &ent.content_hash.to_string());
            peers.dispatch_write(&me, path, ent);
        }
        flush_writes().await;

        let out = model.render_output(&peers, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages.len(), 3, "all three authors' messages appear");
        let mine: Vec<&str> = out
            .messages
            .iter()
            .filter(|m| m.mine)
            .map(|m| m.body.as_str())
            .collect();
        assert_eq!(mine, vec!["hi from me"], "only our own is mine");
        let others: Vec<&str> = out
            .messages
            .iter()
            .filter(|m| !m.mine)
            .map(|m| m.body.as_str())
            .collect();
        assert!(others.contains(&"hi from bob") && others.contains(&"hi from carol"));
    }

    #[test]
    fn subscription_prefixes_cover_every_participant() {
        let model = ChatModel::with_conversation(
            "2KMe".into(),
            "conv1".into(),
            vec!["2KMe".into(), "2KOther".into()],
        );
        let prefixes = model.subscription_prefixes();
        assert_eq!(
            prefixes,
            vec![
                "/2KMe/app/chat/conv1/messages/".to_string(),
                "/2KOther/app/chat/conv1/messages/".to_string(),
            ],
            "one prefix per participant, exactly what load_messages reads"
        );

        // The bound peer is always in its own view even if the roster omits it.
        let folded = ChatModel::with_conversation("2KMe".into(), "c".into(), vec!["2KOther".into()]);
        assert!(folded
            .subscription_prefixes()
            .contains(&"/2KMe/app/chat/c/messages/".to_string()));
    }
}
