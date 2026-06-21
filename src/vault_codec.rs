//! Pure (de)serialization of the localStorage peer **vault** (`entity_peers`)
//! and the shared 32-byte-seed hex codec.
//!
//! Extracted from the wasm-only `persistence::wasm` module (which keeps the
//! browser localStorage I/O) so the on-disk **format — and now its schema
//! version — is covered by native unit tests**. See the system-peer
//! init + capability-posture map §10 (CR-1).
//!
//! ## Format
//!
//! The vault blob is line-oriented. An **optional leading `v{N}` marker** line
//! records the schema version; each remaining line is one entry:
//! `peer_id|seed_hex|label|mode`. The entry layout is **byte-identical** to the
//! pre-version format — adding the marker is backward-compatible because a
//! marker line has no `|`, and the historical parser skipped any line with
//! fewer than two `|`-fields (CR-2). Header-less (pre-marker) data parses as
//! [`LEGACY_VERSION`].
//!
//! ## Migration discipline (MAP §8 invariant)
//!
//! Bump [`VAULT_VERSION`] **only** alongside a parser branch that migrates the
//! prior layout. Changing this format without a version bump + migration is a
//! data-loss change, not an edit.

use crate::peer_mode::PeerMode;

/// Current vault schema version, written as the leading `v{N}` marker.
pub const VAULT_VERSION: u32 = 1;

/// Version reported for pre-marker (header-less) vault data.
pub const LEGACY_VERSION: u32 = 0;

/// One decoded vault row: a peer's identity seed + display/mode metadata.
/// Only the `seed` is secret; `peer_id`/`label`/`mode` are safe to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultEntry {
    pub peer_id: String,
    pub seed: [u8; 32],
    pub label: Option<String>,
    pub mode: PeerMode,
}

