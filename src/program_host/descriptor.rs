//! `app/program/interface` descriptor — strict decode, validation,
//! admission.
//!
//! Field names are the cross-impl ABI (workbench-go
//! `program_descriptor.go` cbor tags, verbatim). Decode is **strict**
//! (unlike the app's lossy content formats): a descriptor that doesn't
//! decode is an admission refusal with a reason, never a default.

use ciborium::Value;

/// Descriptor entity type.
pub const INTERFACE_TYPE: &str = "app/program/interface";
/// Pinned well-known descriptor leaf under a program root.
pub const DESCRIPTOR_LEAF: &str = "/interface";

/// Port roles (hints; `shape` is the driver selector).
pub const ROLE_DISPLAY: &str = "display";
pub const ROLE_INPUT: &str = "input";

/// Shapes (driver-binding discriminants).
pub const SHAPE_TEXT: &str = "text";
pub const SHAPE_DISPLAY_LIST: &str = "display-list";
pub const SHAPE_KEY_SET: &str = "key-set";
pub const SHAPE_DIRECTION: &str = "direction";

pub const TICK_CLOCK_DRIVEN: &str = "clock-driven";

#[derive(Debug, Clone)]
pub struct ProgramDescriptor {
    pub state_path: String,
    pub initial_state: String,
    pub step: String,
    pub step_hash: Option<String>,
    pub initial_state_hash: Option<String>,
    pub input_ports: Vec<ProgramPort>,
    pub output_ports: Vec<ProgramPort>,
    pub tick: ProgramTick,
    /// Present ⇒ sharded program. The POC host refuses these at
    /// admission (named, not silent); decoded only far enough to detect.
    pub shard: Option<Value>,
    /// Capability-handler imports (proposal §5). Refused while the
    /// wasm32 `compute/apply` dispatch stub stands (B1) — advertising
    /// hostability the substrate errors on would violate the `offered`
    /// accuracy MUST.
    pub imports: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ProgramPort {
    pub name: String,
    pub path: String,
    pub type_ref: String,
    pub kind: String,
    pub role: String,
    pub shape: String,
    /// Non-empty ⇒ projection: `put(path, eval(source))` each tick.
    pub source: Option<String>,
    /// Per-shape metadata (`wrap`, `bounds`, `mode`, `cols`, `rows`,
    /// `keymap`, …) — kept opaque here; drivers extract what they know.
    pub scene: Option<Value>,
    /// F-E1 seed path (input ports).
    pub initial: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProgramTick {
    pub mode: String,
    pub rate_hint: u64,
    #[allow(dead_code)] // carried per the ABI; unused by the base host
    pub op_cost: Option<u64>,
}

impl ProgramDescriptor {
    /// Strict decode from a descriptor entity's CBOR data.
    pub fn decode(entity: &entity_entity::Entity) -> Result<Self, String> {
        if entity.entity_type != INTERFACE_TYPE {
            return Err(format!(
                "descriptor type {} != {INTERFACE_TYPE}",
                entity.entity_type
            ));
        }
        let value: Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|e| format!("descriptor cbor: {e}"))?;
        let map = value.as_map().ok_or("descriptor: not a map")?;

        let mut d = ProgramDescriptor {
            state_path: String::new(),
            initial_state: String::new(),
            step: String::new(),
            step_hash: None,
            initial_state_hash: None,
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            tick: ProgramTick {
                mode: String::new(),
                rate_hint: 0,
                op_cost: None,
            },
            shard: None,
            imports: Vec::new(),
        };

        for (k, v) in map {
            match k.as_text() {
                Some("state_path") => d.state_path = req_text(v, "state_path")?,
                Some("initial_state") => d.initial_state = req_text(v, "initial_state")?,
                Some("step") => d.step = req_text(v, "step")?,
                Some("step_hash") => d.step_hash = Some(req_text(v, "step_hash")?),
                Some("initial_state_hash") => {
                    d.initial_state_hash = Some(req_text(v, "initial_state_hash")?)
                }
                Some("input_ports") => d.input_ports = decode_ports(v, "input_ports")?,
                Some("output_ports") => d.output_ports = decode_ports(v, "output_ports")?,
                Some("tick") => d.tick = decode_tick(v)?,
                Some("shard") => {
                    if !v.is_null() {
                        d.shard = Some(v.clone());
                    }
                }
                Some("imports") => {
                    if let Some(arr) = v.as_array() {
                        d.imports = arr.clone();
                    }
                }
                _ => {} // MUST-ignore unknowns
            }
        }
        d.validate()?;
        Ok(d)
    }

    /// The workbench validation rules, mirrored.
    fn validate(&self) -> Result<(), String> {
        if self.state_path.is_empty() {
            return Err("descriptor: state_path required".into());
        }
        if self.step.is_empty() {
            return Err("descriptor: step required".into());
        }
        if self.initial_state.is_empty() {
            return Err("descriptor: initial_state required".into());
        }
        if self.initial_state == self.state_path {
            return Err("descriptor: initial_state must differ from state_path \
                        (reseed would be destroyed by the first tick)"
                .into());
        }
        if self.tick.mode == TICK_CLOCK_DRIVEN && self.tick.rate_hint == 0 {
            return Err("descriptor: clock-driven tick needs rate_hint != 0".into());
        }
        for p in &self.input_ports {
            if p.initial.as_deref().unwrap_or("").is_empty() {
                return Err(format!(
                    "descriptor: input port {} has no initial (F-E1: an unseeded \
                     input read is a compute/error at tick 0)",
                    p.name
                ));
            }
            if p.source.is_some() {
                return Err(format!(
                    "descriptor: input port {} declares source (projections are \
                     output-port semantics)",
                    p.name
                ));
            }
        }
        Ok(())
    }

    /// Admission — can THIS host mount the program? Refusals carry the
    /// reason the window shows (fails closed, never mounts-then-faults).
    pub fn admit(&self, supported_shapes: &[&str]) -> Result<(), String> {
        if !self.imports.is_empty() {
            return Err(format!(
                "program declares {} capability import(s); handler dispatch is \
                 not available on this substrate (wasm32 compute/apply stub) — \
                 refusing rather than advertising hostability that errors at tick 1",
                self.imports.len()
            ));
        }
        if self.shard.is_some() {
            return Err(
                "sharded program (static-k) — not supported by this host yet".into(),
            );
        }
        if self.tick.mode != TICK_CLOCK_DRIVEN {
            return Err(format!(
                "tick mode {:?} not supported (clock-driven only)",
                self.tick.mode
            ));
        }
        for p in self.input_ports.iter().chain(self.output_ports.iter()) {
            if !supported_shapes.contains(&p.shape.as_str()) {
                return Err(format!(
                    "port {} binds shape {:?}; this host drives {:?}",
                    p.name, p.shape, supported_shapes
                ));
            }
        }
        Ok(())
    }

    /// The output port a display driver binds (role `display`), if any.
    pub fn display_port(&self) -> Option<&ProgramPort> {
        self.output_ports.iter().find(|p| p.role == ROLE_DISPLAY)
    }
}

fn req_text(v: &Value, name: &str) -> Result<String, String> {
    v.as_text()
        .map(str::to_string)
        .ok_or_else(|| format!("descriptor: {name} not a string"))
}

fn decode_ports(v: &Value, name: &str) -> Result<Vec<ProgramPort>, String> {
    // Go encodes a nil slice as CBOR null (Life has no input ports).
    if v.is_null() {
        return Ok(Vec::new());
    }
    let arr = v
        .as_array()
        .ok_or_else(|| format!("descriptor: {name} not an array"))?;
    arr.iter()
        .enumerate()
        .map(|(i, item)| decode_port(item).map_err(|e| format!("{name}[{i}]: {e}")))
        .collect()
}

fn decode_port(v: &Value) -> Result<ProgramPort, String> {
    let map = v.as_map().ok_or("port: not a map")?;
    let mut p = ProgramPort {
        name: String::new(),
        path: String::new(),
        type_ref: String::new(),
        kind: String::new(),
        role: String::new(),
        shape: String::new(),
        source: None,
        scene: None,
        initial: None,
    };
    for (k, val) in map {
        match k.as_text() {
            Some("name") => p.name = req_text(val, "name")?,
            Some("path") => p.path = req_text(val, "path")?,
            Some("type_ref") => p.type_ref = req_text(val, "type_ref")?,
            Some("kind") => p.kind = req_text(val, "kind")?,
            Some("role") => p.role = req_text(val, "role")?,
            Some("shape") => p.shape = req_text(val, "shape")?,
            Some("source") => {
                let s = req_text(val, "source")?;
                if !s.is_empty() {
                    p.source = Some(s);
                }
            }
            Some("scene") => p.scene = Some(val.clone()),
            Some("initial") => {
                let s = req_text(val, "initial")?;
                if !s.is_empty() {
                    p.initial = Some(s);
                }
            }
            _ => {}
        }
    }
    if p.name.is_empty() || p.path.is_empty() || p.shape.is_empty() {
        return Err("port: name/path/shape required".into());
    }
    Ok(p)
}

fn decode_tick(v: &Value) -> Result<ProgramTick, String> {
    let map = v.as_map().ok_or("tick: not a map")?;
    let mut t = ProgramTick {
        mode: String::new(),
        rate_hint: 0,
        op_cost: None,
    };
    for (k, val) in map {
        match k.as_text() {
            Some("mode") => t.mode = req_text(val, "mode")?,
            Some("rate_hint") => t.rate_hint = as_u64(val).ok_or("tick: rate_hint not uint")?,
            Some("op_cost") => t.op_cost = as_u64(val),
            _ => {}
        }
    }
    Ok(t)
}

/// A `scene` field as u64 (drivers use this for `cols`/`rows`/`bounds`).
pub fn scene_u64(scene: Option<&Value>, field: &str) -> Option<u64> {
    scene?.as_map()?.iter().find_map(|(k, v)| {
        (k.as_text() == Some(field)).then(|| as_u64(v)).flatten()
    })
}

/// A `scene` field as text.
pub fn scene_text<'a>(scene: Option<&'a Value>, field: &str) -> Option<&'a str> {
    scene?.as_map()?.iter().find_map(|(k, v)| {
        if k.as_text() == Some(field) {
            v.as_text()
        } else {
            None
        }
    })
}

