//! **B16** — the static registry emitter: `entitychurchregistry.org` as files.
//!
//! `EXTENSION-REGISTRY` §7.4 says it outright — *"No live registry peer is
//! required at any step — the registry is itself a coral reef (a static
//! publisher in dormancy)."* This is that, emitted. A registry publishes
//! `name → peer-id` bindings signed by its own key; a consumer pins **only the
//! registry's peer-id**, resolves a name to a peer-id, and then walks *that*
//! peer's signed root for the content. Two hops, one pin, nothing live.
//!
//! **Naming and content are the same operation at different keys.** A registry
//! tree is an ordinary tree: the emitter below is the content emitter with
//! `system/registry/binding/…` in place of `sites/…`, and it signs a root with
//! the same [`RootProjector`]. That is why B16 needed no second mechanism.
//!
//! ## What is committed, and by what
//!
//! | artifact | key | authenticated by |
//! |---|---|---|
//! | binding body | `system/registry/binding/{hash}` | its content address |
//! | by-name index | `system/registry/binding/by-name/{norm}` | the binding's own signature |
//! | §5.2 signature | `system/signature/{binding-hash}` | the registry key |
//! | identity entity | (hash only) | its content address |
//!
//! **The association is committed by the per-binding signature, not by the
//! index.** Arch ruled this (`PROPOSAL-REGISTRY-NAME-ASSOCIATION-AND-THE-HOSTILE-HOST`
//! §4, 2026-08-18): the signature covers a body carrying `name`, so
//! `sig(R, {name, target_peer_id})` **is** the commitment. The finding we routed
//! (F1) was that resolvers *discard* it — they read the binding hash out of the
//! host-served by-name pointer and never compare `binding.name` to the name
//! asked for. Nothing an emitter can do fixes that; it is a resolver check, owed
//! by the three engines. We publish the commitment correctly and say so.
//!
//! A signed root is emitted anyway (arch ruled it **SHOULD**, not MUST): it costs
//! ~+4.7% over the leaves, and it is what makes a *withheld* revocation surface
//! as a broken walk rather than as an absence.
//!
//! ## `ttl` is not optional here
//!
//! §6a.4 permits `ttl: null`, and arch's D3 makes non-null a MUST for
//! `peer-issued` — because a null-TTL binding whose revocation a hostile origin
//! **withholds is permanently unrevokable** (our F2, and the reason the stated
//! *"bounded by ttl + revocation"* bound is void). `issued_at + ttl` is the one
//! check on this path a hostile byte-server cannot influence. This emitter
//! therefore has no way to express a null TTL — not a flag with a default, an
//! absence of the flag.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use entity_hash::Hash;
use entity_registry::data::{BindingData, KIND_PEER_ISSUED};
use entity_registry::{by_name_pointer_path, signature_pointer_path};

use super::paths::NAMES_LIST_PATH;
use super::signed_root::{RootProjector, SignedRootReport};

/// Default binding lifetime: 30 days, **in milliseconds**. Long enough that a
/// registry is not a weekly chore, short enough to bound a withheld revocation
/// to something an operator can state. See the module docs — this is a real
/// bound, not a formality.
///
/// **The unit is not cosmetic, and it is not documented anywhere obvious.**
/// `peer_issued::resolve_one` checks `issued_at + ttl <= now_ms()`, so
/// `issued_at` and `ttl` are milliseconds. Emitting seconds produces a binding
/// that is *always* expired — resolvable in no test that uses a real clock, and
/// silently dead in production. Found by running it, not by reading §6a.
pub const DEFAULT_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// One `name → peer-id` binding to issue.
#[derive(Debug)]
pub struct BindingSpec {
    pub name: String,
    pub target_peer_id: String,
    /// Where that peer's static publish is served — becomes the binding's
    /// `transports` entry. See [`http_poll_profile`] for why this is what turns
    /// a resolution into a fetchable thing.
    ///
    /// **Required** (arch D10): `None` is refused by [`emit_registry`] before
    /// anything is written. It stays an `Option` because the CLI parses
    /// `NAME=PEER_ID` and `NAME=PEER_ID@ORIGIN` into the same shape, and the
    /// refusal reports every missing one at once rather than the first.
    pub origin: Option<String>,
}

/// What a registry publish emitted.
#[derive(Debug)]
pub struct RegistryReport {
    pub registry_peer_id: String,
    pub bindings: Vec<(String, String)>,
    pub ttl_ms: u64,
    pub root: SignedRootReport,
}