/// 32-byte seed → lowercase hex (64 chars). Also used for the `entity_system_seed`
/// key, hence its home here rather than buried in the vault parser.
pub fn seed_to_hex(seed: &[u8; 32]) -> String {
    seed.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Lowercase hex (exactly 64 chars) → 32-byte seed. `None` on wrong length or
/// non-hex — callers treat that as "absent/malformed", never a panic.
pub fn hex_to_seed(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut seed = [0u8; 32];
    for i in 0..32 {
        seed[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(seed)
}

/// A leading `v{N}` marker line → `Some(N)`; anything else → `None`. A marker
/// has no `|` (entries always do), so it can never be mistaken for an entry.
fn parse_version_marker(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix('v')?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u32>().ok()
}

/// Parse a vault blob into `(schema_version, entries)`. Tolerant by design — a
/// boot must never drop a peer over a future-mode key or a stray line:
/// - optional leading `v{N}` marker sets the version; absent → [`LEGACY_VERSION`];
/// - each entry line is `peer_id|seed_hex|label|mode`; `<2` fields → skipped;
/// - missing/unknown mode → [`PeerMode::Frontend`]; bad seed hex → skipped.
pub fn parse(data: &str) -> (u32, Vec<VaultEntry>) {
    let mut lines = data.lines().peekable();
    let mut version = LEGACY_VERSION;
    if let Some(first) = lines.peek() {
        if let Some(v) = parse_version_marker(first) {
            version = v;
            lines.next(); // consume the marker
        }
    }

    let mut entries = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() < 2 {
            continue;
        }
        let peer_id = parts[0].to_string();
        let seed = match hex_to_seed(parts[1]) {
            Some(s) => s,
            None => continue,
        };
        let label = parts
            .get(2)
            .and_then(|s| if s.is_empty() { None } else { Some(s.to_string()) });
        // Missing or unrecognized mode → Frontend (default-on-unknown): never
        // drop a peer because of a future-mode key we don't understand yet.
        let mode = parts
            .get(3)
            .and_then(|s| PeerMode::from_persist_key(s))
            .unwrap_or(PeerMode::Frontend);
        entries.push(VaultEntry { peer_id, seed, label, mode });
    }
    (version, entries)
}

/// Serialize entries with the current [`VAULT_VERSION`] marker. The marker is
/// always written (even for an empty vault) so the blob is self-describing.
pub fn serialize(entries: &[VaultEntry]) -> String {
    let mut out = format!("v{VAULT_VERSION}");
    for e in entries {
        out.push('\n');
        out.push_str(&format!(
            "{}|{}|{}|{}",
            e.peer_id,
            seed_to_hex(&e.seed),
            e.label.as_deref().unwrap_or(""),
            e.mode.persist_key(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, byte: u8, label: Option<&str>, mode: PeerMode) -> VaultEntry {
        VaultEntry {
            peer_id: id.to_string(),
            seed: [byte; 32],
            label: label.map(str::to_string),
            mode,
        }
    }

    #[test]
    fn round_trip_preserves_entries_and_version() {
        let entries = vec![
            entry("2KAAA", 0x11, Some("alpha"), PeerMode::Frontend),
            entry("2KBBB", 0x22, None, PeerMode::BackendOpfs),
            entry("2KCCC", 0x33, Some("gamma"), PeerMode::BackendMemory),
        ];
        let blob = serialize(&entries);
        assert!(blob.starts_with("v1\n"), "marker prepended: {blob:?}");
        let (version, back) = parse(&blob);
        assert_eq!(version, VAULT_VERSION);
        assert_eq!(back, entries);
    }

    #[test]
    fn legacy_header_less_data_parses_identically() {
        // Exactly the pre-version on-disk shape.
        let legacy = format!(
            "2KAAA|{}||frontend\n2KBBB|{}|beta|backend-opfs",
            seed_to_hex(&[0x11; 32]),
            seed_to_hex(&[0x22; 32]),
        );
        let (version, entries) = parse(&legacy);
        assert_eq!(version, LEGACY_VERSION, "no marker → legacy version");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].mode, PeerMode::Frontend);
        assert_eq!(entries[1].label.as_deref(), Some("beta"));
        assert_eq!(entries[1].mode, PeerMode::BackendOpfs);
    }

    #[test]
    fn forward_marker_is_recognized() {
        // A future version marker is read back as-is; entry layout still parses
        // (a real format change would add the migration branch here).
        let blob = format!("v7\n2KAAA|{}||frontend", seed_to_hex(&[0x11; 32]));
        let (version, entries) = parse(&blob);
        assert_eq!(version, 7);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn tolerant_to_garbage_and_unknown_mode() {
        let blob = format!(
            "v1\n\
             garbage-no-pipe\n\
             2KAAA|{seed}|lbl|future-mode-we-dont-know\n\
             2KBAD|not-hex-seed|x|frontend\n\
             2KOK|{seed}||backend-memory",
            seed = seed_to_hex(&[0x44; 32]),
        );
        let (_v, entries) = parse(&blob);
        // garbage line skipped (<2 fields); bad-seed line skipped; 2 survive.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].peer_id, "2KAAA");
        assert_eq!(entries[0].mode, PeerMode::Frontend, "unknown mode → Frontend");
        assert_eq!(entries[1].mode, PeerMode::BackendMemory);
    }

    #[test]
    fn empty_vault_round_trips() {
        let blob = serialize(&[]);
        assert_eq!(blob, "v1");
        let (version, entries) = parse(&blob);
        assert_eq!(version, VAULT_VERSION);
        assert!(entries.is_empty());
        // Truly empty string → legacy, no entries (no panic).
        assert_eq!(parse(""), (LEGACY_VERSION, vec![]));
    }

    #[test]
    fn hex_round_trip_and_rejects_bad() {
        let seed = [0x9a; 32];
        assert_eq!(hex_to_seed(&seed_to_hex(&seed)), Some(seed));
        assert_eq!(hex_to_seed("tooshort"), None);
        assert_eq!(hex_to_seed(&"zz".repeat(32)), None);
    }
}
