//! Program fixture bundles — parse + hash-verified materialization input.
//!
//! A bundle is the JSON emitted by `tools/program-dump` (run via
//! `make program-fixtures`): every entity workbench-go authored under a
//! program root (descriptor, seeds, step/projection IR DAG), plus a
//! per-tick oracle (state/port content hashes under a fixed input
//! schedule) that the native integration test replays against the Rust
//! evaluator — the cross-impl trust gate.
//!
//! Verification is load-bearing: [`Bundle::verified_entities`] re-computes
//! each entity's content hash from `(type, data)` under the Rust canonical
//! encoder and refuses the bundle on any digest mismatch. A mismatch means
//! the two implementations disagree on canonical encoding — exactly the
//! class of bug (field-name/int-sign encoding drift) that must be loud,
//! never "imported fine, evaluates wrong".

use entity_entity::Entity;

/// One embedded program fixture. `key` doubles as the roster identity and
/// the e2e hook; `json` is the raw bundle file. `name`/`glyph`/`description`
/// are the launcher-grid presentation (the descriptor carries no program-level
/// display name), so the Programs window has a single source of truth.
pub struct EmbeddedProgram {
    pub key: &'static str,
    pub json: &'static str,
    /// Display name for the launcher tile.
    pub name: &'static str,
    /// Launcher-card emoji (empty = letter fallback).
    pub glyph: &'static str,
    /// One-line tile subtitle.
    pub description: &'static str,
}

/// The three POC programs, embedded at build time. Regenerate with
/// `make program-fixtures` (requires the sibling Go checkouts).
pub const EMBEDDED_PROGRAMS: &[EmbeddedProgram] = &[
    EmbeddedProgram {
        key: "life",
        json: include_str!("../../assets/programs/life.json"),
        name: "Life",
        glyph: "🧬",
        description: "Conway's Game of Life — a cellular-automaton compute program.",
    },
    EmbeddedProgram {
        key: "snake",
        json: include_str!("../../assets/programs/snake.json"),
        name: "Snake",
        glyph: "🐍",
        description: "Snake — arrow keys steer; a direction-input compute program.",
    },
    EmbeddedProgram {
        key: "asteroids",
        json: include_str!("../../assets/programs/asteroids.json"),
        name: "Asteroids",
        glyph: "🚀",
        description: "Asteroids — arrows steer, Space fires; a vector-display compute program.",
    },
];

/// One authored entity: program-relative path + wire form.
#[derive(Debug, Clone)]
pub struct BundleEntity {
    pub path: String,
    pub entity_type: String,
    /// Raw canonical-CBOR entity data.
    pub data: Vec<u8>,
    /// Authoring-side content hash, `<tag>:<digest-hex>` (Go prints
    /// `ecf-sha256:…`; only the digest hex is compared).
    pub hash: String,
}

/// An oracle input write applied before the given tick runs.
// The oracle-replay mirror of the bundle JSON: read by the cross-impl oracle
// tests, not on the live render/tick path (real input comes from the drivers).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub tick: u64,
    pub port: String,
    pub entity_type: String,
    pub data: Vec<u8>,
}

/// Boundary hashes after tick `i` completes.
// Consumed by the oracle tests (the cross-impl trust gate), not at runtime.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TickOracle {
    pub state_hash: String,
    /// Port name → content hash for `source`-bearing output ports.
    pub port_hashes: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Bundle {
    // program/root/inputs/oracle are the bundle-JSON mirror the oracle tests
    // read; the live host runs off origin_peer/descriptor_path/entities.
    #[allow(dead_code)]
    pub program: String,
    #[allow(dead_code)]
    pub root: String,
    /// The authoring peer's id — the namespace baked into the IR's
    /// `lookup/tree` paths by the Go builder (it peer-qualifies at build
    /// time). The host materializes and runs the program under THIS
    /// namespace in the local store — the foreign-natural-path caching
    /// pattern. ("Program-relative paths port unchanged" holds for the
    /// descriptor's declared paths but NOT the IR internals; finding
    /// routed upstream.)
    pub origin_peer: String,
    pub descriptor_path: String,
    pub entities: Vec<BundleEntity>,
    #[allow(dead_code)]
    pub inputs: Vec<InputEvent>,
    #[allow(dead_code)]
    pub oracle: Vec<TickOracle>,
}