/// Emit a static registry origin into `dir`.
///
/// `issued_at_ms` and `ttl_ms` are **milliseconds** (see [`DEFAULT_TTL_MS`]),
/// taken from the caller rather than read here — so a test can pin the clock and
/// a publish cannot silently depend on the clock of whatever box ran it.
pub fn emit_registry(
    dir: &Path,
    keypair: entity_crypto::Keypair,
    bindings: &[BindingSpec],
    ttl_ms: u64,
    issued_at_ms: u64,
) -> Result<RegistryReport, String> {
    if bindings.is_empty() {
        return Err("a registry with no bindings names nothing".into());
    }
    if ttl_ms == 0 {
        return Err("ttl must be non-zero: a binding that expires on issue names nothing, and \
                    a null/zero ttl is what makes a withheld revocation permanent"
            .into());
    }
    // **arch D10** (`PROPOSAL-REGISTRY-NAME-ASSOCIATION-AND-THE-HOSTILE-HOST`,
    // 2026-08-18): a `peer-issued` binding MUST carry non-empty `transports`.
    // Our own F6, confirmed — §4.1.2 promises a §6.5 transport-resolution
    // fall-through that has no static counterpart, because §6.5.4 makes profile
    // discovery out-of-band in v1. So for a statically published target there is
    // nothing to fall through TO: the binding resolves, and the consumer holds a
    // peer-id it cannot reach.
    //
    // Refused **before anything is written**, and refused rather than warned:
    // a warning on stdout is not a bound, and the operator who can fix this is
    // the one running the emitter. The §4.1.2 pin carve-out is untouched — a pin
    // is the user's own assertion, so reaching the peer is the user's problem.
    let no_transport: Vec<&str> = bindings
        .iter()
        .filter(|b| b.origin.is_none())
        .map(|b| b.name.as_str())
        .collect();
    if !no_transport.is_empty() {
        return Err(format!(
            "every peer-issued binding must carry an origin (arch D10) — {} name(s) have none: \
             {}. Use --bind=NAME=PEER_ID@ORIGIN; a binding with no transports resolves to a \
             peer-id the consumer has no way to reach",
            no_transport.len(),
            no_transport.join(", ")
        ));
    }

    // **The target must be a peer-id (audit F9), refused in the same place and
    // the same style as D10 above.** Nothing checked this, so a typo became a
    // *signed* binding naming a target that is not a peer at all — found by
    // making one: `--bind=example.test=foundation@…` (a slug where the id
    // belonged) emitted happily, and the only symptom arrived at the far end of
    // the chain as *"the named peer-id carries no public key"* on a stranger's
    // machine. The registry operator, the one person who can fix it, saw nothing.
    //
    // `PeerId::validate` is the right line and not a stricter one: it accepts
    // BOTH Ed25519 forms, so a legacy SHA-256-form target — which our own
    // consumer cannot pin without an out-of-band key, but which is a perfectly
    // legal binding another consumer can use — is still issuable. What is
    // refused is a string that is not a peer-id in any form.
    let bad_target: Vec<String> = bindings
        .iter()
        .filter_map(|b| {
            entity_crypto::PeerId::from(b.target_peer_id.clone())
                .validate()
                .err()
                .map(|e| format!("{} → {:?} ({e})", b.name, b.target_peer_id))
        })
        .collect();
    if !bad_target.is_empty() {
        return Err(format!(
            "{} binding(s) name a target that is not a peer-id: {}. A registry signs \
             name → peer-id; signing a name → typo produces a binding whose only symptom is a \
             resolver failure on someone else's machine",
            bad_target.len(),
            bad_target.join("; ")
        ));
    }

    let mut root = RootProjector::new(keypair)?;
    let rid = root.peer_id().to_string();

    // The signer's identity entity. `verify_signed_by_registry` resolves the
    // registry's public key from it, so a projection without it publishes
    // signatures that verify nothing.
    root.publish_identity()?;

    let mut issued = Vec::new();
    for spec in bindings {
        let name = normalized(&spec.name);
        // §6.3 name-path safety, at the emitter, where a human can fix it: a
        // name carrying `/` or a control char makes the by-name key ambiguous
        // with the path structure it is written into.
        entity_registry::data::validate_name_safety(&name)
            .map_err(|e| format!("name {:?}: {e}", spec.name))?;
        let binding = BindingData {
            name: name.clone(),
            kind: KIND_PEER_ISSUED.into(),
            target_peer_id: spec.target_peer_id.clone(),
            transports: spec
                .origin
                .as_deref()
                .map(|o| vec![http_poll_profile(&spec.target_peer_id, o)])
                .unwrap_or_default(),
            issued_at: issued_at_ms,
            // Non-null, always — see the module docs (arch D3).
            ttl: Some(ttl_ms),
            supersedes: None,
            issuer_attestation: None,
            metadata: None,
        };
        let entity = binding.to_entity().map_err(|e| format!("binding {name} encodes: {e:?}"))?;
        let binding_hash = entity.content_hash;

        // §6a.3: the body at its content-addressed key…
        write_entity(dir, &rid, &body_key(&binding_hash), &entity)?;
        root.record(&rid, &body_key(&binding_hash), &entity);

        // …and the by-name index pointing at it. One entity, two keys.
        let by_name = rel(&rid, &by_name_pointer_path(&rid, &name));
        write_pointer(dir, &rid, &by_name, &binding_hash)?;
        root.record_hash(&rid, &by_name, binding_hash);

        // §5.2: the authenticating signature at the invariant pointer. THIS is
        // the commitment to `name → target_peer_id` — the body it covers carries
        // both.
        let (signature, signer) = root.sign_detached(&binding_hash);
        let sig = entity_types::SignatureData {
            target: binding_hash,
            signer,
            algorithm: root.algorithm().into(),
            signature,
        };
        let sig_entity = sig.to_entity().map_err(|e| format!("signature encodes: {e:?}"))?;
        let sig_key = rel(&rid, &signature_pointer_path(&rid, &binding_hash));
        write_entity(dir, &rid, &sig_key, &sig_entity)?;
        root.record(&rid, &sig_key, &sig_entity);

        issued.push((name, spec.target_peer_id.clone()));
    }

    // The registry's enumeration artifact — the sibling of a peer's
    // `sites.list`, and what a registry BROWSER reads.
    //
    // **It is a menu, not the authority** — arch D9, and our F4 was wrong about
    // why. We filed "a signed root cannot be enumerated"; it can. The HAMT leaf
    // is `[key, value_hash]` (`EXTENSION-TREE` §3.1 fixture #2), so the key set
    // is in the nodes, and `entity_tree::trie::collect_all_bindings` recovers it
    // — measured in `the_name_set_is_recoverable_from_the_signed_root_alone`.
    // What we had built was `signed_root::trie_closure`, which scavenges 33-byte
    // windows for *hashes*: it proves the closure is projected and recovers no
    // keys. Structure-agnostic by design, and that design is why we did not see
    // that the decoder next door already answered the question.
    //
    // The list stays because it is one fetch instead of O(N) and it is the right
    // first-paint artifact. Every name it offers is then resolved through the
    // signed root, so an invented name fails; a HIDDEN one is the open half —
    // see `withholding_a_trie_node_shortens_the_walk_silently`, which measures
    // that the walk does not currently catch that either. Amendment 5 names the
    // shape: `{path}{tree_listing_suffix}`, no trailing slash, so a static CDN
    // serves it.
    write_names_list(dir, &rid, &issued)?;

    let report = root.finish(dir)?;
    Ok(RegistryReport {
        registry_peer_id: rid,
        bindings: issued,
        ttl_ms,
        root: report,
    })
}

/// §6a name normalization for the by-name key — **the resolver's own function,
/// with the resolver's own argument.**
///
/// `peer_issued::resolve_one` calls `normalize_name(name, "none")`: NFC, and
/// **no case folding** (case is a local-name-config knob, not a peer-issued
/// concern). An emitter that lowercased would publish `foundation.example` for
/// an operator who typed `Foundation.Example`, and the resolver — building the
/// by-name path from what the *user* typed — would never find it. Emitting
/// through the same call is the only thing that keeps the two ends agreeing;
/// re-deriving "what normalization probably means" is how they drift.
fn normalized(name: &str) -> String {
    entity_registry::data::normalize_name(name.trim(), "none")
}

