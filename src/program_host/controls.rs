//! THE STANDARD CONTROLLER — the input PRESENTATION contract, consumer side.
//!
//! Rust mirror of workbench-go `programs/controls.go` (the reference parser).
//! Keep the two byte/behaviour-identical the same way the `direction` enum is —
//! this is a cross-implementation contract, not a browser-local choice
//! (RESPONSE-GENERIC-HOST-INPUT-DEVICE-MODEL-2026-07-24.md; the browser raised
//! it in PROPOSAL-GENERIC-HOST-INPUT-DEVICE-MODEL).
//!
//! **Transport vs. presentation.** A `key-set` port's held-key bitmask is the
//! wire form (a source ORs `1 << bit` into the mask; the driver never learns
//! what a bit *does*). The bitmask stays. What this file adds is the
//! **presentation** layer: each bit declares a **control ROLE** —
//! a *directional axis* (`up`/`down`/`left`/`right` → the ONE standard d-pad,
//! where simultaneous presses are natural, satisfying rotate+thrust) or a
//! *discrete action* (`fire` → a labelled/glyphed button). Every program then
//! renders the SAME controller; only the bindings differ. This is the
//! generalization of `scene.keymap` from `{bit → name}` to `{bit → role}`.
//!
//! [`parse_keymap`] accepts BOTH forms so programs migrate incrementally:
//! a legacy `{"<bit>": "<name>"}` reads as a momentary ACTION named `<name>`
//! (an un-migrated program still mounts, as buttons — the honest reflection of
//! a program that has not declared roles yet). A malformed entry is an **error**
//! (a control the author declared but a host cannot parse must surface), never a
//! silent drop.

use ciborium::Value;

/// Control roles — how a key-set bit presents on the standard controller.
pub const ROLE_AXIS: &str = "axis";
pub const ROLE_ACTION: &str = "action";

/// Axis positions on the standard directional control. These are the SAME four
/// positions the `direction` shape's enum names (`DIR_UP`/`RIGHT`/`DOWN`/`LEFT`,
/// [`shapes`](super::shapes)) — a directional key-set bit and a `direction` port
/// drive the identical control; only the transport differs (an accumulating
/// bitmask vs. a latched value).
pub const AXIS_UP: &str = "up";
pub const AXIS_DOWN: &str = "down";
pub const AXIS_LEFT: &str = "left";
pub const AXIS_RIGHT: &str = "right";

/// Action behaviours — how a host treats a held vs. tapped action button.
pub const BEHAVIOR_MOMENTARY: &str = "momentary";
pub const BEHAVIOR_TOGGLE: &str = "toggle";

/// The standard action vocabulary's default glyph, used when a binding declares
/// none. Pinned host-agnostic so every implementer's unstyled controller reads
/// the same; a host MAY substitute its own iconography. A program MAY name an
/// action outside this set — it just gets no default glyph.
/// (Items 3/5 of the proposal are still arch's to ratify; this mirrors the set
/// workbench-go seeded so the two stay identical until then.)
pub fn standard_action_glyph(action: &str) -> Option<&'static str> {
    match action {
        "fire" => Some("\u{1F525}"),  // 🔥
        "start" => Some("\u{25B6}"),  // ▶
        "select" => Some("\u{25C9}"), // ◉
        "pause" => Some("\u{23F8}"),  // ⏸
        "restart" => Some("\u{21BB}"), // ↻
        _ => None,
    }
}

/// A control role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Axis,
    Action,
}

/// One key-set bit's presentation role, parsed from a `scene.keymap` entry. The
/// bit stays the transport (mask contribution `1 << bit`); this says how a host
/// renders it and — for the transport layer — what NAME a source presses to set
/// it ([`name`](Self::name)).
#[derive(Debug, Clone)]
pub struct ControlBinding {
    /// Bit index in the held-key mask (contribution `1 << bit`).
    pub bit: u32,
    pub role: Role,
    /// D-pad position (`Role::Axis` only).
    pub axis: String,
    /// Action name (`Role::Action` only).
    pub action: String,
    /// Button face (action only; the d-pad position is self-presenting).
    pub label: String,
    /// Declared glyph; falls back to [`standard_action_glyph`] via
    /// [`effective_glyph`](Self::effective_glyph).
    pub glyph: String,
    /// `momentary` (default) or `toggle`.
    pub behavior: String,
}

