//! **Where a publisher's artifacts actually live** — the `http-poll` endpoint,
//! read rather than assumed.
//!
//! ## The rule this module exists to hold
//!
//! `EXTENSION-NETWORK` §6.5.3 (v1.8, arch `ROUTING-2026-08-18-p` §3):
//!
//! > `signed_pointer` is a tree path; `manifest_url_prefix` is where you fetch.
//! > **A consumer MUST NOT join `signed_pointer` onto an origin.** The manifest's
//! > location is DISCOVERED from the profile, never derived by convention from
//! > the tree path.
//!
//! The two fields answer different questions — `signed_pointer` says *what the
//! origin is asserting* (the §3.3a entity path, so you know a signed root exists
//! and what to verify), `manifest_url_prefix` says *where to GET it*.
//! `{origin}/manifest` and `{origin}/{peer_id}/system/peer/published-root` are
//! **equally conformant**, and only the advertised one is findable.
//!
//! ## Why it is a MUST and not a style note
//!
//! It was found by the cross-implementation run, and the failure is at **hop
//! 0**: our reader derived the manifest path by convention, workbench-go serves
//! theirs at `{origin}/manifest`, and the walk stopped before reaching any of
//! the surfaces the two arms *do* agree on — content sharding, the bare-hashable
//! body form, the two-hop signature. A convention-derived front door does not
//! fail loudly and locally; it fails first and hides everything behind it.
//!
//! **A profile field that exists to be read is not a default to be assumed.**
//!
//! ## The fallback is named, not silent
//!
//! [`PublishLayout::conventional`] still exists, because a pin made from a bare
//! origin string (`name pin <peer-id> <origin>`) has no profile to read — the
//! user typed a URL, not an endpoint. It is the *absence of a source*, not a
//! shortcut, and every construction of it says which case it is.

use entity_hash::Hash;

use super::paths::PUBLISHED_ROOT_REL;

/// How content-hash-keyed URLs are laid out under `content_url_prefix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLayout {
    /// `{prefix}/{hex[0..2]}/{hex[2..4]}/{hex}` — what both arms publish.
    Sharded24,
    /// `{prefix}/{hex}` — upstream's flat `content_url`.
    Flat,
}

impl ContentLayout {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "sharded-2-4" => Some(Self::Sharded24),
            "flat" => Some(Self::Flat),
            _ => None,
        }
    }
}

/// An `http-poll` endpoint, as advertised — the three prefixes plus the two
/// suffixes, with nothing re-derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishLayout {
    /// **Terminal** — the manifest URL itself, no suffix and no trailing slash
    /// (§6.5.3.1). Not a prefix anything is appended to, despite the field name
    /// it is read from.
    pub manifest_url: String,
    pub content_url_prefix: String,
    pub content_layout: ContentLayout,
    /// Tree-path-keyed URLs. **What gets appended is the peer-qualified path** —
    /// see the note on [`Self::tree_leaf_url`], which is where the two arms
    /// currently disagree.
    pub tree_url_prefix: String,
    pub tree_leaf_suffix: String,
}

impl PublishLayout {
    /// **Our own emission's shape, used when there is no profile to read.**
    ///
    /// Not a default for a publisher we have never met — for that, the absence
    /// of a profile means the origin is unconsumable and saying so beats
    /// guessing. This exists for the case where the *user* supplied the origin
    /// (`name pin`) and there is no endpoint document anywhere in the chain.
    pub fn conventional(origin: &str, peer_id: &str) -> Self {
        let origin = origin.trim_end_matches('/');
        Self {
            manifest_url: format!("{origin}/{peer_id}/{PUBLISHED_ROOT_REL}"),
            content_url_prefix: format!("{origin}/content"),
            content_layout: ContentLayout::Sharded24,
            tree_url_prefix: format!("{origin}/{peer_id}"),
            tree_leaf_suffix: ".bin".to_string(),
        }
    }

