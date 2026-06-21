//! Entity formats for the embedded-app (games) convention.
//!
//! Mirrors `content_site::format`: each type is a thin CBOR wrapper over an
//! [`Entity`] with a stable `app/...` type tag, a lossy `from_entity` returning
//! a default on garbage, and a deterministic `to_entity` (so identical bundles
//! content-address to the same store blob and dedup).
//!
//! Three types:
//! - [`AppCatalog`] — the index: one [`AppEntry`] per app (`id/name/desc/saves`).
//! - [`AppBundle`] — a single self-contained `.html` bundle (the iframe source).
//! - [`AppSave`] — opaque per-app save state (a JSON string; treated as a blob).

use entity_entity::Entity;

/// Catalog entity type — the app index for a set (e.g. games).
pub const APP_CATALOG_TYPE: &str = "app/app-catalog";
/// Bundle entity type — one self-contained `.html` app.
pub const APP_BUNDLE_TYPE: &str = "app/app-bundle";
/// Save-state entity type — opaque per-app persisted state.
pub const APP_SAVE_TYPE: &str = "app/app-save";

/// A per-app preferred-size hint (entity-apps' catalog `size`), read per-axis.
/// `Some(n)` on an axis = cap the stage to `n` CSS px (and center); `None` =
/// fill that axis. The whole hint absent (`AppEntry::size == None`) = the host's
/// per-set default. See `dom::games` for how the stage consumes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppSize {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// One catalog entry, matching entity-apps' `dist/index.json` shape. The first
/// four fields are required; `category`/`glyph`/`icon`/`size` are optional
/// presentation hints (older catalogs omit them → `None`, nothing breaks).
/// `icon` carries only the inline-SVG `body` (we support `type:"svg"`); the
/// host wraps it in a sanitized 24×24 viewBox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub saves: bool,
    pub category: Option<String>,
    pub glyph: Option<String>,
    pub icon: Option<String>,
    pub size: Option<AppSize>,
}

/// The app catalog — the list rendered as the launcher grid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppCatalog {
    pub entries: Vec<AppEntry>,
}

impl AppCatalog {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut out = Self::default();
        for (k, v) in map {
            if k.as_text() == Some("entries") {
                if let Some(arr) = v.as_array() {
                    for item in arr {
                        out.entries.push(decode_entry(item));
                    }
                }
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let items: Vec<entity_ecf::Value> = self.entries.iter().map(encode_entry).collect();
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("entries".into()),
            entity_ecf::Value::Array(items),
        )]));
        Entity::new(APP_CATALOG_TYPE, data).unwrap()
    }
}

/// Encode one entry to a CBOR map. Required fields always; the optional hints
/// only when present, in a fixed key order — so a catalog with no hints encodes
/// byte-identically to the pre-hint format (no content-hash churn) and the
/// round-trip is stable.
fn encode_entry(e: &AppEntry) -> entity_ecf::Value {
    let mut fields = vec![
        (entity_ecf::Value::Text("id".into()), entity_ecf::text(&e.id)),
        (entity_ecf::Value::Text("name".into()), entity_ecf::text(&e.name)),
        (
            entity_ecf::Value::Text("description".into()),
            entity_ecf::text(&e.description),
        ),
        (
            entity_ecf::Value::Text("saves".into()),
            entity_ecf::Value::Bool(e.saves),
        ),
    ];
    if let Some(c) = &e.category {
        fields.push((entity_ecf::Value::Text("category".into()), entity_ecf::text(c)));
    }
    if let Some(g) = &e.glyph {
        fields.push((entity_ecf::Value::Text("glyph".into()), entity_ecf::text(g)));
    }
    if let Some(i) = &e.icon {
        fields.push((entity_ecf::Value::Text("icon".into()), entity_ecf::text(i)));
    }
    if let Some(s) = &e.size {
        // A 2-key map; an axis is an Integer when capped, omitted when "fill".
        let mut axes = Vec::new();
        if let Some(w) = s.width {
            axes.push((entity_ecf::Value::Text("width".into()), entity_ecf::integer(w as i64)));
        }
        if let Some(h) = s.height {
            axes.push((entity_ecf::Value::Text("height".into()), entity_ecf::integer(h as i64)));
        }
        fields.push((entity_ecf::Value::Text("size".into()), entity_ecf::Value::Map(axes)));
    }
    entity_ecf::Value::Map(fields)
}

fn decode_entry(item: &ciborium::Value) -> AppEntry {
    let mut e = AppEntry::default();
    if let Some(map) = item.as_map() {
        for (k, v) in map {
            match k.as_text() {
                Some("id") => e.id = v.as_text().unwrap_or("").to_string(),
                Some("name") => e.name = v.as_text().unwrap_or("").to_string(),
                Some("description") => e.description = v.as_text().unwrap_or("").to_string(),
                Some("saves") => e.saves = v.as_bool().unwrap_or(false),
                Some("category") => e.category = v.as_text().map(str::to_string),
                Some("glyph") => e.glyph = v.as_text().map(str::to_string),
                Some("icon") => e.icon = v.as_text().map(str::to_string),
                Some("size") => e.size = decode_size(v),
                _ => {}
            }
        }
    }
    e
}

