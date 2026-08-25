//! Program input target — the source- and boundary-agnostic write side of the
//! `(role, shape)` input ABI.
//!
//! An [`InputTarget`] holds one program input port's encode context (its
//! `type_ref` plus the seed's `field` name) and its per-shape live STATE (the
//! held-key mask for `key-set`), and turns modality-neutral verbs — [`set_direction`], and
//! [`press`]/[`release`] — into an encoded shape entity handed to a
//! **context-injected** `deliver` closure.
//!
//! This is the reuse pivot for input. The target knows nothing about *where* a
//! value came from (keyboard, on-screen buttons, a test) or *how* it reaches the
//! peer (an iframe payload's `spawn_local` inner-peer write vs. a window's
//! dispatched put). A **source** translates its events into the verbs; the
//! **context** supplies `deliver`. So a second input source is exactly that — a
//! second source, not a second code path — and the target is equally at home
//! behind the iframe boundary or on a local peer. It stays program-blind: the
//! program owns name↔bit (its `scene.keymap`, parsed into control bindings by
//! [`controls`](super::controls)); the host contributes no app-level knowledge here.
//!
//! [`set_direction`]: InputTarget::set_direction
//! [`press`]: InputTarget::press
//! [`release`]: InputTarget::release

use std::cell::Cell;

use entity_entity::Entity;

use super::shapes;

/// How a modality-neutral verb becomes a value. `Direction` is latest-wins;
/// `KeySet` accumulates a held-key bitmask from named actions.
enum Mode {
    Direction,
    KeySet {
        /// action name → its bit value (`1 << bit_index`), from the program's
        /// `scene.keymap`. Program-declared; the host never invents a bit.
        action_bit: Vec<(String, u64)>,
        /// The OR of the bits whose binding declared `behavior: toggle` — those
        /// FLIP on press and ignore release (a latch), instead of holding while
        /// pressed. `0` for every all-momentary program (the default, and every
        /// bundled program today), so the behaviour is unchanged there.
        toggle_bits: u64,
        /// The live held-key mask, shared across every source driving this port
        /// (share the target as `Rc<InputTarget>` so a keyboard key and an
        /// on-screen button contribute to ONE mask, never two racing copies).
        mask: Cell<u64>,
    },
}

/// One program input port as a modality-neutral write target. Share it
/// (`Rc<InputTarget>`) across a port's sources so they contribute to one state.
pub struct InputTarget {
    type_ref: String,
    field: String,
    mode: Mode,
    /// Context-injected delivery: `(value, encoded entity)`. The iframe payload
    /// stamps its D13 surface + `spawn_local`s the inner-peer write; a window
    /// would route a dispatched put. The target itself stays platform-free.
    deliver: Box<dyn Fn(u64, Entity)>,
}

impl InputTarget {
    /// A `direction`-shape target (latest press wins).
    pub fn direction(type_ref: String, field: String, deliver: Box<dyn Fn(u64, Entity)>) -> Self {
        Self {
            type_ref,
            field,
            mode: Mode::Direction,
            deliver,
        }
    }

    /// A `key-set`-shape target with the program's name→bit map (from
    /// [`controls::action_bit_map`](super::controls::action_bit_map)). Sources
    /// call [`press`](Self::press)/[`release`](Self::release).
    pub fn key_set(
        type_ref: String,
        field: String,
        action_bit: Vec<(String, u64)>,
        toggle_bits: u64,
        deliver: Box<dyn Fn(u64, Entity)>,
    ) -> Self {
        Self {
            type_ref,
            field,
            mode: Mode::KeySet {
                action_bit,
                toggle_bits,
                mask: Cell::new(0),
            },
            deliver,
        }
    }

    /// `direction`: set the latest direction (no-op on a non-direction target).
    pub fn set_direction(&self, dir: u64) {
        if matches!(self.mode, Mode::Direction) {
            self.emit(dir);
        }
    }

    /// `key-set`: press a named action. A **momentary** action sets its bit
    /// (held while pressed); a **toggle** action FLIPS its bit (a latch). No-op
    /// if the target isn't key-set or the program didn't declare the action.
    /// Sources must debounce auto-repeat (a held key must not re-press) so a
    /// toggle flips exactly once per physical press — see `app_host::input`.
    pub fn press(&self, action: &str) {
        self.set_action(action, true);
    }

    /// `key-set`: release a named action. A **momentary** action clears its bit;
    /// a **toggle** action ignores release (the latch persists until re-pressed).
    pub fn release(&self, action: &str) {
        self.set_action(action, false);
    }

    /// Clear held **momentary** actions and emit the snapshot. The stuck-key
    /// guard: a source calls this when it can no longer observe releases (window
    /// `blur`, document hidden, a cancelled pointer) so a key held at that moment
    /// doesn't stay latched forever. **Latched toggles are preserved** — a toggle
    /// is intentionally on (like caps lock), not a key stuck down, so it survives
    /// focus loss. For an all-momentary program (`toggle_bits == 0`) this clears
    /// the whole mask exactly as before. No-op on a `direction` target
    /// (latest-wins holds no accumulated state).
    pub fn release_all(&self) {
        if let Mode::KeySet { mask, toggle_bits, .. } = &self.mode {
            let next = mask.get() & toggle_bits; // keep toggles, drop held momentary
            if next != mask.get() {
                mask.set(next);
                self.emit(next);
            }
        }
    }