    /// Decode an `http-poll` transport profile's `endpoint`.
    ///
    /// `profile` is the profile map itself — the value that sits in a registry
    /// binding's `transports` array, or the body of a publisher's emitted
    /// `transport-profile` artifact.
    ///
    /// Returns `None` for a profile of another `transport_type`, or one missing
    /// a field a fetch cannot proceed without. **`manifest_url_prefix` is
    /// required**: a profile that does not advertise it describes an origin
    /// whose signed root is, by the rule above, not findable at all.
    pub fn from_http_poll_profile(profile: &entity_ecf::Value) -> Option<Self> {
        let map = profile.as_map()?;
        let get = |m: &[(entity_ecf::Value, entity_ecf::Value)], k: &str| {
            m.iter()
                .find_map(|(mk, mv)| match mk {
                    entity_ecf::Value::Text(s) if s == k => Some(mv),
                    _ => None,
                })
                .cloned()
        };
        if get(map, "transport_type").as_ref().and_then(|v| v.as_text()) != Some("http-poll") {
            return None;
        }
        let endpoint = get(map, "endpoint")?;
        let ep = endpoint.as_map()?;
        let text = |k: &str| get(ep, k).and_then(|v| v.as_text().map(str::to_string));

        Some(Self {
            manifest_url: text("manifest_url_prefix")?.trim_end_matches('/').to_string(),
            content_url_prefix: text("content_url_prefix")?.trim_end_matches('/').to_string(),
            // An unknown layout is NOT sharded-by-default: a wrong guess here
            // produces 404s on every blob, which reads as a withholding origin.
            content_layout: ContentLayout::parse(
                &text("content_layout").unwrap_or_else(|| "sharded-2-4".into()),
            )?,
            tree_url_prefix: text("tree_url_prefix")?.trim_end_matches('/').to_string(),
            tree_leaf_suffix: text("tree_leaf_suffix").unwrap_or_else(|| ".bin".into()),
        })
    }

    /// Decode a publisher's emitted `transport-profile` artifact — the entity a
    /// static publisher ships beside its site (§6.5.4: the profile is
    /// out-of-band and specifically **not** served at `manifest_url_prefix`,
    /// which is reserved for the signed root).
    ///
    /// **Decoded with `ciborium` directly, not `entity_wire::decode_entity`**,
    /// for two independent reasons and either one is sufficient: `entity_wire`
    /// is **not linked on wasm32**, so a browser could not call it at all; and
    /// the artifact's `data` arrives in two shapes across implementations — an
    /// inline CBOR map (what the Go arm emits) or a `bstr` of encoded ECF. Both
    /// are handled here, because a consumer that accepts only its own arm's
    /// shape is the front-door defect one layer down.
    pub fn from_profile_artifact(bytes: &[u8]) -> Option<Self> {
        let outer: entity_ecf::Value = ciborium::from_reader(bytes).ok()?;
        let data = outer.as_map()?.iter().find_map(|(k, v)| match k {
            entity_ecf::Value::Text(s) if s == "data" => Some(v),
            _ => None,
        })?;
        match data {
            entity_ecf::Value::Map(_) => Self::from_http_poll_profile(data),
            entity_ecf::Value::Bytes(b) => {
                let inner: entity_ecf::Value = ciborium::from_reader(&b[..]).ok()?;
                Self::from_http_poll_profile(&inner)
            }
            _ => None,
        }
    }

    /// **Which peer a transport-profile artifact declares itself to be about**,
    /// or `None` when it does not say.
    ///
    /// The endpoint fields answer *where to fetch*; this answers *whose*, and
    /// they are different questions. [`PublishLayout`] deliberately keeps only
    /// the first, so this reads the artifact rather than the parsed layout.
    ///
    /// **Why it exists.** `transport-profile` is **one artifact per hosting
    /// scope** while its contents are per-peer, so at an origin serving several
    /// publishers the last publish wins — and a consumer that follows the
    /// advertised layout blindly then resolves *the wrong peer's* tree. Measured
    /// 2026-09-03: after a second publisher published at one origin,
    /// `publish --verify` on the first reported *"the signed root is not walkable
    /// — a pinned consumer resolves NOTHING from this tree"*, because
    /// `DirFetcher` located its manifest through a profile describing the other
    /// peer.
    ///
    /// **`None` is trusted, deliberately.** core-go's profile carries `peer_id`
    /// and ours does, but a conformant publisher need not — and a profile that
    /// does not name a peer is not evidence it names a *different* one. Rejecting
    /// on absence would break reading any implementation that omits it, which is
    /// a live cross-impl path (`crossimpl_go`). Only a **positive mismatch**
    /// disqualifies.
    pub fn profile_peer_id(bytes: &[u8]) -> Option<String> {
        let outer: entity_ecf::Value = ciborium::from_reader(bytes).ok()?;
        let data = outer.as_map()?.iter().find_map(|(k, v)| match k {
            entity_ecf::Value::Text(s) if s == "data" => Some(v),
            _ => None,
        })?;
        let inner_owned: entity_ecf::Value;
        let map = match data {
            entity_ecf::Value::Map(_) => data,
            entity_ecf::Value::Bytes(b) => {
                inner_owned = ciborium::from_reader(&b[..]).ok()?;
                &inner_owned
            }
            _ => return None,
        };
        map.as_map()?
            .iter()
            .find_map(|(k, v)| match k {
                entity_ecf::Value::Text(s) if s == "peer_id" => v.as_text().map(str::to_string),
                _ => None,
            })
            .filter(|p| !p.is_empty())
    }

