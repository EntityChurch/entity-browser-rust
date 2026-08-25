//! **`name_format_dispatch` — which backends may see which names.**
//!
//! `EXTENSION-REGISTRY` §4.1 step 2 calls this *"the primary privacy mechanism —
//! without it, the queried name leaks to broad-matching backends earlier in
//! priority."* This module is that filter, plus the classification that decides
//! what a catch-all is allowed to contain.
//!
//! ## The tension this resolves
//!
//! `GUIDE-RESOLUTION` §6.1 wants a bare name (`alice`) to route via the catch-all
//! `*` to *"local-name, then default registry"* — otherwise typing a name does
//! not work. §4.1 step 2 says a broad-matching backend is exactly how a private
//! name leaks. Both are right, and the hidden variable is that **"leaks the
//! queried name" is a property of the backend, not of the glob**.
//!
//! A **static `peer-issued` backend resolved by walking a signed root does not
//! receive the name at all**: every request is content-addressed
//! (`content/{aa}/{bb}/{hex}`) and the key is matched inside a node already
//! fetched. Measured, not argued —
//! `named_site::tests::resolving_a_name_never_puts_that_name_on_the_wire` and
//! `…::a_name_the_registry_does_not_carry_dies_in_the_trie_without_being_sent`
//! (a miss costs 4 requests, none carrying the name). So the catch-all may carry
//! the default registry *because that backend is name-blind*, while `dns-txt`
//! and friends must require an explicit `@`/`scheme:` marker.
//!
//! Full reasoning, and the review asks that go with it:
//! `docs/architecture/reviews/PROPOSAL-NAME-FORMAT-DISPATCH-DEFAULTS-AND-THE-NAME-BLIND-BACKEND.md`.
//!
//! ## Status — this ships the mechanism, NOT a default chain entry
//!
//! [`DEFAULT_RULES`] is our *proposed* table and is **not installed anywhere**.
//! Until arch ratifies, a deployment adds its registry explicitly and
//! [`validate_rules`] refuses the configuration that would leak. Shipping our
//! globs first is how two app tiers ship two — the failure §4.1 step 2 names.

#![allow(dead_code)] // the evaluator ships ahead of B16b's surface, deliberately.

use entity_registry::resolver::glob_match;

/// Whether consulting a backend for a name discloses that name.
///
/// A new kind defaults to [`Disclosure::Transmitting`] — fail closed, the same
/// posture as "no signaling node, no WebRTC establisher".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disclosure {
    /// Consultation does not transmit the name or a reversible function of it.
    Blind,
    /// Consultation discloses the name to the backend's operator or the network.
    Transmitting,
}

/// How a backend kind is consulted. **`peer-issued` is name-blind only over a
/// signed root** — a consumer that trusts the host-served `by-name` pointer puts
/// the name in a URL path and loses the property (our F1 / arch D1, still owed by
/// three engines). That carve-out is the conformance line, so it is spelled here
/// rather than assumed.
pub fn disclosure_of(backend_kind: &str) -> Disclosure {
    match backend_kind {
        "local-name" | "pinned" | "out-of-band" | "self-certifying" => Disclosure::Blind,
        // Ours, and only because `named_site::resolve_name` walks the signed root.
        "peer-issued" => Disclosure::Blind,
        _ => Disclosure::Transmitting,
    }
}

/// One `name_format_dispatch` entry (§4.1): a POSIX shell-glob and the backend
/// kinds it makes eligible.
#[derive(Debug, Clone)]
pub struct DispatchRule {
    pub pattern: String,
    pub backend_kinds: Vec<String>,
}

impl DispatchRule {
    pub fn new(pattern: &str, kinds: &[&str]) -> Self {
        Self {
            pattern: pattern.to_string(),
            backend_kinds: kinds.iter().map(|k| k.to_string()).collect(),
        }
    }
}

/// A pattern is **broad** when it can match a name carrying no explicit authority
/// marker — i.e. a bare name the user did not aim anywhere.
///
/// Deliberately conservative: anything that does not *require* an `@`, a
/// `scheme:` prefix or a literal suffix counts as broad. A pattern we cannot
/// confidently classify is treated as broad, because the failure direction
/// matters — calling a broad pattern narrow is what leaks.
pub fn is_broad(pattern: &str) -> bool {
    let requires_marker = pattern.contains('@')
        || pattern.contains(':')
        || (pattern.starts_with('*') && pattern.len() > 1 && !pattern[1..].contains('*'));
    !requires_marker
}