impl ControlBinding {
    /// The token a source presses/releases to drive this bit — the axis position
    /// (`"up"`) for an axis, the action name (`"fire"`) for an action. This is
    /// what feeds the transport's action→bit map; a host maps a physical input to
    /// a Name, never to a bit number.
    pub fn name(&self) -> &str {
        match self.role {
            Role::Axis => &self.axis,
            Role::Action => &self.action,
        }
    }

    /// This bit's held-mask contribution.
    pub fn bit_value(&self) -> u64 {
        1u64 << self.bit
    }

    /// `toggle` — the bit FLIPS on each press and ignores release (a latch),
    /// vs. the default `momentary` (held only while pressed). Program-declared
    /// per binding (`scene.keymap`); the host stays program-blind.
    pub fn is_toggle(&self) -> bool {
        self.behavior == BEHAVIOR_TOGGLE
    }

    /// The glyph to render, falling back to the standard-action default. `None`
    /// for an axis with no declared glyph (the d-pad position is self-presenting).
    pub fn effective_glyph(&self) -> Option<String> {
        if !self.glyph.is_empty() {
            return Some(self.glyph.clone());
        }
        if self.role == Role::Action {
            return standard_action_glyph(&self.action).map(str::to_string);
        }
        None
    }
}

fn valid_axis(s: &str) -> bool {
    matches!(s, AXIS_UP | AXIS_DOWN | AXIS_LEFT | AXIS_RIGHT)
}

/// A text field of a CBOR-map value (a keymap entry's `role`/`axis`/…).
fn entry_text<'a>(map: &'a Value, field: &str) -> Option<&'a str> {
    map.as_map()?
        .iter()
        .find(|(k, _)| k.as_text() == Some(field))
        .and_then(|(_, v)| v.as_text())
}

/// Parse a `key-set` port's `scene.keymap` into per-bit control bindings, sorted
/// by bit — the reference parser the standard-controller contract is defined by
/// (mirror of workbench-go `ParseKeymap`). Absent/empty keymap ⇒ empty (the
/// program declared no usable controls; a source installs nothing rather than
/// guess). A malformed entry is an `Err` — a declared-but-unparseable control
/// must surface, never silently vanish.
pub fn parse_keymap(scene: Option<&Value>) -> Result<Vec<ControlBinding>, String> {
    let Some(scene) = scene else {
        return Ok(Vec::new());
    };
    let Some(map) = scene.as_map() else {
        return Ok(Vec::new());
    };
    let Some(raw) = map
        .iter()
        .find(|(k, _)| k.as_text() == Some("keymap"))
        .map(|(_, v)| v)
    else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let Some(entries) = raw.as_map() else {
        return Err("scene.keymap is not a {bit: role} map".into());
    };

    let mut out = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let key = k
            .as_text()
            .ok_or("scene.keymap key is not a string bit index")?;
        let bit: u32 = key
            .parse()
            .ok()
            .filter(|b| *b <= 63)
            .ok_or_else(|| format!("scene.keymap key {key:?} is not a bit index 0..63"))?;
        let b = parse_binding(bit, v).map_err(|e| format!("scene.keymap[{bit}]: {e}"))?;
        out.push(b);
    }
    out.sort_by_key(|b| b.bit);
    Ok(out)
}

