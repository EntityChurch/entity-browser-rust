//! Renderer-side privacy policy for entity types.
//!
//! Each entity type has a privacy classification per
//! `GUIDE-INSPECTABILITY.md` §9 #4 + the privacy & cross-peer
//! observability audit §2.
//! Renderers consult `RenderPolicy::for_entity_type(t)` at decode time
//! to decide what to draw:
//!
//! - `Public` — render freely (operational state, designed for sharing).
//! - `CapControlled` — render in operator-mode surfaces; redact in
//!   non-operator-mode L3 surfaces.
//! - `Sensitive` — never render to L3 surfaces. Show only
//!   `{ type, hash }` with the body suppressed.
//!
//! **Conservative default.** Per the inspectability-baseline security
//! audit §5.2:
//! entity types not in the table are treated as `Sensitive` until
//! their extension publishes a §9 #4 declaration. Over-redact rather
//! than over-expose.
//!
//! This is the L3 consumer's half of the spec contract. The substrate
//! does not enforce — it is the renderer's responsibility to honor
//! classifications when surfacing inspect data.
//!
//! Today's Entity Tree window is "operator mode" by default and
//! renders entity bodies freely; this module is the policy layer the
//! incoming Inspect window family consumes. See the inspectability
//! feedback correspondence §5 for the design rationale.

#![allow(dead_code)]

/// Privacy classification for one entity type, used by inspect-aware
/// renderers to decide whether to surface body content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPolicy {
    /// Operational state, designed for sharing. Render freely.
    Public,
    /// Render to operator-class surfaces only; show
    /// `{ type, hash, "[capability-controlled]" }` to non-operator L3.
    CapControlled,
    /// Never render body to L3 surfaces. Show only `{ type, hash }`.
    Sensitive,
}

impl RenderPolicy {
    /// Classification for one entity type. The match arms encode the
    /// audit §2 declarations; unrecognized types fall through to
    /// `Sensitive` (conservative default).
    pub fn for_entity_type(t: &str) -> Self {
        // CAPABILITY — audit §2.1. All token/grant/signature material
        // is signature-bearer-equivalent; sensitive everywhere.
        if t == "system/capability/token"
            || t == "system/capability/grants/role-derived"
            || t.starts_with("system/capability/grants/")
        {
            return Self::Sensitive;
        }
        if t == "system/capability/request"
            || t == "system/capability/grant"
            || t == "system/capability/revocation"
        {
            return Self::CapControlled;
        }

        // SUBSCRIPTION — audit §2.2.
        if t == "system/subscription"
            || t == "system/protocol/inbox/notification"
            || t == "system/subscription/request"
            || t == "system/subscription/redirect"
        {
            return Self::CapControlled;
        }
        if t == "system/subscription/cancel"
            || t == "system/subscription/limits"
            || t == "system/config/subscription"
        {
            return Self::Public;
        }

        // REVISION — audit §2.3. Version DAG content is public-by-
        // convention; per-prefix operational state is cap-controlled.
        if t == "system/revision/entry" || t == "system/revision/snapshot" {
            return Self::Public;
        }
        if t == "system/revision/conflict" {
            return Self::CapControlled;
        }

        // INBOX — audit §2.4.
        if t == "system/protocol/inbox/delivery" {
            return Self::CapControlled;
        }

        // CONTINUATION — audit §2.5.
        if t == "system/continuation/install-result" {
            return Self::Public;
        }
        if t == "system/continuation"
            || t == "system/continuation/join"
            || t == "system/continuation/suspended"
            || t == "system/continuation/advance-request"
            || t == "system/continuation/resume-request"
            || t == "system/continuation/abandon-request"
            || t == "system/runtime/chain-error-lost"
            || t == "system/runtime/chain-error-rejected"
        {
            return Self::CapControlled;
        }

        // App-tier entity-browser types — these are app-domain state
        // (event-log entries, window state). Not part of the audit;
        // public for our own UX.
        if t.starts_with("app/entity-browser/")
            || t.starts_with("app/state/")
        {
            return Self::Public;
        }

        // Conservative default per audit §5.2.
        Self::Sensitive
    }

    /// True if the policy permits rendering the entity body to a
    /// non-operator-mode surface.
    pub fn permits_body_in_normal_mode(self) -> bool {
        matches!(self, Self::Public)
    }

    /// True if an operator-mode surface may render the body.
    /// Sensitive types remain redacted even for operators by default;
    /// an explicit "show key material" toggle (with audit logging)
    /// would be needed to override — out of scope here.
    pub fn permits_body_in_operator_mode(self) -> bool {
        matches!(self, Self::Public | Self::CapControlled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_signature_is_sensitive() {
        assert_eq!(
            RenderPolicy::for_entity_type("system/capability/token"),
            RenderPolicy::Sensitive,
        );
        assert_eq!(
            RenderPolicy::for_entity_type("system/capability/grants/foo"),
            RenderPolicy::Sensitive,
        );
    }

    #[test]
    fn continuation_install_result_is_public() {
        assert_eq!(
            RenderPolicy::for_entity_type("system/continuation/install-result"),
            RenderPolicy::Public,
        );
    }

    #[test]
    fn continuation_chain_error_is_cap_controlled() {
        assert_eq!(
            RenderPolicy::for_entity_type("system/runtime/chain-error-lost"),
            RenderPolicy::CapControlled,
        );
        assert_eq!(
            RenderPolicy::for_entity_type("system/continuation"),
            RenderPolicy::CapControlled,
        );
    }

    #[test]
    fn revision_entry_is_public() {
        assert_eq!(
            RenderPolicy::for_entity_type("system/revision/entry"),
            RenderPolicy::Public,
        );
    }

    #[test]
    fn subscription_entity_is_cap_controlled() {
        assert_eq!(
            RenderPolicy::for_entity_type("system/subscription"),
            RenderPolicy::CapControlled,
        );
        assert_eq!(
            RenderPolicy::for_entity_type("system/subscription/cancel"),
            RenderPolicy::Public,
        );
    }

    #[test]
    fn app_browser_types_are_public() {
        assert_eq!(
            RenderPolicy::for_entity_type("app/entity-browser/event"),
            RenderPolicy::Public,
        );
        assert_eq!(
            RenderPolicy::for_entity_type("app/state/query_console"),
            RenderPolicy::Public,
        );
    }

    #[test]
    fn unknown_type_falls_through_to_sensitive() {
        assert_eq!(
            RenderPolicy::for_entity_type("ext/some/new/type"),
            RenderPolicy::Sensitive,
        );
        assert_eq!(
            RenderPolicy::for_entity_type(""),
            RenderPolicy::Sensitive,
        );
    }

    #[test]
    fn body_permission_in_normal_mode() {
        assert!(RenderPolicy::Public.permits_body_in_normal_mode());
        assert!(!RenderPolicy::CapControlled.permits_body_in_normal_mode());
        assert!(!RenderPolicy::Sensitive.permits_body_in_normal_mode());
    }

    #[test]
    fn body_permission_in_operator_mode() {
        assert!(RenderPolicy::Public.permits_body_in_operator_mode());
        assert!(RenderPolicy::CapControlled.permits_body_in_operator_mode());
        assert!(!RenderPolicy::Sensitive.permits_body_in_operator_mode());
    }
}