/// Write `{dir}/{registry}/system/registry/binding/by-name.list` — every name
/// this registry carries, sorted, one per line. See the call site for why it is
/// transport-trusted and why that is safe.
fn write_names_list(dir: &Path, rid: &str, issued: &[(String, String)]) -> Result<(), String> {
    let mut names: Vec<&str> = issued.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let mut body = names.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let path = dir.join(rid).join(format!("{NAMES_LIST_PATH}.list"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The binding's `transports` entry for a statically-published peer — an
/// **`EXTENSION-NETWORK` §6.5.3 `http-poll` profile**, which is precisely what a
/// static origin is.
///
/// **Why this field is the difference between a resolution and a fetch.**
/// `resolve(name)` hands back a *peer-id*, and a peer-id is an identity, not an
/// address. Without transports a consumer knows **who** but not **where**, and
/// every origin has to be pre-registered out of band (which is what
/// `entity-deployment.json`'s `origins` map does today). With it, one pinned
/// registry is enough to reach a domain nobody configured.
///
/// Our projection matches the profile field-for-field, which is not a
/// coincidence — both describe the same Amendment-5 pathing:
/// `content_layout: "sharded-2-4"`, `tree_leaf_suffix: ".bin"`,
/// `tree_listing_suffix: ".list"`.
///
/// **`freshness` is `static-immutable+signed-pointer`, and we are entitled to
/// say so** only because B14 emits the signed root *and* its trie closure —
/// Amendment 10 makes that closure a MUST for any publisher advertising a
/// `signed_pointer`, on the finding that a path-bound serve scope 404s on
/// interior nodes and halts the consumer's walk.
pub fn http_poll_profile(peer_id: &str, origin: &str) -> entity_ecf::Value {
    let origin = origin.trim_end_matches('/');
    entity_ecf::cbor_map! {
        "peer_id" => entity_ecf::Value::Text(peer_id.to_string()),
        "transport_type" => entity_ecf::Value::Text("http-poll".into()),
        "endpoint" => entity_ecf::cbor_map! {
            "tree_url_prefix" => entity_ecf::Value::Text(format!("{origin}/{peer_id}")),
            "content_url_prefix" => entity_ecf::Value::Text(format!("{origin}/content")),
            "content_layout" => entity_ecf::Value::Text("sharded-2-4".into()),
            "tree_leaf_suffix" => entity_ecf::Value::Text(".bin".into()),
            "tree_listing_suffix" => entity_ecf::Value::Text(".list".into()),
            "manifest_url_prefix" => entity_ecf::Value::Text(format!(
                "{origin}/{peer_id}/{}",
                crate::content_site::paths::PUBLISHED_ROOT_REL
            )),
        },
        "supported_ops" => entity_ecf::Value::Array(vec![
            entity_ecf::Value::Text("TREE_GET".into()),
            entity_ecf::Value::Text("CONTENT_GET".into()),
            entity_ecf::Value::Text("MANIFEST_GET".into()),
        ]),
        "freshness" => entity_ecf::Value::Text("static-immutable+signed-pointer".into()),
        "nonce_required" => entity_ecf::Value::Bool(false),
        "cap_flow" => entity_ecf::Value::Text("egress".into()),
    }
}

fn body_key(h: &Hash) -> String {
    format!("system/registry/binding/{}", h.to_hex())
}

/// Strip the `/{peer}/` qualification — trie keys and projected paths are both
/// relative to it, while the upstream path helpers return absolute paths.
fn rel(peer_id: &str, absolute: &str) -> String {
    absolute
        .strip_prefix(&format!("/{peer_id}/"))
        .unwrap_or(absolute)
        .to_string()
}

/// A body + its `system/hash` pointer — the same two-hop convention the content
/// emitter writes.
fn write_entity(
    dir: &Path,
    peer_id: &str,
    key: &str,
    ent: &entity_entity::Entity,
) -> Result<(), String> {
    super::publish_fixture::write_entity(dir, peer_id, key, ent, None)
        .map_err(|e| format!("write {key}: {e}"))
}

/// A pointer at `key` naming an already-written body. No new blob.
fn write_pointer(dir: &Path, peer_id: &str, key: &str, hash: &Hash) -> Result<(), String> {
    super::publish_fixture::write_hash_pointer(dir, peer_id, key, hash)
        .map_err(|e| format!("write pointer {key}: {e}"))
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// `entity-browser registry OUT_DIR --bind NAME=PEER_ID [--bind …] [--ttl-days=N]`
///
/// Identity resolution matches `publish`: the durable publisher keypair by
/// default, `--identity-seed=hex` / `--demo-identity` to override. **A registry
/// SHOULD NOT share an identity with a content publisher** — the two answer
/// different questions and a consumer pins them separately — so the default is
/// a distinct `registry/` keypair, and sharing one takes an explicit seed.
/// What a `registry` command line asked for.
///
/// **This is a separate function from [`run`] on purpose.** The CLI's usage
/// strings and refusals *are* a surface, and until audit F2 no test read one — so
/// the printed help drifted into teaching a `--bind` spelling the parser rejected,
/// and the refusal for it claimed no binding had been passed at all. Parsing here
/// returns the operator-facing message as a value, which is the only way a gate
/// can assert on it (`AUDIT-NAMING-AND-PUBLISHING-ARC-2026-08-18` §5, AP25).
#[derive(Debug)]
pub struct RegistryArgs {
    pub out_dir: std::path::PathBuf,
    pub bindings: Vec<BindingSpec>,
    pub ttl_ms: u64,
    /// Inspect an already-emitted tree instead of writing one.
    pub verify: bool,
}

/// Parse a `registry` command line. `Err` is the message to print, verbatim.
pub fn parse_registry_args(args: &[String]) -> Result<RegistryArgs, String> {
    // **Every flag here takes `=`** — and a bare `--bind` used to be dropped by
    // the `strip_prefix("--bind=")` filter below, which then reported *"at least
    // one --bind is required"* to someone who had just passed four. The printed
    // help taught the space form, so following it could not succeed (audit F2).
    // Checked BEFORE `out_dir`, because with a space form the *value* is the
    // first non-flag argument and would otherwise be adopted as the output
    // directory — a wrong answer instead of a refusal.
    if let Some(bare) =
        args.iter().find(|a| matches!(a.as_str(), "--bind" | "--ttl-days" | "--identity-seed"))
    {
        return Err(format!(
            "{bare} takes `=`, not a space — write {bare}=VALUE. (With a space the value becomes \
             a positional argument, which would have been read as the output directory.)"
        ));
    }

    let out_dir = match args.iter().skip(1).find(|a| !a.starts_with("--")) {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            return Err("an output directory is required\n  entity-browser registry OUT_DIR \
                        --bind=NAME=PEER_ID@ORIGIN [--bind=...]"
                .into())
        }
    };
    let verify = args.iter().any(|a| a == "--verify");

    let mut bindings = Vec::new();
    for a in args.iter().filter_map(|a| a.strip_prefix("--bind=")) {
        // NAME=PEER_ID, or NAME=PEER_ID@ORIGIN to publish where that peer is
        // served. `@` is unambiguous here: it appears in neither Base58 nor a
        // bare origin's scheme+host. Split on the FIRST `@`, which is the
        // correct end here: a Base58 peer-id cannot contain one, while an origin
        // can (`http://user@host`), so the first is always the separator and the
        // last would eat into the origin.
        match a.split_once('=') {
            Some((name, rest)) if !name.is_empty() && !rest.is_empty() => {
                let (target, origin) = match rest.split_once('@') {
                    Some((t, o)) if !t.is_empty() && !o.is_empty() => {
                        (t.to_string(), Some(o.to_string()))
                    }
                    Some(_) => {
                        return Err(format!(
                            "--bind=NAME=PEER_ID@ORIGIN needs both sides, got {a:?}"
                        ))
                    }
                    None => (rest.to_string(), None),
                };
                bindings
                    .push(BindingSpec { name: name.to_string(), target_peer_id: target, origin });
            }
            _ => return Err(format!("--bind expects NAME=PEER_ID@ORIGIN, got {a:?}")),
        }
    }
    // A verify reads a tree that already exists, so it needs no bindings — and
    // demanding them would make the check unreachable for the person most likely
    // to want it (someone verifying a directory they were handed).
    if bindings.is_empty() && !verify {
        return Err("at least one --bind=NAME=PEER_ID@ORIGIN is required\n  the @ORIGIN half is \
                    not optional (arch D10) — it becomes the binding's http-poll transport \
                    profile, and without it a consumer resolves WHO but not WHERE"
            .into());
    }

    let ttl_ms = match args.iter().find_map(|a| a.strip_prefix("--ttl-days=")) {
        Some(d) => match d.parse::<u64>() {
            Ok(n) if n > 0 => n * 24 * 60 * 60 * 1000,
            _ => return Err(format!("--ttl-days expects a positive integer, got {d:?}")),
        },
        None => DEFAULT_TTL_MS,
    };

    Ok(RegistryArgs { out_dir, bindings, ttl_ms, verify })
}

pub fn run(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;

    let parsed = match parse_registry_args(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("registry: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let RegistryArgs { out_dir, bindings, ttl_ms, verify } = parsed;

    let keypair = match super::publish::resolve_registry_keypair(args) {
        Ok(kp) => kp,
        Err(e) => {
            eprintln!("registry: {e}");
            return ExitCode::FAILURE;
        }
    };

    // **`--verify` belongs on THIS verb, not on `publish`** — and that is a
    // correctness point, not ergonomics. The two verbs resolve *different*
    // durable identities (`persistence::publisher_keypair` vs
    // `registry_keypair`), so `publish <registry-dir> --verify` with no
    // `--identity-seed=` looks for `{out}/{publisher-peer}/` — a directory the
    // registry emit never wrote — and would report a clean tree as unverifiable.
    // Whoever owns the identity owns the verification (audit F1/F3).
    if verify {
        // `Keypair` is not `Clone`, and the peer-id must come from the SAME
        // derivation the emit used (`RootProjector` → `PeerBuilder`), not from a
        // second one that merely looks equivalent — the directory name is that
        // id. So build the projector to derive it, then resolve a fresh keypair
        // for the verify; both resolutions read the same seed or the same durable
        // file, so they cannot disagree.
        let peer_id = match RootProjector::new(keypair) {
            Ok(p) => p.peer_id().to_string(),
            Err(e) => {
                eprintln!("registry --verify: {e}");
                return ExitCode::FAILURE;
            }
        };
        let key = match super::publish::resolve_registry_keypair(args) {
            Ok(kp) => kp,
            Err(e) => {
                eprintln!("registry --verify: {e}");
                return ExitCode::FAILURE;
            }
        };
        // No prefix: a registry publishes at the root of its own origin.
        return super::publish::run_verify(&out_dir, &peer_id, "", &key);
    }

    let issued_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    match emit_registry(&out_dir, keypair, &bindings, ttl_ms, issued_at_ms) {
        Ok(r) => {
            println!(
                "registry published → {} ({} binding(s), ttl {} day(s))",
                out_dir.display(),
                r.bindings.len(),
                r.ttl_ms / (24 * 60 * 60 * 1000)
            );
            for (name, target) in &r.bindings {
                println!("  {name} → {target}");
            }
            println!("  registry peer: {}", r.registry_peer_id);
            println!(
                "  signed root: {} (seq {}, {} keys, {} trie nodes)",
                &r.root.head_hex[..16.min(r.root.head_hex.len())],
                r.root.seq,
                r.root.keys,
                r.root.trie_nodes
            );
            println!();
            println!(
                "  A consumer pins ONLY this peer-id. It is not an authority — it is one \
                 entry in a resolver chain, and a name it does not carry falls through."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("registry: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use crate::content_site::publish_fixture::emit_owned_sites;
    use crate::content_site::read::OwnedSite;
    use crate::content_site::signed_root::{DirFetcher, RootProjector};
    use entity_peer::published_root::PublishedRootClient;
    use entity_registry::data::ResolverChainEntry;
    use entity_registry::{peer_issued, BACKEND_KIND_PEER_ISSUED};
    use entity_store::{ContentStore, LocationIndex, MemoryContentStore, MemoryLocationIndex};
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::Arc;

    /// A pinned past issue time, for the tests that only decode a body or that
    /// want expiry. **Not usable for a resolve**: `resolve_one` enforces
    /// `issued_at + ttl > now()`, so a live binding has to be issued against a
    /// real clock — the same thing the CLI does.
    const ISSUED_AT_MS: u64 = 1_760_000_000_000;
    const NAME: &str = "entitychurch.example";
    /// Every emitted binding carries an origin now (arch D10), so the fixtures
    /// that are not *about* transports still have to supply one.
    const TEST_ORIGIN: &str = "https://origin.example";

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// A **real** target peer-id for the fixtures.
    ///
    /// These used to say `"2PEERTARGET"`, which is not a peer-id in any form —
    /// and nothing noticed until the emitter started checking (audit F9). Worth
    /// keeping the lesson attached: a fixture that could not exist in production
    /// proves less than it appears to, and here it was hiding the fact that the
    /// emitter would sign a binding naming a string that is not a peer.
    fn target_pid() -> String {
        entity_crypto::Keypair::from_seed([0x7A; 32]).peer_id().as_str().to_string()
    }

    /// Publish a content domain into `dir`, returning `(peer_id, pubkey, key_type)`.
    fn domain(dir: &Path, seed: u8) -> (String, Vec<u8>, entity_crypto::KeyType) {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let kt = kp.key_type();
        let mut root = RootProjector::new(kp).unwrap();
        let peer_id = root.peer_id().to_string();
        let site = OwnedSite {
            peer_id: peer_id.clone(),
            site_id: "home".into(),
            manifest: SiteManifest::new("home", "Home", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![("index".into(), SitePage::markdown("Home", "# named bytes"))],
            assets: vec![],
        };
        emit_owned_sites(dir, std::slice::from_ref(&site), "", Some(&mut root)).unwrap();
        root.finish(dir).unwrap();
        (peer_id, pubkey, kt)
    }

    /// The consumer's local store, warmed from an **emitted registry directory**.
    ///
    /// Faithful to `peer_issued::warm_cache`'s split, which is the whole point:
    /// `read_path` is **host-trusted** (pointers land as served) while
    /// `read_content` is **hash-verified**. Weakening either half here would
    /// make the gate prove something the deployment does not have.
    fn warm_from_dir(dir: &Path, peer_id: &str) -> (Arc<dyn ContentStore>, Arc<dyn LocationIndex>) {
        let cs: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        let li: Arc<dyn LocationIndex> = Arc::new(MemoryLocationIndex::new());

        for blob in walk(&dir.join("content")) {
            let name = blob.file_name().unwrap().to_string_lossy().to_string();
            let Some(want) = hash_from_hex(&name) else { continue };
            let Ok(bytes) = fs::read(&blob) else { continue };
            if let Ok(entity) = entity_peer::published_root::verify_content(&bytes, &want) {
                cs.put(entity).expect("content put");
            }
        }
        for ptr in walk(&dir.join(peer_id)) {
            if ptr.extension().and_then(|s| s.to_str()) != Some("bin") {
                continue;
            }
            let Ok(bytes) = fs::read(&ptr) else { continue };
            let Ok(hash) = crate::content_site::http_poll::crack_pointer(&bytes) else { continue };
            let rel = ptr.strip_prefix(dir).unwrap().to_string_lossy().replace(".bin", "");
            li.set(&format!("/{rel}"), hash);
        }
        (cs, li)
    }

    /// `Hash` has `to_hex` but no inverse; a blob's filename IS its hex address,
    /// and `verify_content` needs the expected hash to check against — so the
    /// name is parsed as a *claim* and the re-hash is what settles it.
    fn hash_from_hex(hex: &str) -> Option<Hash> {
        if hex.len() % 2 != 0 {
            return None;
        }
        let bytes: Option<Vec<u8>> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();
        Hash::from_bytes(&bytes?).ok()
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    fn chain_entry(registry_id: &str) -> ResolverChainEntry {
        ResolverChainEntry {
            backend_kind: BACKEND_KIND_PEER_ISSUED.into(),
            backend_id: registry_id.into(),
            priority: 0,
            accepted_trust_anchors: vec![format!("peer_issued:{registry_id}")],
            hints: None,
        }
    }

    /// Emit one live binding — issued now, so it is inside its TTL.
    fn emit(dir: &Path, name: &str, target: &str, ttl: u64) -> RegistryReport {
        emit_at(dir, name, target, ttl, now_ms())
    }

    fn emit_at(dir: &Path, name: &str, target: &str, ttl: u64, issued_at: u64) -> RegistryReport {
        emit_registry(
            dir,
            entity_crypto::Keypair::from_seed([0xE5; 32]),
            &[BindingSpec { name: name.into(), target_peer_id: target.into(), origin: Some(TEST_ORIGIN.into()) }],
            ttl,
            issued_at,
        )
        .expect("registry emits")
    }

    /// **The B16 gate.** A name typed by a consumer that holds only the
    /// registry's peer-id comes back as a domain peer-id, and that peer-id walks
    /// its own signed root to the authored page. Three static directories,
    /// nothing live.
    #[test]
    fn a_published_registry_resolves_a_name_to_a_domain_that_serves_the_page() {
        let reg_dir = tempfile::tempdir().unwrap();
        let dom_dir = tempfile::tempdir().unwrap();
        let (domain_id, pubkey, kt) = domain(dom_dir.path(), 0xA1);

        let report = emit(reg_dir.path(), NAME, &domain_id, DEFAULT_TTL_MS);
        let rid = report.registry_peer_id.clone();

        // Hop 1 — the name.
        let (cs, li) = warm_from_dir(reg_dir.path(), &rid);
        let result = peer_issued::resolve_one(&cs, &li, &chain_entry(&rid), NAME)
            .expect("the backend answers");
        assert!(result.is_resolved(), "status was {}", result.status);
        assert_eq!(
            result.trust_anchor.as_deref(),
            Some(format!("peer_issued:{rid}").as_str()),
            "a resolved binding must carry the registry's own trust anchor"
        );
        assert_eq!(result.peer_id.as_deref(), Some(domain_id.as_str()));

        // Hop 2 — the content, off the DOMAIN's root. Nothing here consults the
        // registry again: the pin for the tree is the peer-id we just learned.
        let client = PublishedRootClient::new(
            DirFetcher::new(dom_dir.path(), &domain_id),
            pubkey,
            kt,
            Some(domain_id.clone()),
        );
        let page = client
            .resolve("sites/home/pages/index")
            .expect("walk")
            .expect("the named domain serves the page");
        assert!(
            String::from_utf8_lossy(&entity_ecf::ecf_for_hash(&page.entity_type, &page.data))
                .contains("named bytes"),
            "the page the name resolved to must be the authored one"
        );
    }

    /// **`ttl: null` is not expressible, and that is the point.** Arch's D3: a
    /// null-TTL peer-issued binding whose revocation a hostile origin withholds
    /// is *permanently* unrevokable, which is why the *"bounded by ttl +
    /// revocation"* argument is void. `issued_at + ttl` is the one check on this
    /// path a byte-server cannot influence, so the emitter must always set it.
    #[test]
    fn every_issued_binding_carries_a_non_null_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let report = emit_at(dir.path(), NAME, &target_pid(), DEFAULT_TTL_MS, ISSUED_AT_MS);
        let (cs, li) = warm_from_dir(dir.path(), &report.registry_peer_id);

        let path = entity_registry::by_name_pointer_path(&report.registry_peer_id, NAME);
        let hash = li.get(&path).expect("the by-name pointer is projected");
        let entity = cs.get(&hash).expect("the binding body is projected");
        let binding = entity_registry::data::BindingData::from_entity(&entity)
            .expect("the body decodes as a binding");

        assert_eq!(binding.ttl, Some(DEFAULT_TTL_MS), "ttl must be set, and to what we asked");
        assert_eq!(binding.issued_at, ISSUED_AT_MS);
        assert_eq!(binding.name, NAME, "the signed body carries the name — this IS the association");
    }

    /// A zero TTL is refused at the emitter rather than published. A binding
    /// that expires on issue names nothing, and there is no reason to let a
    /// registry emit one.
    #[test]
    fn a_zero_ttl_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let err = emit_registry(
            dir.path(),
            entity_crypto::Keypair::from_seed([0xB2; 32]),
            &[BindingSpec { name: NAME.into(), target_peer_id: target_pid(), origin: Some(TEST_ORIGIN.into()) }],
            0,
            ISSUED_AT_MS,
        )
        .expect_err("a zero ttl must be refused");
        assert!(err.contains("ttl"), "{err}");
        assert!(
            !dir.path().join("content").exists(),
            "a refused registry must write nothing"
        );
    }

    /// **The mutation check.** Expiry is enforced against the emitted `ttl`, so
    /// a binding published with a short life stops resolving once
    /// `issued_at + ttl` is behind the clock. This is what makes the TTL a real
    /// bound on a withheld revocation rather than a field nobody reads.
    #[test]
    fn an_expired_binding_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        // Issued far enough in the past that any real clock is past issued_at+ttl.
        let report = emit_registry(
            dir.path(),
            entity_crypto::Keypair::from_seed([0xC3; 32]),
            &[BindingSpec { name: NAME.into(), target_peer_id: target_pid(), origin: Some(TEST_ORIGIN.into()) }],
            60_000,
            1_000_000_000,
        )
        .expect("emits");
        let (cs, li) = warm_from_dir(dir.path(), &report.registry_peer_id);
        let result = peer_issued::resolve_one(&cs, &li, &chain_entry(&report.registry_peer_id), NAME);
        assert!(
            !matches!(&result, Some(r) if r.is_resolved()),
            "a binding whose issued_at+ttl has passed must not resolve: {:?}",
            result.map(|r| r.status)
        );
    }

    /// The registry's own tree is **just a tree** — walkable from its signed
    /// root like any other publish. This is the §7.4 property that makes
    /// "naming and content are the same operation at different keys" concrete,
    /// and it is what a *committed* by-name association would rest on.
    #[test]
    fn the_registrys_own_tree_walks_from_its_signed_root() {
        let dir = tempfile::tempdir().unwrap();
        let kp = entity_crypto::Keypair::from_seed([0xD4; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let kt = kp.key_type();
        let report = emit_registry(
            dir.path(),
            kp,
            &[BindingSpec { name: NAME.into(), target_peer_id: target_pid(), origin: Some(TEST_ORIGIN.into()) }],
            DEFAULT_TTL_MS,
            ISSUED_AT_MS,
        )
        .expect("emits");

        let client = PublishedRootClient::new(
            DirFetcher::new(dir.path(), &report.registry_peer_id),
            pubkey,
            kt,
            Some(report.registry_peer_id.clone()),
        );
        assert!(client.fetch_root().is_ok(), "the registry's root must verify against its key");

        let key = rel(
            &report.registry_peer_id,
            &entity_registry::by_name_pointer_path(&report.registry_peer_id, NAME),
        );
        let got = client.resolve(&key).expect("walk").expect("by-name key is in the signed tree");
        let binding = entity_registry::data::BindingData::from_entity(&got)
            .expect("the by-name key yields the binding itself");
        assert_eq!(binding.name, NAME);
    }

    /// **F4, measured.** Arch ruled (`ROUTING-2026-08-18-b` §2) that a signed
    /// root *is* enumerable — `EXTENSION-TREE` §3.1's leaf is `[key, value_hash]`,
    /// so the key is in the node — and asked to hear it before the fold if the
    /// walk does not recover the key set in practice. It does.
    ///
    /// `entity_tree::trie::collect_all_bindings` is §3.5's
    /// `walk_entry_collect_bindings` analog, and over a store holding only what
    /// the origin serves it returns every emitted name. **This is not what our
    /// `--verify` walk was**: that one is `signed_root::trie_closure`, which
    /// scavenges 33-byte windows for *hashes* and is deliberately
    /// structure-agnostic. It proves the closure is projected; it recovers no
    /// keys. The enumeration needed the decoder, and the decoder was already
    /// upstream — so arch's conclusion holds and its premise about our code
    /// does not.
    #[test]
    fn the_name_set_is_recoverable_from_the_signed_root_alone() {
        let dir = tempfile::tempdir().unwrap();
        let kp = entity_crypto::Keypair::from_seed([0xF4; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let kt = kp.key_type();
        let names = ["a.example", "b.example", "c.example", "d.example", "e.example"];
        let specs: Vec<BindingSpec> = names
            .iter()
            .map(|n| BindingSpec {
                name: (*n).into(),
                target_peer_id: target_pid(),
                origin: Some("https://origin.example".into()),
            })
            .collect();
        let report = emit_registry(dir.path(), kp, &specs, DEFAULT_TTL_MS, now_ms()).expect("emits");

        let (cs, _li) = warm_from_dir(dir.path(), &report.registry_peer_id);
        let client = PublishedRootClient::new(
            DirFetcher::new(dir.path(), &report.registry_peer_id),
            pubkey,
            kt,
            Some(report.registry_peer_id.clone()),
        );
        let root = client.fetch_root().expect("the registry root verifies");

        let recovered = recovered_names(cs.as_ref(), root.root_hash, &report.registry_peer_id);
        let want: BTreeSet<String> = names.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            recovered, want,
            "the trie walk must recover exactly the emitted name set — D9's premise"
        );
    }

    /// **The half of D9 that does NOT hold, and it is the load-bearing half.**
    ///
    /// Arch's argument for preferring the walk over the served `.list` is that
    /// *"hiding a name requires withholding a node, so the walk fails visibly
    /// rather than returning a short list."* Measured against the primitive arch
    /// cites, it does not: `collect_bindings_into` skips a missing `Entry::Link`
    /// with a bare `if let Some(..)`, recording nothing. A hostile origin drops
    /// one interior node and the enumeration returns a **short list, silently** —
    /// the exact property the `.list` was faulted for.
    ///
    /// The name is not merely hidden from the listing: it is still *resolvable*
    /// by a consumer that already knows it, because its by-name key may live in
    /// a different branch. So the two surfaces disagree with no error anywhere.
    ///
    /// Note what this is NOT: it is not a claim that enumeration is impossible.
    /// The signed root commits to the complete key set, and a walk that treats a
    /// missing link as a **failure** recovers the property arch describes. The
    /// defect is in the collector's error handling, one layer down — same shape
    /// as B14 (`Ok(None)` for an unwalkable tree) and as our own `--verify`
    /// incomplete-closure hole, which is the third time this seam has produced
    /// "absent" where it meant "withheld".
    #[test]
    fn withholding_a_trie_node_shortens_the_walk_silently() {
        let dir = tempfile::tempdir().unwrap();
        let kp = entity_crypto::Keypair::from_seed([0xF5; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let kt = kp.key_type();
        let names: Vec<String> = (0..24).map(|i| format!("n{i:02}.example")).collect();
        let specs: Vec<BindingSpec> = names
            .iter()
            .map(|n| BindingSpec {
                name: n.clone(),
                target_peer_id: target_pid(),
                origin: Some("https://origin.example".into()),
            })
            .collect();
        // **A PINNED clock.** `issued_at` goes into every binding body, so a real
        // clock gives every run different binding hashes — different trie keys,
        // a different HAMT shape, and a different set of names under any given
        // link. The first version of this test used `now_ms()` and failed on
        // roughly the runs where the chosen link happened to hold no by-name key.
        let report =
            emit_registry(dir.path(), kp, &specs, DEFAULT_TTL_MS, ISSUED_AT_MS).expect("emits");

        let (cs, _li) = warm_from_dir(dir.path(), &report.registry_peer_id);
        let client = PublishedRootClient::new(
            DirFetcher::new(dir.path(), &report.registry_peer_id),
            pubkey,
            kt,
            Some(report.registry_peer_id.clone()),
        );
        let root = client.fetch_root().expect("the registry root verifies");
        let full = recovered_names(cs.as_ref(), root.root_hash, &report.registry_peer_id);
        assert_eq!(full.len(), names.len(), "precondition: the full walk sees every name");

        // Withhold exactly one interior node that is NOT the root — the move a
        // hostile origin has, since it serves the bytes and the root's own hash
        // is pinned by the signature. Chosen by *effect* rather than by position:
        // a link is only a witness if hiding it actually hides a name, and which
        // link that is depends on the HAMT shape.
        let victim = hiding_link_below_root(cs.as_ref(), root.root_hash, &report.registry_peer_id)
            .expect("a 24-name registry must have a link whose subtree holds a name");

        let starved: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        for h in entity_tree::trie::collect_all_hashes(cs.as_ref(), root.root_hash) {
            if h == victim {
                continue;
            }
            if let Some(e) = cs.get(&h) {
                starved.put(e).expect("put");
            }
        }
        if let Some(e) = cs.get(&root.root_hash) {
            let _ = starved.put(e);
        }

        let short = recovered_names(starved.as_ref(), root.root_hash, &report.registry_peer_id);
        assert!(
            short.len() < full.len(),
            "withholding a node must actually hide names (hid {} of {})",
            full.len() - short.len(),
            full.len()
        );

        // THE FINDING: no error, no signal, no gap marker. Just fewer names.
        // `collect_all_bindings` returns a `BTreeMap`, not a `Result`, so there
        // is nowhere for the miss to be reported even if it were noticed.
        println!(
            "F4/D9 measured: withholding 1 interior node hid {} of {} names \
             with NO error from collect_all_bindings",
            full.len() - short.len(),
            full.len()
        );
    }

    /// The by-name keys under a registry's signed root, as names.
    fn recovered_names(store: &dyn ContentStore, root_hash: Hash, rid: &str) -> BTreeSet<String> {
        let prefix = rel(rid, &by_name_pointer_path(rid, ""));
        entity_tree::trie::collect_all_bindings(store, root_hash, "")
            .into_keys()
            .filter_map(|k| k.strip_prefix(&prefix).map(|n| n.to_string()))
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// A link directly under the root **whose subtree actually carries a name** —
    /// the interior node a withholding origin would drop.
    ///
    /// Selected by effect, not position: with hash-keyed routing the by-name keys
    /// scatter, so some links under the root hold only binding bodies and
    /// signatures. Withholding one of those hides nothing and would make this
    /// test assert on a no-op.
    fn hiding_link_below_root(
        store: &dyn ContentStore,
        root_hash: Hash,
        rid: &str,
    ) -> Option<Hash> {
        let node = entity_tree::trie::load_trie_node(store, root_hash)?;
        let before = recovered_names(store, root_hash, rid);
        node.data.iter().find_map(|e| match e {
            entity_tree::trie::Entry::Link(h) => {
                let sub = entity_tree::trie::collect_all_bindings(store, *h, "");
                let prefix = rel(rid, &by_name_pointer_path(rid, ""));
                let hides = sub.keys().any(|k| {
                    k.strip_prefix(&prefix).is_some_and(|n| !n.is_empty() && before.contains(n))
                });
                hides.then_some(*h)
            }
            _ => None,
        })
    }

    // ===================================================================
    // The CLI surface. Everything below asserts on what an OPERATOR sees —
    // the class of defect no test in this module could previously reach,
    // because every one of them constructs `BindingSpec` directly and never
    // parses a command line (audit F2 / AP25).
    // ===================================================================

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// **F2, the worst message in the audit's telemetry-gap map.** The printed
    /// help taught `--bind NAME=…` (a space); the parser matched only
    /// `--bind=`, so the flag was dropped and the refusal claimed *"at least
    /// one --bind is required"* — of someone who had just passed one.
    ///
    /// Mutation check: delete the bare-flag guard and this fails on the message
    /// assertion, because parsing then falls through to the empty-bindings
    /// refusal. An exit-code-only assertion would NOT have caught it — both
    /// paths fail — which is exactly why the parse returns its message as a
    /// value.
    #[test]
    fn a_space_separated_flag_is_refused_by_name_not_reported_as_absent() {
        let err = parse_registry_args(&argv(&[
            "registry",
            "out",
            "--bind",
            "a.example=2PEER@https://a.example",
        ]))
        .expect_err("a space-separated --bind must be refused");
        assert!(err.contains("--bind takes `=`"), "must name the actual mistake, got: {err}");
        assert!(
            !err.contains("at least one"),
            "must NOT claim the flag was absent — it was passed, just spelled wrong: {err}"
        );
    }

    /// The same guard's second job: with a space form, the *value* is the first
    /// non-flag argument, so without the guard it would have been adopted as the
    /// output directory — a wrong answer rather than a refusal.
    #[test]
    fn a_space_separated_value_is_never_adopted_as_the_output_directory() {
        let err = parse_registry_args(&argv(&["registry", "--bind", "a.example=2PEER@o"]))
            .expect_err("refused before out_dir is chosen");
        assert!(err.contains("output directory"), "must say what it would have become: {err}");
    }

    /// D10 in the *usage* text, not just in the emitter. The refusal has to name
    /// the `@ORIGIN` half, because a reader who omitted it has no other way to
    /// learn it is mandatory.
    #[test]
    fn the_no_bindings_refusal_states_that_origin_is_mandatory() {
        let err = parse_registry_args(&argv(&["registry", "out"])).expect_err("no bindings");
        assert!(err.contains("@ORIGIN"), "the required form must appear: {err}");
        assert!(err.contains("D10"), "and where the requirement comes from: {err}");
    }

    /// `--verify` reads a tree that already exists, so it must NOT demand
    /// bindings — otherwise the check is unreachable for the person most likely
    /// to want it: someone verifying a directory they were handed.
    #[test]
    fn verify_needs_no_bindings() {
        let parsed = parse_registry_args(&argv(&["registry", "out", "--verify"]))
            .expect("a verify with no bindings must parse");
        assert!(parsed.verify);
        assert!(parsed.bindings.is_empty());
        assert_eq!(parsed.out_dir, std::path::PathBuf::from("out"));
    }

    /// An origin may legitimately contain `@` (`http://user@host`); a Base58
    /// peer-id may not. So the separator is the FIRST `@`, and this pins it —
    /// `rsplit_once` here would silently truncate the origin.
    #[test]
    fn the_bind_separator_is_the_first_at_so_an_origin_may_contain_one() {
        let parsed = parse_registry_args(&argv(&[
            "registry",
            "out",
            "--bind=a.example=2PEERTARGET@http://user@host/path",
        ]))
        .expect("parses");
        assert_eq!(parsed.bindings[0].target_peer_id, "2PEERTARGET");
        assert_eq!(parsed.bindings[0].origin.as_deref(), Some("http://user@host/path"));
    }

    /// **F1 — the check that existed and nothing called.** `make federation`
    /// verified four domains and skipped the registry, which is the one tree the
    /// whole chain hangs from: a consumer pins this key and nothing else, so an
    /// unwalkable registry root means every name resolves to nothing while every
    /// domain below it verifies clean.
    ///
    /// It also pins **why the check lives on this verb**: `publish --verify`
    /// resolves a different durable identity, so it would look for
    /// `{out}/{publisher-peer}/` — a directory a registry emit never wrote.
    ///
    /// Mutation-checked in both directions: the clean tree must pass, and
    /// withholding one interior node — which leaves every pointer resolving and
    /// every body hashing correctly — must fail with
    /// [`super::super::publish::VERIFY_DEFECT_EXIT`].
    #[test]
    fn registry_verify_passes_a_clean_tree_and_fails_a_withheld_interior_node() {
        use std::process::ExitCode;
        let dir = tempfile::tempdir().unwrap();
        let seed_hex = "c1".repeat(32);
        let kp = entity_crypto::Keypair::from_seed([0xC1; 32]);
        let specs: Vec<BindingSpec> = (0..24)
            .map(|i| BindingSpec {
                name: format!("v{i:02}.example"),
                target_peer_id: target_pid(),
                origin: Some(TEST_ORIGIN.into()),
            })
            .collect();
        let report =
            emit_registry(dir.path(), kp, &specs, DEFAULT_TTL_MS, ISSUED_AT_MS).expect("emits");

        let verify = || {
            run(&argv(&[
                "registry",
                dir.path().to_str().unwrap(),
                &format!("--identity-seed={seed_hex}"),
                "--verify",
            ]))
        };
        assert_eq!(verify(), ExitCode::SUCCESS, "the tree we just emitted must verify");

        // Withhold one interior node, chosen by EFFECT (a link whose subtree
        // actually carries a name) — position is not a stable referent under
        // hash-keyed routing.
        let (cs, _li) = warm_from_dir(dir.path(), &report.registry_peer_id);
        let client = PublishedRootClient::new(
            DirFetcher::new(dir.path(), &report.registry_peer_id),
            entity_crypto::Keypair::from_seed([0xC1; 32]).public_key_bytes().to_vec(),
            entity_crypto::Keypair::from_seed([0xC1; 32]).key_type(),
            Some(report.registry_peer_id.clone()),
        );
        let root = client.fetch_root().expect("root verifies");
        let victim = hiding_link_below_root(cs.as_ref(), root.root_hash, &report.registry_peer_id)
            .expect("a 24-name registry has such a link");
        let removed = remove_blob(dir.path(), &victim);
        assert!(removed, "the victim node must exist on disk as a content blob");

        assert_eq!(
            verify(),
            ExitCode::from(crate::content_site::publish::VERIFY_DEFECT_EXIT),
            "an incomplete closure must FAIL — every pointer still resolves and every body \
             still hashes, so this is the only check that can see it"
        );
    }

    /// **F9 — a registry signs `name → peer-id`, so the target has to BE one.**
    ///
    /// Found by typing the wrong thing: a `--bind=example.test=foundation@…` that
    /// put a directory slug where the peer-id belonged emitted a fully signed
    /// binding, and the only symptom appeared at the far end of the chain, on a
    /// consumer's machine, as *"the named peer-id carries no public key"*. The
    /// operator who could fix it saw a success message.
    ///
    /// Refused at the emitter, like D10, and **before anything is written** —
    /// with every offender named, not the first.
    #[test]
    fn a_target_that_is_not_a_peer_id_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let err = emit_registry(
            dir.path(),
            entity_crypto::Keypair::from_seed([0xE9; 32]),
            &[
                BindingSpec {
                    name: "ok.example".into(),
                    target_peer_id: target_pid(),
                    origin: Some(TEST_ORIGIN.into()),
                },
                BindingSpec {
                    name: "slug.example".into(),
                    target_peer_id: "foundation".into(),
                    origin: Some(TEST_ORIGIN.into()),
                },
                BindingSpec {
                    name: "empty.example".into(),
                    target_peer_id: String::new(),
                    origin: Some(TEST_ORIGIN.into()),
                },
            ],
            DEFAULT_TTL_MS,
            ISSUED_AT_MS,
        )
        .expect_err("a non-peer-id target must be refused");

        assert!(err.contains("slug.example"), "must name the offender: {err}");
        assert!(err.contains("empty.example"), "must name EVERY offender, not the first: {err}");
        assert!(!err.contains("ok.example"), "must not blame the valid one: {err}");
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "a refusal that leaves a half-registry behind is worse than the emit it prevented"
        );
    }

    /// The other half of F9's line, and the reason it is `validate` rather than
    /// `from_peer_id`: a **legacy SHA-256-form** peer-id is a legal target that
    /// our own consumer cannot pin without an out-of-band key. Refusing it would
    /// be this emitter deciding what other consumers may resolve, which is not
    /// its call. Well-formed is the bar; pinnable-by-us is not.
    #[test]
    fn a_legacy_form_target_is_still_issuable_because_it_is_still_a_peer_id() {
        let legacy = entity_crypto::Keypair::from_seed([0x7B; 32])
            .peer_id_with_hash_type(entity_crypto::HASH_TYPE_SHA256);
        let Ok(legacy) = legacy else {
            // Upstream refuses to CONSTRUCT this form for Ed25519 now
            // ("SHA-256-form is legacy-decode-only", Amendment 3). If it cannot
            // be built, it cannot be issued, and there is nothing to assert —
            // recorded rather than silently skipped.
            println!("legacy SHA-256-form is no longer constructible upstream — nothing to test");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let out = emit_registry(
            dir.path(),
            entity_crypto::Keypair::from_seed([0xEA; 32]),
            &[BindingSpec {
                name: "legacy.example".into(),
                target_peer_id: legacy.as_str().to_string(),
                origin: Some(TEST_ORIGIN.into()),
            }],
            DEFAULT_TTL_MS,
            ISSUED_AT_MS,
        );
        assert!(out.is_ok(), "a well-formed legacy-form target must still issue: {out:?}");
    }

    /// Delete the content blob for `hash`, wherever the sharded layout put it.
    /// Located by file NAME rather than by recomputing the shard path, so this
    /// keeps working if the layout changes.
    fn remove_blob(dir: &Path, hash: &Hash) -> bool {
        let want = hash.to_hex();
        fn walk(at: &Path, want: &str) -> bool {
            let Ok(rd) = fs::read_dir(at) else { return false };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if walk(&p, want) {
                        return true;
                    }
                } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == want) {
                    return fs::remove_file(&p).is_ok();
                }
            }
            false
        }
        walk(&dir.join("content"), &want)
    }
}
