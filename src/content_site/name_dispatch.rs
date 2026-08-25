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
//! ## Status — RATIFIED, and still not a default chain entry
//!
//! **Our position carried** — `EXTENSION-REGISTRY` 1.7 → 1.8 (arch `8bfc9b6`)
//! re-keys §4.1 step 2 from **remoteness** to **name transmission**, and
//! `peer-issued` resolved per §6a.4 through the signed root is now an explicit
//! **MAY** in the catch-all. Arch's own reason for it being safe rather than a
//! loosening is worth keeping: §6a.4 makes verification *inside the signed tree*
//! mandatory and §6a.3a bars the host-served listing from being authoritative,
//! so **the kind fixes the mechanism** — a conformant `peer-issued` resolution
//! has no host-trusted-pointer path to fall back to.
//!
//! **[`default_rules`] is still not installed anywhere, and ratification did not
//! change that.** The blocker was never only the ruling: there is no default
//! registry to point a catch-all at. A deployment adds its registry explicitly
//! and [`validate_rules`] refuses the configuration that would leak. Shipping
//! our globs first is how two app tiers ship two — the failure §4.1 step 2 names.
//!
//! What the spec does **not** assert, deliberately, is zero disclosure: the walk
//! descends by the name's own hash, so an origin sees a **hash-prefix oracle** on
//! a miss and a public binding's blob on a hit — neither the queried string, and
//! neither reaching a name the registry does not carry. Arch declined D-A's *"nor
//! a reversible function of it"* wording for exactly that reason, and they were
//! right: it is falsifiable on our own hit path.

#![allow(dead_code)] // the evaluator ships ahead of B16b's surface, deliberately.

/// **§4.1's dispatch matcher, implemented here — deliberately NOT
/// `entity_registry::resolver::glob_match`.**
///
/// The grammar is CLOSED as of `EXTENSION-REGISTRY` v1.13: `*` matches any run
/// of characters including none, **every other byte is a literal** — `?`, `[`,
/// `]`, `\`, `.`, `:`, `@` and `/` match only themselves — any number of `*` is
/// permitted, `/` is **not** a separator, and the match spans the whole name.
///
/// The spec says implementations MUST NOT delegate this to a path-glob or
/// shell-glob library, and `glob_match` is that in local form: it gives `?`
/// single-character-wildcard meaning and `[a-c]` / `[!a-c]` character-class
/// meaning, neither of which this grammar grants. Delegating to it made
/// `a?c` match `abc` — a name the pattern must **not** reach, which in a
/// dispatch filter means a backend becomes eligible for names its operator
/// never made eligible. Pinned by [`tests::the_dispatch_grammar_is_closed`]
/// (conformance `REG-DISPATCH-GRAMMAR-1`).
///
/// **A matcher that merely omits those features and one that treats them as
/// literals are indistinguishable until a name or pattern carries one** — which
/// is why this is written out rather than assumed from the absence of a
/// character-class branch.
fn dispatch_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                // Collapse a run of stars; `**` is not a token here, it is just
                // two stars, and it means exactly what one means.
                let mut rest = p;
                while rest.first() == Some(&b'*') {
                    rest = &rest[1..];
                }
                if rest.is_empty() {
                    return true;
                }
                // `*` crosses EVERY byte, `/` included — the row that fails
                // against every path-glob implementation.
                (0..=t.len()).any(|i| go(rest, &t[i..]))
            }
            // No `?`, no `[…]`, no escapes. One literal byte, or no match.
            Some(c) => match t.first() {
                Some(d) if d == c => go(&p[1..], &t[1..]),
                _ => false,
            },
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

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
        // §2.4.1 IS the vocabulary. `pinned` and `did-key` used to sit here and
        // are not in it — they were never declared kinds, which is why v1.12
        // struck both from the shipped table. An undeclared token now falls to
        // the `_` arm and is treated as transmitting, which is the fail-closed
        // direction and matches §4.2 (an unknown kind is skipped with a warning,
        // never honored).
        "local-name" | "out-of-band" | "self-certifying" => Disclosure::Blind,
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