    /// `{content_url_prefix}/{layout-path}/{hash}`.
    pub fn content_url(&self, h: &Hash) -> String {
        let hex = h.to_hex();
        match self.content_layout {
            // hex is ≥66 chars for any supported algorithm.
            ContentLayout::Sharded24 => {
                format!("{}/{}/{}/{}", self.content_url_prefix, &hex[0..2], &hex[2..4], hex)
            }
            ContentLayout::Flat => format!("{}/{}", self.content_url_prefix, hex),
        }
    }

    /// **Does `tree_url_prefix` already carry the peer-id?**
    ///
    /// §6.5.3 states the tree join two ways and two conformant publishers emit
    /// one each: the normative sentence is
    /// `{tree_url_prefix}/{peer_id}/{path}{tree_leaf_suffix}` (origin-rooted —
    /// workbench-go), while the worked example in the same section advertises
    /// `tree_url_prefix: ".../peers/<peer_id>"` and joins `{tree_url_prefix}/{tree-path}`
    /// (peer-rooted — ours). Routed as `ROUTING-2026-08-19-c` §3, unruled.
    ///
    /// **This is the audit-F6 discriminator, reused.** `http_poll_origin` already
    /// had to answer the same question to strip an origin, and its rule is the
    /// one that survived review: act only when the last path segment is
    /// **exactly** the peer-id we are about to pin. An unconditional split turned
    /// a perfectly legal prefix into `https:/`; an unconditional join produces
    /// `{origin}/{peer}/{peer}/{path}`. Both fail as "the origin is down".
    ///
    /// A bridge until arch rules, not a third convention: it reads what the
    /// publisher wrote instead of deciding what it ought to have written.
    /// **`last == peer_id` is the whole guard** — and it is sufficient on its
    /// own. F6's rule also required a non-empty head, which reads as a second
    /// safety belt and is in fact a bug: a same-origin deployment advertises
    /// `tree_url_prefix: "/{peer_id}"`, whose head *is* empty and which is
    /// peer-rooted all the same. The mangling case F6 found —
    /// `"https://x.example"` splitting to `("https:/", "x.example")` — is already
    /// refused because `"x.example"` is not the peer-id we are pinning.
    fn prefix_carries_peer(&self, peer_id: &str) -> bool {
        self.tree_url_prefix
            .rsplit_once('/')
            .is_some_and(|(_, last)| last == peer_id)
    }

    /// A tree-path URL — `path` is **peer-relative** (`system/signature/…`), the
    /// key space our trie and our projection both use.
    pub fn tree_url(&self, peer_id: &str, path: &str, suffix: &str) -> String {
        let path = path.trim_start_matches('/');
        if self.prefix_carries_peer(peer_id) {
            format!("{}/{path}{suffix}", self.tree_url_prefix)
        } else {
            format!("{}/{peer_id}/{path}{suffix}", self.tree_url_prefix)
        }
    }

    /// A tree leaf — `{…}{tree_leaf_suffix}`.
    pub fn tree_leaf_url(&self, peer_id: &str, path: &str) -> String {
        let suffix = self.tree_leaf_suffix.clone();
        self.tree_url(peer_id, path, &suffix)
    }

