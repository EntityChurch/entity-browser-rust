//! In-memory outcome of the **last Pull pressed** in File Transfer — the
//! counterpart of [`crate::offer_attempt`] for the other direction (B-13).
//!
//! Why this exists: Pull reported only to the *Results* pane at the bottom of
//! the window, and its button stayed enabled while a pull ran. So on a phone —
//! where that pane is off screen — a multi-megabyte closure walk looked like a
//! button that did nothing, and pressing it again started a second walk of the
//! same file whose outcome nobody could tell apart from the first. The offer
//! card had the same defect and was fixed with `OfferAttempt`; this is that
//! shape, on the Pull card.
//!
//! **Keyed by the device the pull is from.** File Transfer drops late answers
//! from a device you have switched away from; a pull's progress line for device
//! A rendered under device B's file list would be the same confusion one layer
//! up. The window renders the slot only when its `target` is the device on
//! screen, and disables Pull while *any* pull is in flight — there is one slot,
//! so a second press would overwrite the only report of the first.
//!
//! In memory, not in the tree, for [`crate::offer_attempt`]'s reason: it is one
//! press's progress, meaningless across a reload, and a durable copy would
//! strand a `Pulling` row — and a disabled button — forever when a reload lands
//! mid-walk.

use std::sync::{Arc, Mutex};

/// What became of the last Pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    /// Running. `total` is `0` until the blob has named its chunks (and stays
    /// `0` for a share `read`, which arrives in one response).
    Pulling { held: usize, total: usize },
    /// Saved to this device or kept in My files. Renders nothing: the download
    /// (or the My files row) is the feedback, and a line saying so would
    /// outlive it.
    Done,
    /// It did not happen, and this is why — verbatim, because a bare "failed"
    /// leaves the user as stuck as the silence did.
    Failed(String),
}

impl PullOutcome {
    pub fn in_flight(&self) -> bool {
        matches!(self, PullOutcome::Pulling { .. })
    }
}

/// One slot's contents: which device, which file, what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRecord {
    pub target: String,
    pub filename: String,
    pub outcome: PullOutcome,
}

/// Cheap-to-clone handle to the app's single last-pull slot, shared with the
/// render context ([`crate::dom::util::DomCtx`]).
#[derive(Clone, Default)]
pub struct PullAttempt {
    inner: Arc<Mutex<Option<PullRecord>>>,
}

#[allow(dead_code)] // the writers are WASM-only (the app's pull path)
impl PullAttempt {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pull of `filename` from `target` has started.
    pub fn start(&self, target: &str, filename: &str) {
        self.set(target, filename, PullOutcome::Pulling { held: 0, total: 0 });
    }

    /// Progress on the pull in the slot. Ignored unless that pull is still
    /// running: a late progress callback must not resurrect a finished press.
    pub fn progress(&self, target: &str, filename: &str, held: usize, total: usize) {
        self.update(target, filename, |o| {
            if o.in_flight() {
                *o = PullOutcome::Pulling { held, total };
            }
        });
    }

    pub fn done(&self, target: &str, filename: &str) {
        self.update(target, filename, |o| *o = PullOutcome::Done);
    }

    pub fn failed(&self, target: &str, filename: &str, why: &str) {
        self.update(target, filename, |o| *o = PullOutcome::Failed(why.to_string()));
    }

    /// Is any pull running? Drives the disabled state of Pull and Keep.
    pub fn in_flight(&self) -> bool {
        self.read().is_some_and(|r| r.outcome.in_flight())
    }

    /// The last pull, `None` when nothing has been pulled this session. A
    /// poisoned lock reads as `None` rather than panicking in the render path.
    pub fn read(&self) -> Option<PullRecord> {
        self.inner.lock().ok().and_then(|slot| slot.clone())
    }

    fn set(&self, target: &str, filename: &str, outcome: PullOutcome) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = Some(PullRecord { target: target.to_string(), filename: filename.to_string(), outcome });
        }
    }

    /// Change the outcome only if the slot still describes this pull. Without
    /// the check, the end of an older pull (a second press is refused while one
    /// runs, but the Shell verb and a reload race are not) would overwrite the
    /// report of a newer one.
    fn update(&self, target: &str, filename: &str, f: impl FnOnce(&mut PullOutcome)) {
        if let Ok(mut slot) = self.inner.lock() {
            if let Some(rec) = slot.as_mut() {
                if rec.target == target && rec.filename == filename {
                    f(&mut rec.outcome);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_pulled_reads_none_and_nothing_is_in_flight() {
        let a = PullAttempt::new();
        assert_eq!(a.read(), None);
        assert!(!a.in_flight());
    }

    /// The button's disabled state is this bit: on while the walk runs, off the
    /// moment it ends either way. A pull that ends and leaves it on is a Pull
    /// button that never works again this session.
    #[test]
    fn a_pull_is_in_flight_until_it_ends_either_way() {
        let a = PullAttempt::new();
        a.start("peerA", "photo.jpg");
        assert!(a.in_flight());
        a.progress("peerA", "photo.jpg", 3, 12);
        assert_eq!(a.read().unwrap().outcome, PullOutcome::Pulling { held: 3, total: 12 });
        a.done("peerA", "photo.jpg");
        assert!(!a.in_flight(), "a finished pull must re-enable the button");

        a.start("peerA", "big.iso");
        a.failed("peerA", "big.iso", "Can't reach this device right now");
        assert!(!a.in_flight(), "a FAILED pull must re-enable the button too");
        match a.read().unwrap().outcome {
            PullOutcome::Failed(why) => assert!(why.contains("Can't reach")),
            other => panic!("expected a failure carrying its reason, got {other:?}"),
        }
    }

    /// A late report about an older pull must not overwrite the newer one's —
    /// neither its outcome nor, worse, flip a running pull to done.
    #[test]
    fn a_late_report_about_another_pull_changes_nothing() {
        let a = PullAttempt::new();
        a.start("peerA", "old.bin");
        a.start("peerB", "new.bin");
        a.done("peerA", "old.bin");
        a.failed("peerA", "old.bin", "stale");
        a.progress("peerA", "old.bin", 9, 9);
        let rec = a.read().unwrap();
        assert_eq!((rec.target.as_str(), rec.filename.as_str()), ("peerB", "new.bin"));
        assert!(rec.outcome.in_flight(), "the newer pull is still running: {rec:?}");
    }

    /// Progress that arrives after the end is not a resurrection.
    #[test]
    fn progress_after_the_end_does_not_restart_the_spinner() {
        let a = PullAttempt::new();
        a.start("p", "f");
        a.done("p", "f");
        a.progress("p", "f", 1, 2);
        assert_eq!(a.read().unwrap().outcome, PullOutcome::Done);
    }
}
