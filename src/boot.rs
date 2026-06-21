//! Boot classification — the explicit cold / warm / ephemeral signal.
//!
//! Before this module, "warmth" was implicit and inferred in five places
//! (does `entity_peers` localStorage hold a Frontend keypair? does
//! `workers/{pid}/` OPFS exist? which arm are we on?). The clobber bug was
//! precisely *treating a warm-durable boot as cold* and re-seeding defaults
//! over persisted state. We now compute the boot class **once**, at the top
//! of [`EntityApp::new_wasm`](crate::app::EntityApp::new_wasm) /
//! [`new_wasm_worker`](crate::app::EntityApp::new_wasm_worker), and carry it
//! into the owned boot-load step.
//!
//! Ratified internal-only for now (decision #5): it gates seeding
//! observability today and will later drive the durability banner + a
//! first-run experience. See §2.2 for the class table.

/// Which class of startup this is. Computed once from "did a persisted
/// keypair exist before this boot?" and "is the tree durable this session?"
/// (the Worker arm with its replayed OPFS journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootClass {
    /// First-ever launch on this profile: no persisted keypair existed, so
    /// one was just generated. The tree starts empty; seeding defaults is
    /// unconditionally correct (there is nothing to preserve).
    Cold,
    /// Returning launch with a durable tree: keypair restored **and** the
    /// Worker arm replayed an OPFS journal. Persisted state is
    /// authoritative — defaults must be seeded *only if absent*.
    WarmDurable,
    /// Returning identity, ephemeral tree: keypair restored but the arm is
    /// Direct/Tauri, so the tree is in-memory by substrate constraint and
    /// re-defaults every boot. Durable from the tree's point of view it is
    /// not, but identity persists.
    WarmIdColdTree,
    /// Intentionally ephemeral: a multi-tab secondary or a Worker→Direct
    /// downgrade. Identity may be restored; the tree is empty on purpose.
    /// (Set by the caller that *knows* it downgraded; `classify` never
    /// returns this on its own.)
    ///
    /// Not yet constructed: `main.rs` already detects these postures via
    /// `BootStorageStatus` (multi-tab secondary / downgrade-to-Direct), but
    /// threading that into the boot class is deferred to the §2.3 / §2.4
    /// hardening step (session-restore + banner wiring, decision #5). The
    /// variant exists now so the vocabulary is complete and that wiring is a
    /// pure addition, not a re-shape.
    #[allow(dead_code)]
    Ephemeral,
}

impl BootClass {
    /// Classify a boot from whether a persisted keypair (a Frontend entry)
    /// existed *before* this boot generated one, and whether the tree is
    /// durable this session (`true` only on the Worker arm).
    ///
    /// Never returns [`BootClass::Ephemeral`] — that is a caller-known
    /// downgrade/multi-tab posture, applied on top.
    pub fn classify(had_persisted_keypair: bool, durable_tree: bool) -> Self {
        match (had_persisted_keypair, durable_tree) {
            (false, _) => BootClass::Cold,
            (true, true) => BootClass::WarmDurable,
            (true, false) => BootClass::WarmIdColdTree,
        }
    }

    /// First-ever launch (nothing persisted to preserve).
    pub fn is_cold(&self) -> bool {
        matches!(self, BootClass::Cold)
    }

    /// The tree is durable-authoritative this session — a returning boot
    /// whose persisted state must never be clobbered.
    pub fn tree_is_durable(&self) -> bool {
        matches!(self, BootClass::WarmDurable)
    }

    /// Stable label for structured logging.
    pub fn label(&self) -> &'static str {
        match self {
            BootClass::Cold => "cold",
            BootClass::WarmDurable => "warm-durable",
            BootClass::WarmIdColdTree => "warm-id/cold-tree",
            BootClass::Ephemeral => "ephemeral",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_the_matrix() {
        assert_eq!(BootClass::classify(false, false), BootClass::Cold);
        assert_eq!(BootClass::classify(false, true), BootClass::Cold);
        assert_eq!(BootClass::classify(true, true), BootClass::WarmDurable);
        assert_eq!(BootClass::classify(true, false), BootClass::WarmIdColdTree);
    }

    #[test]
    fn durability_predicates() {
        assert!(BootClass::Cold.is_cold());
        assert!(!BootClass::WarmDurable.is_cold());
        assert!(BootClass::WarmDurable.tree_is_durable());
        assert!(!BootClass::WarmIdColdTree.tree_is_durable());
        assert!(!BootClass::Cold.tree_is_durable());
    }
}
