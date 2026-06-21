//! Embedded-app (games) convention — host side.
//!
//! A sibling to `content_site`: self-contained HTML apps (today, games from the
//! `entity-apps` repo) ingested into the tree under `/{peer}/apps/{set}/…` and
//! run in a **sandboxed iframe** by the Games window.
//!
//! - [`format`] — the catalog / bundle / save-state entity formats.
//! - [`paths`] — the `/{peer}/apps/{set}/…` tree layout.

// The catalog/bundle formats and the `/{peer}/apps/{set}/…` path helpers are the
// foundation for the ingester + catalog grid (next phase); some are not yet
// referenced by the de-risk slice (which loads a single bundled fixture).
#![allow(dead_code)]

pub mod format;
pub mod ingest;
pub mod paths;
pub mod read;
pub mod save_retention;