    fn set_action(&self, action: &str, on: bool) {
        let Mode::KeySet { action_bit, toggle_bits, mask } = &self.mode else {
            return;
        };
        let Some((_, bit)) = action_bit.iter().find(|(a, _)| a == action) else {
            return;
        };
        let cur = mask.get();
        let next = if toggle_bits & bit != 0 {
            // Toggle: flip on press; release is a no-op (the latch holds until
            // the next press). The source debounces auto-repeat, so a held key
            // flips exactly once.
            if on { cur ^ bit } else { cur }
        } else {
            // Momentary: held while pressed.
            if on { cur | bit } else { cur & !bit }
        };
        if next != cur {
            mask.set(next);
            self.emit(next);
        }
    }

    /// Encode `value` as this port's entity and hand it to the injected
    /// delivery. Encode faults log loudly (never silent) and drop the write.
    fn emit(&self, value: u64) {
        match shapes::encode_input(&self.type_ref, &self.field, value) {
            Ok(entity) => (self.deliver)(value, entity),
            Err(e) => tracing::error!(field = %self.field, "input target: encode: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_host::shapes::{DIRECTION_TYPE, DIR_LEFT, DIR_UP, KEY_SET_TYPE};
    use std::rc::Rc;

    /// A recording delivery — the reusable target's write side without any DOM
    /// or peer, proving it is boundary/modality/platform-free.
    fn recorder() -> (Box<dyn Fn(u64, Entity)>, Rc<std::cell::RefCell<Vec<u64>>>) {
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = log.clone();
        (Box::new(move |value, _entity| sink.borrow_mut().push(value)), log)
    }

    #[test]
    fn direction_latest_wins() {
        let (deliver, log) = recorder();
        let t = InputTarget::direction(DIRECTION_TYPE.into(), "dir".into(), deliver);
        t.set_direction(DIR_UP);
        t.set_direction(DIR_LEFT);
        // press/release are no-ops on a direction target (wrong-mode guard).
        t.press("fire");
        assert_eq!(*log.borrow(), vec![DIR_UP, DIR_LEFT]);
    }

    #[test]
    fn key_set_shared_mask_accumulates_and_dedups() {
        let (deliver, log) = recorder();
        // Asteroids-style: right=bit1(2), thrust=bit2(4), fire=bit3(8).
        let action_bit = vec![
            ("right".to_string(), 2u64),
            ("thrust".to_string(), 4u64),
            ("fire".to_string(), 8u64),
        ];
        // Rc-shared: two "sources" (e.g. keyboard + on-screen) drive ONE mask.
        let t = Rc::new(InputTarget::key_set(
            KEY_SET_TYPE.into(),
            "keys".into(),
            action_bit,
            0, // all momentary
            deliver,
        ));
        let a = t.clone();
        let b = t.clone();
        a.press("right"); // 2
        b.press("thrust"); // 2|4 = 6  (shared mask, not a second copy)
        a.press("right"); // no change → no emit (dedup)
        b.release("right"); // 6 & !2 = 4
        assert_eq!(*log.borrow(), vec![2, 6, 4]);
    }

    #[test]
    fn release_all_clears_held_mask_once() {
        let (deliver, log) = recorder();
        let action_bit = vec![("thrust".to_string(), 4u64), ("fire".to_string(), 8u64)];
        let t = InputTarget::key_set(KEY_SET_TYPE.into(), "keys".into(), action_bit, 0, deliver);
        t.press("thrust"); // 4
        t.press("fire"); // 4|8 = 12
        t.release_all(); // 0 — the stuck-key guard (blur/hidden)
        t.release_all(); // already 0 → no emit (dedup)
        assert_eq!(*log.borrow(), vec![4, 12, 0]);
    }

    #[test]
    fn release_all_is_noop_on_direction() {
        let (deliver, log) = recorder();
        let t = InputTarget::direction(DIRECTION_TYPE.into(), "dir".into(), deliver);
        t.set_direction(DIR_LEFT);
        t.release_all(); // latest-wins holds no state → nothing to clear, no emit
        assert_eq!(*log.borrow(), vec![DIR_LEFT]);
    }

    #[test]
    fn key_set_ignores_undeclared_action() {
        let (deliver, log) = recorder();
        let t = InputTarget::key_set(KEY_SET_TYPE.into(), "keys".into(), vec![], 0, deliver);
        t.press("fire"); // program declared no bit → no-op
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn toggle_flips_on_press_and_ignores_release() {
        let (deliver, log) = recorder();
        // fire = bit3 (8) momentary; shield = bit4 (16) toggle.
        let action_bit = vec![("fire".to_string(), 8u64), ("shield".to_string(), 16u64)];
        let t = InputTarget::key_set(KEY_SET_TYPE.into(), "keys".into(), action_bit, 16, deliver);
        t.press("shield"); // flip on → 16
        t.release("shield"); // toggle: release is a no-op → still 16 (no emit)
        t.press("shield"); // flip off → 0
        t.press("fire"); // momentary, coexists → 8
        t.release("fire"); // momentary clears → 0
        assert_eq!(*log.borrow(), vec![16, 0, 8, 0]);
    }

    #[test]
    fn release_all_preserves_latched_toggles_clears_momentary() {
        let (deliver, log) = recorder();
        let action_bit = vec![("fire".to_string(), 8u64), ("shield".to_string(), 16u64)];
        let t = InputTarget::key_set(KEY_SET_TYPE.into(), "keys".into(), action_bit, 16, deliver);
        t.press("shield"); // latch on → 16
        t.press("fire"); // + momentary → 24
        t.release_all(); // blur: drop momentary (8), KEEP the toggle latch (16) → 16
        assert_eq!(*log.borrow(), vec![16, 24, 16]);
    }
}
