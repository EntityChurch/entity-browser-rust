//! In-memory outcome of the **last file offered** — the app-owned "I picked a
//! file and here is what happened to it" fact, so that button can never again
//! complete in silence.
//!
//! Why this exists: `Action::OfferFile`'s only report was a line in the File
//! Transfer window's *Results* pane at the bottom of the window. Every way an
//! offer can fail lands there and nowhere else — the file is over
//! [`MAX_OFFER_BYTES`](crate::file_offer::MAX_OFFER_BYTES), the local peer is
//! not routed, the ingest is refused — and on a phone, where the window is a
//! few hundred pixels tall and the picker takes over the screen, that pane is
//! not where anyone is looking. Reported from a real Android run as *"I hit the
//! button, it doesn't work, doesn't give an error, doesn't show anything"*,
//! which is a correct bug report about the feedback whatever the underlying
//! cause turned out to be. D13: say it, next to the control that did it.
//!
//! It also covers the half the app cannot see at all. Between the tap and
//! `Action::OfferFile` there is a file picker and an `array_buffer()` read, and
//! both can end without an action ever being raised — a refusal on size (which
//! we make *before* reading, so a 200 MB video does not get pulled into wasm
//! memory just to be turned down) or a read that fails. The DOM writes those
//! here directly, which is why this is a shared handle on
//! [`DomCtx`](crate::dom::util::DomCtx) rather than app-private state.
//!
//! Why in-memory and not a tree entity — the same reasoning as
//! [`crate::connect_attempt`], which this deliberately mirrors: it is one
//! button press's progress, meaningless across a reload, and a durable copy
//! would strand a `Preparing` row forever when a reload lands mid-ingest.
//!
//! **Not the offer list.** What we are serving is read from the tree
//! (`file_offer::read_own_offers`) and is the authority on what exists; this
//! reports on *the press*. They disagree in exactly the useful way: a failed
//! offer shows a reason here and no row there, and a withdrawn offer loses its
//! row while this still says the press it describes succeeded.

use std::sync::{Arc, Mutex};

/// What became of the last file the user offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferOutcome {
    /// The browser is handing us the bytes (`File::array_buffer`). On a phone
    /// this is the slow step for anything camera-sized.
    Reading,
    /// Chunking + `system/content:ingest` in flight.
    Preparing,
    /// Published. Carries the size, though the window's own offers table is the
    /// authority on what is being served — this only says the press worked.
    Offered(u64),
    /// It did not happen. Carries the reason verbatim, because a bare "failed"
    /// leaves the user exactly as stuck as the silence did.
    Failed(String),
    /// "Save to this device" on one of our own offers: reading its bytes back
    /// out of our own store. The same card, the same one status line — a press
    /// on a row is a press on this card, and only the latest is waited on.
    Saving,
    /// Handed to the browser as a download. Renders nothing: the download is
    /// the feedback, and a line saying so would outlive it.
    Saved(u64),
    /// The save did not happen, and this is why.
    SaveFailed(String),
}

impl OfferOutcome {
    /// Is something still happening? The two in-flight states render as one
    /// spinner — the distinction matters to a log reader, not to the person
    /// waiting.
    pub fn in_flight(&self) -> bool {
        matches!(self, OfferOutcome::Reading | OfferOutcome::Preparing | OfferOutcome::Saving)
    }
}

/// Cheap-to-clone handle to the app's single last-offer slot. Cloned into the
/// render context ([`crate::dom::util::DomCtx`]) so the File Transfer window
/// reads it at render — one owner, no per-window copy.
///
/// Only the most recent press is kept: there is one picker, so an older press's
/// outcome is never the one the user is waiting on.
#[derive(Clone, Default)]
pub struct OfferAttempt {
    inner: Arc<Mutex<Option<(String, OfferOutcome)>>>,
}

#[allow(dead_code)] // the writers are WASM-only (DOM picker + app action path)
impl OfferAttempt {
    pub fn new() -> Self {
        Self::default()
    }

