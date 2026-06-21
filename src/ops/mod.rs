//! App-tier operation primitives — Stage D factoring.
//!
//! Renderer-neutral request/response shapes around the `Peers` L1
//! dispatch surface. Consumers (Execute Console, the shell `exec`
//! verb, future call sites) build a typed request, await the op, and
//! turn the response into whatever output their renderer needs.
//!
//! See the shell arc handoff for the
//! motivation and the relationship to the shell arc.

pub mod execute;

pub use execute::{execute, ExecuteRequest};
#[allow(unused_imports)] // consumed by the shell `exec` verb (Phase 4)
pub use execute::ExecuteResponse;
