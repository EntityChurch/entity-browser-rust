//! Shape payload decoders — the driver side of the `(role, shape)` ABI.
//!
//! Field names are pinned by the workbench-go shape structs
//! (`program_shapes.go`); a name drift here decodes to zero with green
//! logs and a blank board (their documented `{"Cols":16}` bug), so the
//! decoders are strict: required fields present, lengths consistent.

use ciborium::Value;

use super::descriptor::as_u64;

/// `app/shape/text-frame` — `{cols, rows, cells}`, cells = code points,
/// row-major, `len == cols*rows`.
pub const TEXT_FRAME_TYPE: &str = "app/shape/text-frame";

/// `app/shape/display-list` — struct-of-arrays closed quads (the
/// workbench de-facto ABI; arity divergence from the spec is flagged
/// upstream, companion review F2).
pub const DISPLAY_LIST_TYPE: &str = "app/shape/display-list";

/// `app/shape/direction` — `{dir}`, Up=0 Right=1 Down=2 Left=3.
pub const DIRECTION_TYPE: &str = "app/shape/direction";
pub const DIR_UP: u64 = 0;
pub const DIR_RIGHT: u64 = 1;
pub const DIR_DOWN: u64 = 2;
pub const DIR_LEFT: u64 = 3;

/// `app/shape/key-set` — a held-key bitmask snapshot. NOTE the field
/// name is owned by the program's seed entity (`keys` for the shipped
/// Asteroids), not assumed — see `input_field_name`.
pub const KEY_SET_TYPE: &str = "app/shape/key-set";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFrame {
    pub cols: u64,
    pub rows: u64,
    pub cells: Vec<u64>,
}

impl TextFrame {
    pub fn decode(entity: &entity_entity::Entity) -> Result<Self, String> {
        if entity.entity_type != TEXT_FRAME_TYPE {
            return Err(format!(
                "text-frame: entity type {} != {TEXT_FRAME_TYPE}",
                entity.entity_type
            ));
        }
        let value: Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|e| format!("text-frame cbor: {e}"))?;
        let map = value.as_map().ok_or("text-frame: not a map")?;
        let mut cols = None;
        let mut rows = None;
        let mut cells = None;
        for (k, v) in map {
            match k.as_text() {
                Some("cols") => cols = as_u64(v),
                Some("rows") => rows = as_u64(v),
                Some("cells") => {
                    cells = v.as_array().map(|arr| {
                        arr.iter().map(|c| as_u64(c).unwrap_or(0)).collect::<Vec<_>>()
                    })
                }
                _ => {}
            }
        }
        let (cols, rows, cells) = match (cols, rows, cells) {
            (Some(c), Some(r), Some(cells)) => (c, r, cells),
            _ => return Err("text-frame: cols/rows/cells required".into()),
        };
        if cells.len() as u64 != cols * rows {
            return Err(format!(
                "text-frame: cells len {} != cols*rows {}",
                cells.len(),
                cols * rows
            ));
        }
        Ok(Self { cols, rows, cells })
    }

    /// Render as text lines (unknown code points → '?').
    pub fn lines(&self) -> Vec<String> {
        (0..self.rows as usize)
            .map(|r| {
                (0..self.cols as usize)
                    .map(|c| {
                        let cp = self.cells[r * self.cols as usize + c];
                        u32::try_from(cp)
                            .ok()
                            .and_then(char::from_u32)
                            .unwrap_or('?')
                    })
                    .collect()
            })
            .collect()
    }
}

/// The single numeric input field of a seeded input-port entity — the
/// program-blind way to learn what field name the program's step reads
/// (`dir` for Snake, `keys` for Asteroids; workbench's own `EncodeKeySet`
/// diverges from its shipped seed, so the SEED is the authority).
/// Returns `(field_name, current_value)` when the entity is a single-
/// field numeric map.
pub fn input_field_name(entity: &entity_entity::Entity) -> Option<(String, u64)> {
    let value: Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    if map.len() != 1 {
        return None;
    }
    let (k, v) = &map[0];
    Some((k.as_text()?.to_string(), as_u64(v)?))
}

/// Encode a single-field numeric input entity (`{field: value}`) of the
/// port's declared `type_ref`.
pub fn encode_input(
    type_ref: &str,
    field: &str,
    value: u64,
) -> Result<entity_entity::Entity, String> {
    let data = entity_ecf::to_ecf(&Value::Map(vec![(
        Value::Text(field.to_string()),
        Value::Integer(ciborium::value::Integer::from(value)),
    )]));
    entity_entity::Entity::new(type_ref, data).map_err(|e| format!("input entity: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_entity(cols: u64, rows: u64, cells: Vec<u64>) -> entity_entity::Entity {
        let data = entity_ecf::to_ecf(&Value::Map(vec![
            (
                Value::Text("cols".into()),
                Value::Integer(ciborium::value::Integer::from(cols)),
            ),
            (
                Value::Text("rows".into()),
                Value::Integer(ciborium::value::Integer::from(rows)),
            ),
            (
                Value::Text("cells".into()),
                Value::Array(
                    cells
                        .into_iter()
                        .map(|c| Value::Integer(ciborium::value::Integer::from(c)))
                        .collect(),
                ),
            ),
        ]));
        entity_entity::Entity::new(TEXT_FRAME_TYPE, data).unwrap()
    }

    #[test]
    fn text_frame_roundtrip_and_lines() {
        let e = frame_entity(2, 2, vec![35, 46, 46, 35]); // "#.", ".#"
        let f = TextFrame::decode(&e).unwrap();
        assert_eq!(f.lines(), vec!["#.", ".#"]);
    }

    /// The workbench blank-board bug class: a short cells array must be
    /// refused, not rendered as garbage.
    #[test]
    fn text_frame_length_mismatch_refused() {
        let e = frame_entity(4, 4, vec![35; 15]);
        let err = TextFrame::decode(&e).unwrap_err();
        assert!(err.contains("cells len"), "{err}");
    }

    #[test]
    fn input_field_roundtrip() {
        let e = encode_input(DIRECTION_TYPE, "dir", DIR_DOWN).unwrap();
        assert_eq!(e.entity_type, DIRECTION_TYPE);
        let (field, v) = input_field_name(&e).unwrap();
        assert_eq!(field, "dir");
        assert_eq!(v, DIR_DOWN);
    }
}