fn parse_binding(bit: u32, v: &Value) -> Result<ControlBinding, String> {
    match v {
        // Legacy form: a bare action name → a momentary action.
        Value::Text(t) => {
            if t.is_empty() {
                return Err("empty action name".into());
            }
            Ok(ControlBinding {
                bit,
                role: Role::Action,
                axis: String::new(),
                action: t.clone(),
                label: title_case(t),
                glyph: String::new(),
                behavior: BEHAVIOR_MOMENTARY.into(),
            })
        }
        // Roled form.
        Value::Map(_) => {
            let role = entry_text(v, "role").unwrap_or("");
            match role {
                ROLE_AXIS => {
                    let axis = entry_text(v, "axis").unwrap_or("").to_string();
                    if !valid_axis(&axis) {
                        return Err(format!(
                            "axis role needs axis up|down|left|right, got {axis:?}"
                        ));
                    }
                    Ok(ControlBinding {
                        bit,
                        role: Role::Axis,
                        axis,
                        action: String::new(),
                        label: String::new(),
                        glyph: String::new(),
                        behavior: BEHAVIOR_MOMENTARY.into(),
                    })
                }
                ROLE_ACTION => {
                    let action = entry_text(v, "action").unwrap_or("").to_string();
                    if action.is_empty() {
                        return Err("action role needs a non-empty action name".into());
                    }
                    let mut label = entry_text(v, "label").unwrap_or("").to_string();
                    if label.is_empty() {
                        label = title_case(&action);
                    }
                    let glyph = entry_text(v, "glyph").unwrap_or("").to_string();
                    let mut behavior = entry_text(v, "behavior").unwrap_or("").to_string();
                    if behavior.is_empty() {
                        behavior = BEHAVIOR_MOMENTARY.into();
                    }
                    if behavior != BEHAVIOR_MOMENTARY && behavior != BEHAVIOR_TOGGLE {
                        return Err(format!("unknown behavior {behavior:?}"));
                    }
                    Ok(ControlBinding {
                        bit,
                        role: Role::Action,
                        axis: String::new(),
                        action,
                        label,
                        glyph,
                        behavior,
                    })
                }
                other => Err(format!(
                    "role must be {ROLE_AXIS:?} or {ROLE_ACTION:?}, got {other:?}"
                )),
            }
        }
        _ => Err("entry is not a name string or a {role:...} map".into()),
    }
}

/// Project control bindings to the `(name, bit_value)` pairs the transport layer
/// ([`InputTarget`](super::input::InputTarget)) holds, ordered by bit — mirror of
/// workbench-go `ActionBitMap`. A source presses a Name; the target ORs the bit.
pub fn action_bit_map(bindings: &[ControlBinding]) -> Vec<(String, u64)> {
    bindings
        .iter()
        .map(|b| (b.name().to_string(), b.bit_value()))
        .collect()
}

/// The OR of every `toggle`-behaviour bit — the mask [`InputTarget`] flips on
/// press (a latch) instead of holding. A program with only momentary bindings
/// (the default, and every bundled program today) yields `0`, so the target's
/// behaviour is unchanged. Program-blind: the host reads the declared behaviour,
/// it never decides which controls latch.
pub fn toggle_bit_mask(bindings: &[ControlBinding]) -> u64 {
    bindings
        .iter()
        .filter(|b| b.is_toggle())
        .fold(0, |mask, b| mask | b.bit_value())
}