impl Bundle {
    /// Strict parse of a `tools/program-dump` bundle JSON.
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("bundle json: {e}"))?;
        let obj = v.as_object().ok_or("bundle: not an object")?;
        let text = |name: &str| -> Result<String, String> {
            obj.get(name)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("bundle: missing field {name}"))
        };

        let mut entities = Vec::new();
        for (i, e) in obj
            .get("entities")
            .and_then(|x| x.as_array())
            .ok_or("bundle: missing entities")?
            .iter()
            .enumerate()
        {
            let eo = e.as_object().ok_or(format!("entity[{i}]: not an object"))?;
            let field = |name: &str| -> Result<String, String> {
                eo.get(name)
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| format!("entity[{i}]: missing {name}"))
            };
            entities.push(BundleEntity {
                path: field("path")?,
                entity_type: field("type")?,
                data: hex_decode(&field("data_hex")?)
                    .map_err(|e| format!("entity[{i}]: {e}"))?,
                hash: field("hash")?,
            });
        }

        let mut inputs = Vec::new();
        if let Some(arr) = obj.get("inputs").and_then(|x| x.as_array()) {
            for (i, e) in arr.iter().enumerate() {
                let eo = e.as_object().ok_or(format!("input[{i}]: not an object"))?;
                inputs.push(InputEvent {
                    tick: eo
                        .get("tick")
                        .and_then(|x| x.as_u64())
                        .ok_or(format!("input[{i}]: missing tick"))?,
                    port: eo
                        .get("port")
                        .and_then(|x| x.as_str())
                        .ok_or(format!("input[{i}]: missing port"))?
                        .to_string(),
                    entity_type: eo
                        .get("type")
                        .and_then(|x| x.as_str())
                        .ok_or(format!("input[{i}]: missing type"))?
                        .to_string(),
                    data: hex_decode(
                        eo.get("data_hex")
                            .and_then(|x| x.as_str())
                            .ok_or(format!("input[{i}]: missing data_hex"))?,
                    )
                    .map_err(|e| format!("input[{i}]: {e}"))?,
                });
            }
        }

        let mut oracle = Vec::new();
        for (i, e) in obj
            .get("oracle")
            .and_then(|x| x.as_array())
            .ok_or("bundle: missing oracle")?
            .iter()
            .enumerate()
        {
            let eo = e.as_object().ok_or(format!("oracle[{i}]: not an object"))?;
            let mut port_hashes = Vec::new();
            if let Some(ph) = eo.get("port_hashes").and_then(|x| x.as_object()) {
                for (k, v) in ph {
                    port_hashes.push((
                        k.clone(),
                        v.as_str()
                            .ok_or(format!("oracle[{i}]: port hash not a string"))?
                            .to_string(),
                    ));
                }
            }
            oracle.push(TickOracle {
                state_hash: eo
                    .get("state_hash")
                    .and_then(|x| x.as_str())
                    .ok_or(format!("oracle[{i}]: missing state_hash"))?
                    .to_string(),
                port_hashes,
            });
        }

        Ok(Self {
            program: text("program")?,
            root: text("root")?,
            origin_peer: text("origin_peer")?,
            descriptor_path: text("descriptor_path")?,
            entities,
            inputs,
            oracle,
        })
    }

    /// Construct every entity and verify its Rust-computed content hash
    /// against the authoring-side hash. Returns `(program-relative path,
    /// entity)` pairs ready to materialize, or the first mismatch —
    /// loudly, with both digests.
    pub fn verified_entities(&self) -> Result<Vec<(String, Entity)>, String> {
        let mut out = Vec::with_capacity(self.entities.len());
        for be in &self.entities {
            let entity = Entity::new(&be.entity_type, be.data.clone())
                .map_err(|e| format!("{}: entity construction: {e}", be.path))?;
            let ours = digest_hex(&entity);
            let theirs = digest_of_hash_string(&be.hash)?;
            if ours != theirs {
                return Err(format!(
                    "{}: cross-impl content-hash mismatch — authored {} vs recomputed {} \
                     (canonical-encoding divergence; refusing the bundle)",
                    be.path, theirs, ours
                ));
            }
            out.push((be.path.clone(), entity));
        }
        Ok(out)
    }
}

/// Digest hex of an entity's content hash (tag-independent compare:
/// Go prints `ecf-sha256:…`, Rust `ecfv1-sha256:…` — same digest).
pub fn digest_hex(entity: &Entity) -> String {
    entity
        .content_hash
        .digest()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The digest-hex half of a `<tag>:<hex>` hash string.
pub fn digest_of_hash_string(s: &str) -> Result<String, String> {
    s.split_once(':')
        .map(|(_, hex)| hex.to_ascii_lowercase())
        .ok_or_else(|| format!("hash string without tag: {s}"))
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| format!("bad hex: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_embedded_bundles() {
        for p in EMBEDDED_PROGRAMS {
            let b = Bundle::parse(p.json).expect(p.key);
            assert_eq!(b.program, p.key);
            assert!(!b.entities.is_empty(), "{}: no entities", p.key);
            assert!(!b.oracle.is_empty(), "{}: no oracle", p.key);
            assert!(
                b.entities.iter().any(|e| e.path == b.descriptor_path),
                "{}: descriptor entity missing from bundle",
                p.key
            );
        }
    }

    /// The cross-impl canonical-encoding gate: every authored entity's
    /// content hash must recompute identically under the Rust encoder.
    #[test]
    fn all_embedded_entities_hash_verify() {
        for p in EMBEDDED_PROGRAMS {
            let b = Bundle::parse(p.json).expect(p.key);
            let ents = b.verified_entities().expect(p.key);
            assert_eq!(ents.len(), b.entities.len());
        }
    }

    /// Corruption must be refused loudly, per entity.
    #[test]
    fn corrupted_entity_is_refused() {
        let b = Bundle::parse(EMBEDDED_PROGRAMS[0].json).unwrap();
        let mut bad = b.clone();
        bad.entities[0].data[0] ^= 0x01;
        let err = bad.verified_entities().unwrap_err();
        assert!(err.contains("cross-impl content-hash mismatch"), "{err}");
    }

    #[test]
    fn hex_decode_rejects_garbage() {
        assert!(hex_decode("0").is_err());
        assert!(hex_decode("zz").is_err());
        assert_eq!(hex_decode("00ff").unwrap(), vec![0x00, 0xff]);
    }
}