/// A `scene` field as bool.
pub fn scene_bool(scene: Option<&Value>, field: &str) -> Option<bool> {
    scene?.as_map()?.iter().find_map(|(k, v)| {
        if k.as_text() == Some(field) {
            v.as_bool()
        } else {
            None
        }
    })
}

pub fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Integer(i) => {
            let n: i128 = (*i).into();
            u64::try_from(n).ok()
        }
        _ => None,
    }
}

/// A CBOR integer as `i64` (display-list vertices are signed world-space).
pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => {
            let n: i128 = (*i).into();
            i64::try_from(n).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_host::bundle::{Bundle, EMBEDDED_PROGRAMS};

    fn descriptor_of(key: &str) -> ProgramDescriptor {
        let p = EMBEDDED_PROGRAMS.iter().find(|p| p.key == key).unwrap();
        let b = Bundle::parse(p.json).unwrap();
        let ents = b.verified_entities().unwrap();
        let (_, ent) = ents
            .iter()
            .find(|(path, _)| *path == b.descriptor_path)
            .expect("descriptor entity present");
        ProgramDescriptor::decode(ent).expect(key)
    }

    #[test]
    fn life_descriptor_decodes_and_admits_on_text() {
        let d = descriptor_of("life");
        assert_eq!(d.state_path, "app/life/state");
        assert_eq!(d.tick.mode, TICK_CLOCK_DRIVEN);
        assert_eq!(d.tick.rate_hint, 6);
        assert!(d.input_ports.is_empty());
        let disp = d.display_port().expect("display port");
        assert_eq!(disp.shape, SHAPE_TEXT);
        assert_eq!(disp.type_ref, "app/shape/text-frame");
        assert!(disp.source.is_some(), "display is a projection");
        assert_eq!(
            crate::program_host::descriptor::scene_u64(disp.scene.as_ref(), "cols"),
            Some(16)
        );
        d.admit(&[SHAPE_TEXT]).expect("admits on text");
    }

    #[test]
    fn snake_needs_direction_driver() {
        let d = descriptor_of("snake");
        assert_eq!(d.input_ports.len(), 1);
        assert_eq!(d.input_ports[0].shape, SHAPE_DIRECTION);
        assert!(d.input_ports[0].initial.is_some(), "F-E1 seed");
        // A text-only host refuses Snake, with the shape named.
        let err = d.admit(&[SHAPE_TEXT]).unwrap_err();
        assert!(err.contains("direction"), "{err}");
        d.admit(&[SHAPE_TEXT, SHAPE_DIRECTION]).expect("admits");
    }

    #[test]
    fn asteroids_binds_display_list_and_key_set() {
        let d = descriptor_of("asteroids");
        let disp = d.display_port().unwrap();
        assert_eq!(disp.shape, SHAPE_DISPLAY_LIST);
        assert_eq!(scene_bool(disp.scene.as_ref(), "wrap"), Some(true));
        assert_eq!(scene_u64(disp.scene.as_ref(), "bounds"), Some(65536));
        assert_eq!(d.input_ports[0].shape, SHAPE_KEY_SET);
        assert_eq!(d.tick.rate_hint, 12);
        // The nested keymap scene must survive decode (the Go interface-
        // keyed map trap).
        let scene = d.input_ports[0].scene.as_ref().expect("keymap scene");
        assert!(scene.as_map().is_some());
    }

    #[test]
    fn admission_refuses_unsupported_shape_with_reason() {
        let d = descriptor_of("asteroids");
        let err = d.admit(&[SHAPE_TEXT, SHAPE_DIRECTION]).unwrap_err();
        assert!(err.contains("key-set") || err.contains("display-list"), "{err}");
    }
}