/// **§4.1b.1** — the enumerated typed suffixes, and the whole list.
///
/// Admitting a suffix is a **privacy decision**: it declares that every name a
/// user types ending in it may be disclosed to a third party. So the list grows
/// only by spec revision — not implementation-defined, not operator-extensible,
/// and *not inferable from a pattern's shape*, which is exactly the inference
/// our first classifier made and the reason it was wrong (see [`is_broad`]).
const TYPED_SUFFIXES: &[&str] = &[".eth"];

/// A pattern is **broad** when it can match at least one **bare** name — one
/// carrying no authority marker at all, which is what a user types when they
/// have named nobody.
///
/// **§4.1b (v1.19). NARROW iff at least one of:**
///
/// | # | condition |
/// |---|---|
/// | a | no `*` at all — it matches exactly one name |
/// | b | contains a literal `@` — every match carries an `@authority` |
/// | c | the literal head before the first `*` ends in `:` — every match carries a `scheme:` |
/// | d | ends in an enumerated typed suffix ([`TYPED_SUFFIXES`]) with no `*` after it |
///
/// Otherwise broad. Derived by arch from `GUIDE-RESOLUTION` §6.2 (*presence of
/// `@` ⇒ scoped; leading `scheme:` ⇒ typed system*) over §6.1's name shapes.
///
/// **Our first version got two rows wrong, in both directions, and only one of
/// them was the one arch named.** It asked "does this pattern *require* a
/// marker", spelled as `contains('@') || contains(':') || a fixed suffix`:
///
/// - `a.b` and `alice.eth` came back **broad** — a literal name matches exactly
///   one name and is the most explicit routing decision an operator can write.
///   Harmless in effect (it refuses a rule that should be allowed) but it makes
///   the privacy MUST reject legitimate config, which operators route around.
/// - **`*.lab` came back NARROW, and that is the direction that leaks.** Any
///   fixed trailing literal satisfied the old test, so a rule sending every
///   `*.lab` name to `did-web` would have been *accepted*. `.lab` is not a
///   naming system anyone opted into by typing it. §4.1b's answer is that the
///   line is not *"does a literal exist"* but *"does the literal identify an
///   authority or a naming system"* — hence an enumerated list, and an
///   unrecognized suffix makes the pattern broad (fail-safe: the cost is a
///   refused rule an operator rewrites, against silently disclosing a namespace
///   nobody reviewed).
///
/// The whole vector is pinned by `the_broad_classifier_matches_4_1b`.
pub fn is_broad(pattern: &str) -> bool {
    // (a) No wildcard: exactly one name.
    let Some(first_star) = pattern.find('*') else {
        return false;
    };
    // (b) A literal `@` anywhere.
    if pattern.contains('@') {
        return false;
    }
    // (c) The literal head, before the FIRST `*`, ends in `:`. Checked on the
    // head rather than "contains a colon" — `*.foo:bar` carries no scheme, and
    // the old `contains(':')` would have called it narrow.
    if pattern[..first_star].ends_with(':') {
        return false;
    }
    // (d) Ends in an enumerated typed suffix, with no `*` after it. The
    // no-`*`-after is what the `ends_with` gives us for free; `*.e*` fails here
    // because it does not end in a fixed suffix at all.
    if TYPED_SUFFIXES.iter().any(|s| pattern.ends_with(s)) {
        return false;
    }
    true
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
        if dispatch_match(&r.pattern, name) {
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

/// **The ratified §4.1a table — RATIFIED but still NOT installed, and those are
/// two different facts.**
///
/// Ratified: our D-A/D-B position carried (`EXTENSION-REGISTRY` 1.7 → 1.8, arch
/// `8bfc9b6`). §4.1 step 2 is re-keyed from **remoteness** to **name
/// transmission**, so `peer-issued` resolved per §6a.4 through the signed root
/// is an explicit **MAY** in the catch-all — which is row 6, and was the whole
/// proposal. Row 6 gained `"pinned"` when the spec's own table landed; a pin is
/// the user's own assertion and consults nothing, so it is `Disclosure::Blind`
/// and belongs beside `local-name`.
///
/// **Not installed, and ratification did not change that.** The blocker was
/// never only the ruling: there is **no default registry to point a catch-all
/// at**, and shipping our own before the ecosystem has one is exactly how two
/// app tiers ship two. B16b's surface stays fail-closed — a registry is added
/// explicitly and every entry carries a non-`*` pattern — until that exists.
/// This function is here so the table can be tested and reviewed as one object
/// rather than argued about in prose.
pub fn default_rules() -> Vec<DispatchRule> {
    vec![
        DispatchRule::new("did:web:*", &["did-web"]),
        DispatchRule::new("did:key:*", &["self-certifying"]),
        DispatchRule::new("*.eth", &["consensus-anchored"]),
        DispatchRule::new("*@*.*", &["dns-txt", "well-known-url"]),
        DispatchRule::new("*@*", &["peer-issued"]),
        DispatchRule::new("*", &["local-name", "self-certifying", "out-of-band", "peer-issued"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **§4.1b, every row, both directions** — the classifier ruled at v1.19
    /// after two conformant implementations split on four patterns and a third
    /// (ours) classified an exact literal as broad.
    ///
    /// Three groups, and each is load-bearing for a different reason:
    ///
    /// 1. **§4.1a's own six rows.** The check any implementation should run
    ///    first, and the one our old classifier already passed — which is
    ///    precisely why passing it proves nothing on its own.
    /// 2. **The five `REG-DISPATCH-CONFIG-REFUSED-1` row-7 patterns**, where
    ///    independent readings diverged. `*.lab` is the one that matters most
    ///    here: our old rule called it NARROW, so a chain sending every `*.lab`
    ///    name to `did-web` would have been accepted. That is the leaking
    ///    direction, and arch's write-up named only the harmless one.
    /// 3. **The conditions themselves**, including `*.foo:bar` — a colon that is
    ///    not a scheme. `contains(':')` called that narrow; §4.1b asks whether
    ///    the literal *head* ends in `:`.
    #[test]
    fn the_broad_classifier_matches_4_1b() {
        // (1) §4.1a's default table, row for row.
        for narrow in ["did:web:*", "did:key:*", "*.eth", "*@*.*", "*@*"] {
            assert!(!is_broad(narrow), "§4.1a row {narrow:?} must be NARROW");
        }
        assert!(is_broad("*"), "the catch-all is the canonical broad pattern");

        // (2) REG-DISPATCH-CONFIG-REFUSED-1 row 7 — the five that diverged.
        for broad in ["*.*", "*.e*", "*.lab"] {
            assert!(
                is_broad(broad),
                "{broad:?} must be BROAD — it matches bare dotted names, which §6a admits as \
                 legal local names, and `.lab` is not a naming system anyone opted into"
            );
        }
        for narrow in ["a.b", "alice.eth"] {
            assert!(
                !is_broad(narrow),
                "{narrow:?} must be NARROW by (a) — a literal matches exactly one name, and is \
                 the most explicit routing decision an operator can write"
            );
        }

        // (3) The four conditions, and the near-misses that separate them.
        assert!(!is_broad("bare.name.example"), "(a) no wildcard at all");
        assert!(!is_broad("*@example.com"), "(b) a literal @");
        assert!(!is_broad("scheme:*"), "(c) the literal head ends in `:`");
        assert!(
            is_broad("*.foo:bar"),
            "a colon that is not a scheme prefix must NOT narrow — the old `contains(':')` \
             test called this narrow"
        );
        assert!(
            is_broad("*.ETH"),
            "the suffix list is matched literally; a case variant is unrecognized, and an \
             unrecognized suffix is broad (fail-safe)"
        );
        assert!(is_broad("*.eth*"), "(d) requires no `*` after the suffix");
    }

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

    /// **Conformance `REG-DISPATCH-GRAMMAR-1` — the grammar is closed, and the
    /// two rows that matter are the ones a shell-glob passes wrongly.**
    ///
    /// `EXTENSION-REGISTRY` §4.1 v1.13: `*` matches any run including none;
    /// **every other byte is a literal**; any number of `*`; `/` is not a
    /// separator; anchored at both ends. We used to delegate this to
    /// `entity_registry::resolver::glob_match`, which implements `?` and
    /// `[a-c]`/`[!a-c]` — so `a?c` matched `abc`, a name the pattern must not
    /// reach. In a dispatch filter that is not cosmetic: a backend becomes
    /// eligible for names its operator never made eligible, and the catch-all
    /// MUST is the only thing standing between that and a leak.
    ///
    /// The spec's own note is why this is spelled out rather than inferred from
    /// the absence of a character-class branch: **a matcher that omits those
    /// features and one that treats them as literals are indistinguishable
    /// until a name or a pattern carries one.**
    #[test]
    fn the_dispatch_grammar_is_closed() {
        // Row 1 — `?` is a literal, not a single-character wildcard.
        assert!(dispatch_match("a?c", "a?c"), "`?` matches itself");
        assert!(!dispatch_match("a?c", "abc"), "`?` is NOT a wildcard");

        // Row 2 — `[` `]` are literals, not a character class.
        assert!(dispatch_match("a[bc]d", "a[bc]d"), "brackets match themselves");
        assert!(!dispatch_match("a[bc]d", "abd"), "brackets are NOT a class");

        // Row 3 — any number of `*`; our own table row 4 carries three.
        assert!(dispatch_match("*@*.*", "alice@example.com"));
        assert!(!dispatch_match("*@*.*", "noatsign"));

        // Row 4 — `*` crosses `/`. The row that fails every path-glob impl.
        assert!(dispatch_match("x*z", "x/y/z"), "`/` is not a separator here");

        // Anchored at both ends: there is no substring form.
        assert!(!dispatch_match("bc", "abcd"), "the match spans the WHOLE name");
        assert!(dispatch_match("*", ""), "`*` matches none, too");
        assert!(dispatch_match("**", "anything"), "`**` is two stars, not a token");

        // A backslash is a literal — there are no escapes to strip.
        assert!(dispatch_match(r"a\*", r"a\zz"), "the star still globs after a literal backslash");
        assert!(!dispatch_match(r"a\b", "ab"), "the backslash is not an escape");

        // And the whole point: this reaches the FILTER, not just the matcher.
        let rules = vec![DispatchRule::new("a?c", &["local-name"])];
        assert!(eligible_backends(&rules, "abc").is_empty(), "a literal `?` must not dispatch `abc`");
        assert_eq!(eligible_backends(&rules, "a?c"), vec!["local-name".to_string()]);
    }

    /// **The table is a ratified external contract now, so drift from it is a
    /// defect rather than a preference.** `EXTENSION-REGISTRY` §4.1a (1.8, arch
    /// `8bfc9b6`) fixes all six rows; our row 6 read `["local-name",
    /// "peer-issued"]` for a session after ratification and nothing caught it,
    /// because every other test here asserts a *property* of the table (nothing
    /// transmitting is eligible for a bare name) and every property held with a
    /// row missing. A property test cannot notice an omission that is still
    /// safe — `"pinned"` consults nothing, so leaving it out leaked nothing and
    /// simply made us quietly non-conformant.
    #[test]
    fn our_table_matches_the_ratified_4_1a_rows() {
        let ratified: Vec<(&str, Vec<&str>)> = vec![
            ("did:web:*", vec!["did-web"]),
            ("did:key:*", vec!["self-certifying"]),
            ("*.eth", vec!["consensus-anchored"]),
            ("*@*.*", vec!["dns-txt", "well-known-url"]),
            ("*@*", vec!["peer-issued"]),
            ("*", vec!["local-name", "self-certifying", "out-of-band", "peer-issued"]),
        ];
        let ours = default_rules();
        assert_eq!(ours.len(), ratified.len(), "row count drifted from §4.1a");
        for (i, (pattern, kinds)) in ratified.iter().enumerate() {
            assert_eq!(&ours[i].pattern, pattern, "§4.1a row {} pattern", i + 1);
            assert_eq!(&ours[i].backend_kinds, kinds, "§4.1a row {} backend_kinds", i + 1);
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
