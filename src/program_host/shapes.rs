//! Shape payload decoders — the driver side of the `(role, shape)` ABI.
//!
//! Field names are pinned by the workbench-go shape structs
//! (`program_shapes.go`); a name drift here decodes to zero with green
//! logs and a blank board (their documented `{"Cols":16}` bug), so the
//! decoders are strict: required fields present, lengths consistent.

use ciborium::Value;

use super::descriptor::{as_i64, as_u64};

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

/// `app/shape/display-list` — struct-of-arrays closed quads (the workbench
/// vector display, `DisplayListDto`): `kinds` (colour indices) + `x0..x3` /
/// `y0..y3` (world-space vertices), one entry per actor. All arrays share one
/// length; a length divergence is the blank-board bug class (refused, not drawn
/// as garbage). Decoded here into per-quad rows the driver draws — the driver
/// stays program-blind (a `kind` is a colour index, never an object type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayList {
    /// One row per actor: `(kind, [(x,y); 4])` — the closed outline.
    pub quads: Vec<(u64, [(i64, i64); 4])>,
}

impl DisplayList {
    pub fn decode(entity: &entity_entity::Entity) -> Result<Self, String> {
        if entity.entity_type != DISPLAY_LIST_TYPE {
            return Err(format!(
                "display-list: entity type {} != {DISPLAY_LIST_TYPE}",
                entity.entity_type
            ));
        }
        let value: Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|e| format!("display-list cbor: {e}"))?;
        let map = value.as_map().ok_or("display-list: not a map")?;
        let mut kinds: Option<Vec<u64>> = None;
        // x0,y0,x1,y1,x2,y2,x3,y3 in draw order.
        let mut coords: [Option<Vec<i64>>; 8] = Default::default();
        const NAMES: [&str; 8] = ["x0", "y0", "x1", "y1", "x2", "y2", "x3", "y3"];
        for (k, v) in map {
            match k.as_text() {
                Some("kinds") => {
                    kinds = v
                        .as_array()
                        .map(|a| a.iter().map(|n| as_u64(n).unwrap_or(0)).collect())
                }
                Some(name) => {
                    if let Some(idx) = NAMES.iter().position(|n| *n == name) {
                        coords[idx] = v
                            .as_array()
                            .map(|a| a.iter().map(|n| as_i64(n).unwrap_or(0)).collect());
                    }
                }
                None => {}
            }
        }
        let kinds = kinds.ok_or("display-list: kinds required")?;
        let n = kinds.len();
        let mut arrays: Vec<Vec<i64>> = Vec::with_capacity(8);
        for (i, slot) in coords.into_iter().enumerate() {
            let a = slot.ok_or_else(|| format!("display-list: {} required", NAMES[i]))?;
            if a.len() != n {
                return Err(format!(
                    "display-list: {} len {} != kinds len {n}",
                    NAMES[i],
                    a.len()
                ));
            }
            arrays.push(a);
        }
        let quads = (0..n)
            .map(|i| {
                (
                    kinds[i],
                    [
                        (arrays[0][i], arrays[1][i]),
                        (arrays[2][i], arrays[3][i]),
                        (arrays[4][i], arrays[5][i]),
                        (arrays[6][i], arrays[7][i]),
                    ],
                )
            })
            .collect();
        Ok(Self { quads })
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

    fn display_list_entity(kinds: Vec<u64>, xs: [Vec<i64>; 8]) -> entity_entity::Entity {
        let arr = |v: &[i64]| {
            Value::Array(
                v.iter()
                    .map(|n| Value::Integer(ciborium::value::Integer::from(*n)))
                    .collect(),
            )
        };
        let names = ["x0", "y0", "x1", "y1", "x2", "y2", "x3", "y3"];
        let mut fields = vec![(
            Value::Text("kinds".into()),
            Value::Array(
                kinds
                    .iter()
                    .map(|n| Value::Integer(ciborium::value::Integer::from(*n)))
                    .collect(),
            ),
        )];
        for (i, n) in names.iter().enumerate() {
            fields.push((Value::Text((*n).into()), arr(&xs[i])));
        }
        let data = entity_ecf::to_ecf(&Value::Map(fields));
        entity_entity::Entity::new(DISPLAY_LIST_TYPE, data).unwrap()
    }

    #[test]
    fn display_list_roundtrip() {
        // Two actors, kinds 0 and 3; a unit square and a shifted one.
        let e = display_list_entity(
            vec![0, 3],
            [
                vec![0, 10], // x0
                vec![0, 10], // y0
                vec![1, 11], // x1
                vec![0, 10], // y1
                vec![1, 11], // x2
                vec![1, 11], // y2
                vec![0, 10], // x3
                vec![1, 11], // y3
            ],
        );
        let dl = DisplayList::decode(&e).unwrap();
        assert_eq!(dl.quads.len(), 2);
        assert_eq!(dl.quads[0], (0, [(0, 0), (1, 0), (1, 1), (0, 1)]));
        assert_eq!(dl.quads[1].0, 3);
        assert_eq!(dl.quads[1].1[0], (10, 10));
    }

    /// Blank-board bug class: a coordinate array shorter than `kinds` must be
    /// refused, not drawn with garbage/zero vertices.
    #[test]
    fn display_list_length_mismatch_refused() {
        let e = display_list_entity(
            vec![0, 1],
            [
                vec![0],    // x0 — short!
                vec![0, 0],
                vec![1, 1],
                vec![0, 0],
                vec![1, 1],
                vec![1, 1],
                vec![0, 0],
                vec![1, 1],
            ],
        );
        let err = DisplayList::decode(&e).unwrap_err();
        assert!(err.contains("x0 len"), "{err}");
    }
}