/// Decode a `size` value (`{width?,height?}` map) into an [`AppSize`]. A missing
/// or non-integer axis → `None` (fill). Returns `None` if `v` isn't a map.
fn decode_size(v: &ciborium::Value) -> Option<AppSize> {
    let map = v.as_map()?;
    let mut size = AppSize::default();
    for (k, av) in map {
        let n = av.as_integer().and_then(|i| u32::try_from(i128::from(i)).ok());
        match k.as_text() {
            Some("width") => size.width = n,
            Some("height") => size.height = n,
            _ => {}
        }
    }
    Some(size)
}

/// A single self-contained `.html` app bundle (the iframe `srcdoc`/`src` source).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppBundle {
    pub html: String,
}

impl AppBundle {
    pub fn new(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let mut out = Self::default();
        if let Some(map) = value.as_map() {
            for (k, v) in map {
                if k.as_text() == Some("html") {
                    out.html = v.as_text().unwrap_or("").to_string();
                }
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("html".into()),
            entity_ecf::text(&self.html),
        )]));
        Entity::new(APP_BUNDLE_TYPE, data).unwrap()
    }
}

/// Opaque per-app save state. The host treats `state` as an opaque JSON string
/// (the app's `serialize()` output); we never reach into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppSave {
    pub state: String,
}

impl AppSave {
    pub fn new(state: impl Into<String>) -> Self {
        Self { state: state.into() }
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let mut out = Self::default();
        if let Some(map) = value.as_map() {
            for (k, v) in map {
                if k.as_text() == Some("state") {
                    out.state = v.as_text().unwrap_or("").to_string();
                }
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("state".into()),
            entity_ecf::text(&self.state),
        )]));
        Entity::new(APP_SAVE_TYPE, data).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_round_trips() {
        let c = AppCatalog {
            entries: vec![
                AppEntry {
                    id: "tic-tac-toe".into(),
                    name: "Tic-Tac-Toe".into(),
                    description: "X and O".into(),
                    saves: true,
                    ..Default::default()
                },
                AppEntry {
                    id: "war".into(),
                    name: "War".into(),
                    description: "flip and compare".into(),
                    saves: false,
                    ..Default::default()
                },
            ],
        };
        let c2 = AppCatalog::from_entity(&c.to_entity());
        assert_eq!(c, c2);
        assert_eq!(c.to_entity().entity_type, APP_CATALOG_TYPE);
    }

    #[test]
    fn entry_presentation_hints_round_trip() {
        let c = AppCatalog {
            entries: vec![
                AppEntry {
                    id: "audio-lab".into(),
                    name: "Audio Lab".into(),
                    description: "bench".into(),
                    saves: false,
                    category: Some("audio".into()),
                    glyph: Some("🎛️".into()),
                    icon: Some("<path d='M6 4v16'/>".into()),
                    size: Some(AppSize { width: Some(460), height: Some(600) }),
                },
                // width-only cap (height fills), and no hints at all.
                AppEntry {
                    id: "beat-maker".into(),
                    name: "Beat Maker".into(),
                    description: "seq".into(),
                    saves: true,
                    size: Some(AppSize { width: Some(680), height: None }),
                    ..Default::default()
                },
                AppEntry {
                    id: "war".into(),
                    name: "War".into(),
                    description: "cards".into(),
                    saves: true,
                    ..Default::default()
                },
            ],
        };
        assert_eq!(AppCatalog::from_entity(&c.to_entity()), c);
    }

    #[test]
    fn hintless_entry_encodes_identically_to_pre_hint_format() {
        // A catalog carrying no optional hints must content-address to the same
        // bytes it did before the hint fields existed (no hash churn on deploys
        // whose catalog predates them).
        let bare = AppCatalog {
            entries: vec![AppEntry {
                id: "war".into(),
                name: "War".into(),
                description: "cards".into(),
                saves: true,
                ..Default::default()
            }],
        };
        // Hand-built pre-hint encoding (the 4 required keys only).
        let legacy = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("entries".into()),
            entity_ecf::Value::Array(vec![entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("id".into()), entity_ecf::text("war")),
                (entity_ecf::Value::Text("name".into()), entity_ecf::text("War")),
                (entity_ecf::Value::Text("description".into()), entity_ecf::text("cards")),
                (entity_ecf::Value::Text("saves".into()), entity_ecf::Value::Bool(true)),
            ])]),
        )]));
        assert_eq!(bare.to_entity().data, legacy);
    }

    #[test]
    fn bundle_round_trips_and_dedups() {
        let a = AppBundle::new("<html><body>hi</body></html>");
        let a2 = AppBundle::from_entity(&a.to_entity());
        assert_eq!(a, a2);
        // identical bundles content-address to identical bytes (store dedup).
        let b = AppBundle::new("<html><body>hi</body></html>");
        assert_eq!(a.to_entity().data, b.to_entity().data);
        assert_eq!(a.to_entity().entity_type, APP_BUNDLE_TYPE);
    }

    #[test]
    fn save_round_trips() {
        let s = AppSave::new(r#"{"score":42}"#);
        let s2 = AppSave::from_entity(&s.to_entity());
        assert_eq!(s, s2);
        assert_eq!(s.to_entity().entity_type, APP_SAVE_TYPE);
    }

    #[test]
    fn garbage_decodes_to_default() {
        let junk = Entity::new(APP_CATALOG_TYPE, vec![0xff, 0x00, 0x13]).unwrap();
        assert_eq!(AppCatalog::from_entity(&junk), AppCatalog::default());
    }
}