    /// The browser is reading `filename`'s bytes.
    pub fn set_reading(&self, filename: &str) {
        self.set(filename, OfferOutcome::Reading);
    }

    /// The bytes are in hand; chunk + ingest are running.
    pub fn set_preparing(&self, filename: &str) {
        self.set(filename, OfferOutcome::Preparing);
    }

    /// `filename` is published at `size` bytes.
    pub fn set_offered(&self, filename: &str, size: u64) {
        self.set(filename, OfferOutcome::Offered(size));
    }

    /// `filename` was not offered, and this is why.
    pub fn set_failed(&self, filename: &str, reason: &str) {
        self.set(filename, OfferOutcome::Failed(reason.to_string()));
    }

    /// `filename`'s bytes are being read back to save them to this device.
    pub fn set_saving(&self, filename: &str) {
        self.set(filename, OfferOutcome::Saving);
    }

    /// `filename` was handed to the browser as a download of `size` bytes.
    pub fn set_saved(&self, filename: &str, size: u64) {
        self.set(filename, OfferOutcome::Saved(size));
    }

    /// `filename` was not saved to this device, and this is why.
    pub fn set_save_failed(&self, filename: &str, reason: &str) {
        self.set(filename, OfferOutcome::SaveFailed(reason.to_string()));
    }

    fn set(&self, filename: &str, outcome: OfferOutcome) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = Some((filename.to_string(), outcome));
        }
    }

    /// Drop the outcome.
    pub fn clear(&self) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = None;
        }
    }

    /// The last press's filename + outcome, `None` when nothing has been
    /// offered this session. Tolerant read: a poisoned lock reads as `None`
    /// rather than panicking in the render path.
    pub fn read(&self) -> Option<(String, OfferOutcome)> {
        self.inner.lock().ok().and_then(|slot| slot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_offered_reads_none() {
        assert_eq!(OfferAttempt::new().read(), None);
    }

    /// The reason is the entire value of this surface. A bare "failed" — or a
    /// success the user has to go and look for — is the silence this replaces.
    #[test]
    fn a_failure_carries_its_reason_not_just_a_flag() {
        let a = OfferAttempt::new();
        a.set_failed("holiday.mp4", "holiday.mp4 is 48.2 MB — this browser offers files up to 16.0 MB");
        let (name, outcome) = a.read().unwrap();
        assert_eq!(name, "holiday.mp4");
        match outcome {
            OfferOutcome::Failed(why) => assert!(
                why.contains("16.0 MB"),
                "the ceiling has to be IN the refusal, or the remedy is a guess: {why}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// One picker, one slot: a second press replaces the first, and the first
    /// press's stale outcome can never be mistaken for this one's.
    #[test]
    fn the_latest_press_is_the_one_reported() {
        let a = OfferAttempt::new();
        a.set_failed("first.bin", "nope");
        a.set_reading("second.bin");
        assert_eq!(a.read(), Some(("second.bin".into(), OfferOutcome::Reading)));
        a.set_preparing("second.bin");
        assert!(a.read().unwrap().1.in_flight());
        a.set_offered("second.bin", 12);
        assert!(!a.read().unwrap().1.in_flight(), "a finished press is not a spinner");
    }

    #[test]
    fn a_save_is_a_press_on_the_same_card_and_reports_the_same_way() {
        let a = OfferAttempt::new();
        a.set_offered("note.txt", 5);
        a.set_saving("note.txt");
        assert!(a.read().unwrap().1.in_flight(), "a save in flight is a spinner");
        a.set_save_failed("note.txt", "this browser no longer holds the file's contents");
        match a.read().unwrap().1 {
            OfferOutcome::SaveFailed(why) => assert!(why.contains("no longer holds")),
            other => panic!("expected a save failure, got {other:?}"),
        }
        a.set_saved("note.txt", 5);
        assert!(!a.read().unwrap().1.in_flight());
    }

    #[test]
    fn clearing_returns_to_nothing_attempted() {
        let a = OfferAttempt::new();
        a.set_offered("x.bin", 1);
        a.clear();
        assert_eq!(a.read(), None);
    }
}