/// **D-B.** A broad pattern MUST NOT make a name-transmitting backend eligible.
///
/// This is the check that turns §4.1 step 2 from a statement of intent into a
/// configuration a deployment can be held to. Returns every violation, not the
/// first — an operator fixing a chain wants the whole list.
pub fn validate_rules(rules: &[DispatchRule]) -> Result<(), Vec<String>> {
    let mut bad = Vec::new();
    for r in rules {
        if !is_broad(&r.pattern) {
            continue;
        }
        for k in &r.backend_kinds {
            if disclosure_of(k) == Disclosure::Transmitting {
                bad.push(format!(
                    "pattern {:?} is broad (it matches names carrying no explicit authority) \
                     and lists name-transmitting backend {:?} — every name the user types would \
                     be disclosed to it",
                    r.pattern, k
                ));
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

/// **§4.1 step 2, as written: a filter, not a routing table.** A name matching
/// several rules is eligible at the *union* of their backends; `resolver_chain`
/// priority (step 3) is what orders them. First-match-wins would be a semantic
/// change to a landed step and is deliberately NOT what this does.
///
/// Order is preserved and duplicates removed, so the caller sees a stable list.
pub fn eligible_backends(rules: &[DispatchRule], name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in rules {
        if glob_match(&r.pattern, name) {
            for k in &r.backend_kinds {
                if !out.iter().any(|e| e == k) {
                    out.push(k.clone());
                }
            }
        }
    }
    out
}

/// What the `X` in `name@X` turned out to be (§6.3, proposal D-D).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authority {
    /// `X` validates as a V7 §1.5 peer-id ⇒ resolve `name` however the chain
    /// does, but the answer MUST equal this.
    Pin(String),
    /// `X` is a registry handle or DNS domain ⇒ resolve `name` *at* it.
    Handle(String),
}

/// Split `name@X` into its local part and authority, deciding **structurally**
/// what `X` is.
///
/// One separator, dispatched on what `X` decodes as. Two separators were the
/// alternative and are worse: every link-emitting surface would have to choose,
/// and a consumer given the wrong one has to guess an intent the bytes already
/// settled.
///
/// The discriminator is a full peer-id validation (supported key/hash type,
/// digest length matching the pair) — **not** a charset test, which would be
/// wrong: a handle like `entitychurch` is entirely within the Base58 alphabet.
/// The residual collision is closed at issuance, not here — see
/// [`handle_is_issuable`].
///
/// Splits on the **last** `@`, so a local part containing one survives.
pub fn split_qualified(name: &str) -> (&str, Option<Authority>) {
    match name.rsplit_once('@') {
        Some((local, x)) if !local.is_empty() && !x.is_empty() => {
            let auth = if peer_id_shaped(x) {
                Authority::Pin(x.to_string())
            } else {
                Authority::Handle(x.to_string())
            };
            (local, Some(auth))
        }
        _ => (name, None),
    }
}

/// **D-D's issuance-time MUST.** A registry handle or local-name must not be a
/// string that validates as a peer-id, or `name@handle` becomes ambiguous with
/// `name@pin`. Refusing it where a human can read the error beats resolving it
/// ambiguously where nobody sees.
pub fn handle_is_issuable(handle: &str) -> bool {
    !peer_id_shaped(handle)
}

fn peer_id_shaped(s: &str) -> bool {
    entity_crypto::PeerId::from(s.to_string()).validate().is_ok()
}

/// **Our proposed defaults (D-C) — NOT installed.** Present so the table can be
/// tested and reviewed as one object rather than argued about in prose. Row 6 is
/// the whole proposal: the catch-all carries the default registry *because*
/// `peer-issued` over a signed root is name-blind, and carries nothing else.
pub fn default_rules() -> Vec<DispatchRule> {
    vec![
        DispatchRule::new("did:web:*", &["did-web"]),
        DispatchRule::new("did:key:*", &["did-key"]),
        DispatchRule::new("*.eth", &["consensus-anchored"]),
        DispatchRule::new("*@*.*", &["dns-txt", "well-known-url"]),
        DispatchRule::new("*@*", &["peer-issued"]),
        DispatchRule::new("*", &["local-name", "peer-issued"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The proposed default table must satisfy its own rule. If D-C and D-B ever
    /// disagree, this is where it shows — and it is the check an operator's own
    /// chain gets too.
    #[test]
    fn the_proposed_defaults_satisfy_the_catch_all_constraint() {
        validate_rules(&default_rules()).expect("D-C must satisfy D-B");
    }

    /// **The configuration the whole proposal exists to forbid**, and the one
    /// `GUIDE-RESOLUTION` §6.1's table would produce if read literally: a
    /// catch-all that reaches a name-transmitting backend. Every name the user
    /// ever types — including one they never meant to look up — goes to it.
    #[test]
    fn a_catch_all_reaching_a_name_transmitting_backend_is_refused() {
        let rules = vec![DispatchRule::new("*", &["local-name", "dns-txt"])];
        let err = validate_rules(&rules).expect_err("a catch-all with dns-txt must be refused");
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("dns-txt"), "{:?}", err);
    }

    /// An unknown backend kind is name-transmitting, so a catch-all listing one
    /// is refused. Fail-closed: a kind nobody has classified is exactly the kind
    /// nobody has checked.
    #[test]
    fn an_unclassified_backend_is_treated_as_transmitting() {
        assert_eq!(disclosure_of("some-future-backend"), Disclosure::Transmitting);
        let rules = vec![DispatchRule::new("*", &["some-future-backend"])];
        assert!(validate_rules(&rules).is_err());
    }

    /// A name-transmitting backend is fine once its pattern requires a marker the
    /// user had to type — that is the whole bargain.
    #[test]
    fn a_marker_bearing_pattern_may_carry_a_transmitting_backend() {
        validate_rules(&[DispatchRule::new("*@*.*", &["dns-txt"])]).expect("explicit @ is enough");
        validate_rules(&[DispatchRule::new("did:web:*", &["did-web"])]).expect("explicit scheme");
        validate_rules(&[DispatchRule::new("*.eth", &["consensus-anchored"])])
            .expect("explicit suffix");
    }

    /// §4.1 step 2 is a **filter**: a name matching several rules is eligible at
    /// the union. Pinned here because "ordered glob list, first match wins" is
    /// the obvious reading and it is the wrong one.
    #[test]
    fn matching_several_rules_yields_the_union_not_the_first() {
        let got = eligible_backends(&default_rules(), "alice@example.org");
        assert!(got.contains(&"dns-txt".to_string()), "{got:?}");
        assert!(got.contains(&"well-known-url".to_string()), "{got:?}");
        // `*@*` and `*` match it too — the union, not a winner.
        assert!(got.contains(&"peer-issued".to_string()), "{got:?}");
        assert!(got.contains(&"local-name".to_string()), "{got:?}");
    }

    /// A bare name reaches the default registry — the product requirement §6.1
    /// states — and reaches **no** name-transmitting backend.
    #[test]
    fn a_bare_name_reaches_the_default_registry_and_nothing_that_would_publish_it() {
        let got = eligible_backends(&default_rules(), "alice");
        assert!(got.contains(&"local-name".to_string()), "{got:?}");
        assert!(got.contains(&"peer-issued".to_string()), "{got:?}");
        for k in &got {
            assert_eq!(
                disclosure_of(k),
                Disclosure::Blind,
                "a bare name made {k} eligible, and it transmits the name"
            );
        }
    }

    /// A private-looking dotted name is still a bare name — no `@`, no scheme —
    /// so it must not reach DNS. This is the concrete leak §4.1 step 2 warns
    /// about, stated as a test.
    #[test]
    fn a_private_dotted_name_does_not_reach_dns() {
        let got = eligible_backends(&default_rules(), "payroll.internal");
        assert!(!got.contains(&"dns-txt".to_string()), "{got:?}");
        assert!(!got.contains(&"well-known-url".to_string()), "{got:?}");
    }

    /// **D-D.** A real peer-id in the authority slot is a verification pin; a
    /// handle is a target. Same separator, decided by what the bytes decode as.
    #[test]
    fn the_authority_slot_tells_a_pin_from_a_handle_by_decoding_it() {
        let kp = entity_crypto::Keypair::from_seed([0x7A; 32]);
        let pid = kp.peer_id().as_str().to_string();
        let qualified = format!("alice@{pid}");

        let (local, auth) = split_qualified(&qualified);
        assert_eq!(local, "alice");
        assert_eq!(auth, Some(Authority::Pin(pid)));

        let (local, auth) = split_qualified("alice@entity-church");
        assert_eq!(local, "alice");
        assert_eq!(auth, Some(Authority::Handle("entity-church".into())));

        assert_eq!(split_qualified("alice"), ("alice", None));
    }

    /// The charset shortcut would have been wrong, and this pins why: a handle
    /// made only of Base58 characters is still a handle, because it does not
    /// *decode* to a supported multikey.
    #[test]
    fn a_base58_alphabet_handle_is_still_a_handle() {
        let (_, auth) = split_qualified("alice@entitychurch");
        assert_eq!(auth, Some(Authority::Handle("entitychurch".into())));
        assert!(handle_is_issuable("entitychurch"));
    }

    /// …and the residual collision is closed at issuance, where a human reads it.
    #[test]
    fn a_handle_shaped_like_a_peer_id_is_not_issuable() {
        let pid = entity_crypto::Keypair::from_seed([0x7B; 32]).peer_id().as_str().to_string();
        assert!(!handle_is_issuable(&pid), "a peer-id-shaped handle must be refused at issuance");
    }

    /// A local part containing `@` survives — we split on the last one.
    #[test]
    fn the_split_takes_the_last_separator() {
        let (local, auth) = split_qualified("odd@name@entity-church");
        assert_eq!(local, "odd@name");
        assert_eq!(auth, Some(Authority::Handle("entity-church".into())));
    }
}
