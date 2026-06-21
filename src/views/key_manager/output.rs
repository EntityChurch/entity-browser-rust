//! Renderer-neutral output for the Key Manager window.
//!
//! One row per hosted peer's public identity, derived from the
//! tree-backed peer registry. No private key material — `peer_id` is
//! the Ed25519-derived public identity.

#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct KeyManagerOutput {
    pub keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone)]
pub struct KeyEntry {
    pub label: String,
    pub peer_id: String,
    pub role: String,
}