    /// The two-hop signature pointer for a published-root entity hash.
    pub fn signature_pointer_url(&self, peer_id: &str, target: &Hash) -> String {
        self.tree_leaf_url(peer_id, &format!("system/signature/{}", target.to_hex()))
    }

    /// **The origin this publisher is served from**, recovered from
    /// `tree_url_prefix` by the same discriminator.
    ///
    /// This replaces `named_site::http_poll_origin`'s conditional strip, which
    /// returned `None` for every layout that did not bake the peer-id into the
    /// prefix — i.e. for every conformant publisher that is not us. It is used
    /// only for the **transport-trusted** convenience listings (`sites.list`);
    /// nothing on the signed path derives a URL from it any more.
    pub fn origin_for(&self, peer_id: &str) -> String {
        if self.prefix_carries_peer(peer_id) {
            self.tree_url_prefix
                .rsplit_once('/')
                .map(|(head, _)| head.to_string())
                .unwrap_or_else(|| self.tree_url_prefix.clone())
        } else {
            self.tree_url_prefix.clone()
        }
    }

    /// **Map an advertised URL onto a path inside a projected directory.**
    ///
    /// A directory *is* an origin with the transport removed, so the consumer
    /// knows which origin it stands for — the one the publisher was pointed at
    /// when it emitted. Returns `None` when the URL belongs to some other
    /// origin, which is a layout this directory cannot serve and is worth saying
    /// rather than silently reading the wrong file.
    pub fn relative_to_origin<'a>(url: &'a str, origin: &str) -> Option<&'a str> {
        let origin = origin.trim_end_matches('/');
        url.strip_prefix(origin).map(|r| r.trim_start_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A profile says whose it is, and the absent case is trusted on purpose.**
    ///
    /// `transport-profile` is one artifact per hosting scope with per-peer
    /// contents, so at a shared origin the last publish wins and a consumer must
    /// be able to notice the profile is not about the peer it is reading. Only a
    /// **positive mismatch** may disqualify: a conformant publisher need not emit
    /// `peer_id`, and rejecting on absence would break reading one that does not
    /// (core-go's fixture does emit it, which is why the real one is checked
    /// here rather than only a synthetic).
    #[test]
    fn a_profile_names_the_peer_it_is_about_and_silence_is_not_a_mismatch() {
        let real = std::fs::read("tests/fixtures/crossimpl-go-site/transport-profile")
            .expect("the go fixture ships a transport-profile");
        assert_eq!(
            PublishLayout::profile_peer_id(&real).as_deref(),
            Some("2KLv2nhwtPrLFd4BZFQuNK1ujtE74q8cVg7y8cYdcZZ5BL"),
            "a foreign publisher's profile declares its own peer, and that is the \
             discriminator a shared hosting scope needs"
        );
        // Garbage is silence, not a mismatch — it must not disqualify a layout.
        assert_eq!(PublishLayout::profile_peer_id(b"not a profile"), None);
        assert_eq!(PublishLayout::profile_peer_id(&[]), None);
    }

    /// The two arms' advertised manifest locations are **both** conformant and
    /// **not** each other — which is the entire content of the v1.8 MUST.
    #[test]
    fn two_conformant_publishers_advertise_different_manifest_urls() {
        let ours = PublishLayout::conventional("https://x.example", "2PEER");
        assert_eq!(
            ours.manifest_url,
            "https://x.example/2PEER/system/peer/published-root"
        );

        let theirs = profile("https://go-arm.example", "https://go-arm.example/manifest");
        let theirs = PublishLayout::from_http_poll_profile(&theirs).expect("decodes");
        assert_eq!(theirs.manifest_url, "https://go-arm.example/manifest");

        assert_ne!(
            ours.manifest_url, theirs.manifest_url,
            "if these ever coincide this gate has stopped testing anything — the \
             point is that a convention-derived front door misses a conformant peer"
        );
    }

    /// A profile with no `manifest_url_prefix` is refused rather than filled in
    /// from the convention. Filling it in is exactly the defect the MUST names.
    #[test]
    fn a_profile_without_a_manifest_url_is_not_silently_completed() {
        let mut p = raw_profile("https://x.example");
        strip(&mut p, "manifest_url_prefix");
        assert!(
            PublishLayout::from_http_poll_profile(&p).is_none(),
            "no advertised manifest ⇒ no findable signed root; guessing one is the MUST violation"
        );
    }

    /// An unrecognised `content_layout` is refused, not assumed sharded. A wrong
    /// guess 404s every blob, which a consumer reads as a withholding origin —
    /// the failure mode this arc keeps meeting.
    #[test]
    fn an_unknown_content_layout_is_refused_rather_than_guessed() {
        let mut p = raw_profile("https://x.example");
        set(&mut p, "content_layout", "sharded-3-3");
        assert!(PublishLayout::from_http_poll_profile(&p).is_none());
    }

    #[test]
    fn a_profile_of_another_transport_type_is_not_an_http_poll_endpoint() {
        let mut p = entity_ecf::cbor_map! {
            "transport_type" => entity_ecf::Value::Text("tcp".into()),
            "endpoint" => entity_ecf::cbor_map! {
                "url" => entity_ecf::Value::Text("tcp://x.example:4041".into()),
            },
        };
        assert!(PublishLayout::from_http_poll_profile(&p).is_none());
        set(&mut p, "transport_type", "http-poll");
        assert!(
            PublishLayout::from_http_poll_profile(&p).is_none(),
            "an http-poll profile carrying a live `url` endpoint is still unusable"
        );
    }

    #[test]
    fn a_url_under_another_origin_is_not_a_path_in_this_directory() {
        assert_eq!(
            PublishLayout::relative_to_origin("https://x.example/manifest", "https://x.example"),
            Some("manifest")
        );
        assert_eq!(
            PublishLayout::relative_to_origin("https://evil.example/manifest", "https://x.example"),
            None
        );
    }

    // -- fixtures ----------------------------------------------------------

    fn profile(origin: &str, manifest: &str) -> entity_ecf::Value {
        let mut p = raw_profile(origin);
        set_endpoint(&mut p, "manifest_url_prefix", manifest);
        p
    }

    fn raw_profile(origin: &str) -> entity_ecf::Value {
        entity_ecf::cbor_map! {
            "transport_type" => entity_ecf::Value::Text("http-poll".into()),
            "endpoint" => entity_ecf::cbor_map! {
                "tree_url_prefix" => entity_ecf::Value::Text(origin.to_string()),
                "content_url_prefix" => entity_ecf::Value::Text(format!("{origin}/content")),
                "content_layout" => entity_ecf::Value::Text("sharded-2-4".into()),
                "tree_leaf_suffix" => entity_ecf::Value::Text(".bin".into()),
                "manifest_url_prefix" => entity_ecf::Value::Text(format!("{origin}/manifest")),
            },
        }
    }

    fn entries(v: &mut entity_ecf::Value) -> &mut Vec<(entity_ecf::Value, entity_ecf::Value)> {
        match v {
            entity_ecf::Value::Map(m) => m,
            _ => panic!("fixture is a map"),
        }
    }

    fn set(v: &mut entity_ecf::Value, key: &str, val: &str) {
        if key == "content_layout" {
            return set_endpoint(v, key, val);
        }
        let e = entries(v);
        e.retain(|(k, _)| !matches!(k, entity_ecf::Value::Text(s) if s == key));
        e.push((entity_ecf::text(key), entity_ecf::Value::Text(val.into())));
    }

    fn set_endpoint(v: &mut entity_ecf::Value, key: &str, val: &str) {
        for (k, mv) in entries(v).iter_mut() {
            if matches!(k, entity_ecf::Value::Text(s) if s == "endpoint") {
                let e = entries(mv);
                e.retain(|(k, _)| !matches!(k, entity_ecf::Value::Text(s) if s == key));
                e.push((entity_ecf::text(key), entity_ecf::Value::Text(val.into())));
                return;
            }
        }
        panic!("fixture has an endpoint");
    }

    fn strip(v: &mut entity_ecf::Value, key: &str) {
        for (k, mv) in entries(v).iter_mut() {
            if matches!(k, entity_ecf::Value::Text(s) if s == "endpoint") {
                entries(mv).retain(|(k, _)| !matches!(k, entity_ecf::Value::Text(s) if s == key));
                return;
            }
        }
    }
}