/// Title-case the first character — a default label from an action name
/// (`fire` → `Fire`). Presentation only; the emitted action is the raw name.
pub fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `scene` Value wrapping a `keymap` map from `(bit_key, entry)` pairs.
    fn scene(entries: Vec<(&str, Value)>) -> Value {
        let km = Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::Text(k.into()), v))
                .collect(),
        );
        Value::Map(vec![(Value::Text("keymap".into()), km)])
    }

    fn roled_axis(axis: &str) -> Value {
        Value::Map(vec![
            (Value::Text("role".into()), Value::Text("axis".into())),
            (Value::Text("axis".into()), Value::Text(axis.into())),
        ])
    }

    fn roled_action(action: &str, glyph: &str) -> Value {
        Value::Map(vec![
            (Value::Text("role".into()), Value::Text("action".into())),
            (Value::Text("action".into()), Value::Text(action.into())),
            (Value::Text("glyph".into()), Value::Text(glyph.into())),
            (
                Value::Text("behavior".into()),
                Value::Text("momentary".into()),
            ),
        ])
    }

    /// The re-declared Asteroids keymap: left/right/thrust are axes, fire is an
    /// action — sorted by bit, with the transport name→bit map preserved.
    #[test]
    fn parses_roled_asteroids_keymap() {
        let s = scene(vec![
            ("0", roled_axis("left")),
            ("1", roled_axis("right")),
            ("2", roled_axis("up")),
            ("3", roled_action("fire", "\u{1F525}")),
        ]);
        let b = parse_keymap(Some(&s)).unwrap();
        assert_eq!(b.len(), 4);
        // Sorted by bit.
        assert_eq!(b[0].role, Role::Axis);
        assert_eq!(b[0].name(), "left");
        assert_eq!(b[2].name(), "up");
        assert_eq!(b[3].role, Role::Action);
        assert_eq!(b[3].name(), "fire");
        assert_eq!(b[3].label, "Fire");
        assert_eq!(b[3].effective_glyph().as_deref(), Some("\u{1F525}"));
        // Transport projection: rotate-right is bit 1 → mask 2 (the e2e pin).
        let m = action_bit_map(&b);
        assert_eq!(m[1], ("right".to_string(), 2));
        assert_eq!(m[3], ("fire".to_string(), 8));
    }

    /// `behavior: toggle` parses and projects into the toggle bit-mask;
    /// momentary (declared or defaulted) does not. No bundled program declares a
    /// toggle yet — this is the exerciser for the ABI field's handling.
    #[test]
    fn toggle_behavior_parses_and_projects_to_the_mask() {
        let toggle_action = |action: &str| {
            Value::Map(vec![
                (Value::Text("role".into()), Value::Text("action".into())),
                (Value::Text("action".into()), Value::Text(action.into())),
                (Value::Text("behavior".into()), Value::Text("toggle".into())),
            ])
        };
        let s = scene(vec![
            ("0", roled_axis("left")),          // axis → momentary by nature
            ("3", roled_action("fire", "\u{1F525}")), // explicit momentary
            ("4", toggle_action("shield")),     // toggle (bit 4 → mask 16)
        ]);
        let b = parse_keymap(Some(&s)).unwrap();
        assert!(!b[0].is_toggle(), "an axis is momentary");
        assert!(!b[1].is_toggle(), "explicit momentary action");
        assert!(b[2].is_toggle(), "shield declared toggle");
        // Only the shield bit (1<<4 = 16) is in the toggle mask.
        assert_eq!(toggle_bit_mask(&b), 16);
    }

    /// A program with no toggle bindings (every bundled program today) yields a
    /// zero toggle mask — so `InputTarget` behaves exactly as before.
    #[test]
    fn no_toggle_bindings_yields_zero_mask() {
        let s = scene(vec![("1", roled_axis("right")), ("3", roled_action("fire", "\u{1F525}"))]);
        let b = parse_keymap(Some(&s)).unwrap();
        assert_eq!(toggle_bit_mask(&b), 0);
    }

    /// Legacy `{bit: "name"}` still parses — as a momentary action — so an
    /// un-migrated program keeps mounting (the incremental-migration guarantee).
    #[test]
    fn parses_legacy_string_keymap_as_actions() {
        let s = scene(vec![
            ("1", Value::Text("right".into())),
            ("3", Value::Text("fire".into())),
        ]);
        let b = parse_keymap(Some(&s)).unwrap();
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|x| x.role == Role::Action));
        assert_eq!(b[0].name(), "right");
        assert_eq!(b[0].label, "Right");
        assert_eq!(action_bit_map(&b)[1], ("fire".to_string(), 8));
    }

    /// A default glyph fills in for a bare standard action; a non-standard action
    /// gets none.
    #[test]
    fn standard_action_default_glyph() {
        let s = scene(vec![
            ("0", Value::Text("fire".into())),
            ("1", Value::Text("wibble".into())),
        ]);
        let b = parse_keymap(Some(&s)).unwrap();
        assert_eq!(b[0].effective_glyph().as_deref(), Some("\u{1F525}"));
        assert_eq!(b[1].effective_glyph(), None);
    }

    /// A control the author declared but a host cannot parse must SURFACE
    /// (error), never silently vanish.
    #[test]
    fn malformed_entries_error_not_drop() {
        // Bad axis position.
        let s = scene(vec![("0", roled_axis("sideways"))]);
        assert!(parse_keymap(Some(&s)).is_err());
        // Unknown role.
        let bad_role = Value::Map(vec![(Value::Text("role".into()), Value::Text("wat".into()))]);
        let s = scene(vec![("0", bad_role)]);
        assert!(parse_keymap(Some(&s)).is_err());
        // Out-of-range bit.
        let s = scene(vec![("64", Value::Text("fire".into()))]);
        assert!(parse_keymap(Some(&s)).is_err());
        // Action role with empty action name.
        let empty_action = Value::Map(vec![
            (Value::Text("role".into()), Value::Text("action".into())),
            (Value::Text("action".into()), Value::Text("".into())),
        ]);
        let s = scene(vec![("0", empty_action)]);
        assert!(parse_keymap(Some(&s)).is_err());
    }

    /// No keymap ⇒ empty (install nothing), not an error.
    #[test]
    fn absent_keymap_is_empty() {
        assert!(parse_keymap(None).unwrap().is_empty());
        let no_keymap = Value::Map(vec![(Value::Text("wrap".into()), Value::Bool(true))]);
        assert!(parse_keymap(Some(&no_keymap)).unwrap().is_empty());
    }
}
