//! Theme Editor model — typed accessor over the editor's window state.
//!
//! Structural state only (which theme is loaded, last status, the draft
//! revision) lives in the tree at the standard per-window state path; the
//! edit buffers themselves live in DOM inputs (`data-token`) read at
//! preview/save time — a per-keystroke tree write would rebuild the window
//! and destroy focus (AGENTS gotcha). The themes being edited live in the
//! runtime registry (`theme_tokens`) + the tree (`user_themes`); this model
//! never mirrors them.

use entity_entity::Entity;

use crate::peers::Peers;
use crate::theme_tokens::{self, UserThemeSpec};
use crate::user_themes;
use crate::window::WindowId;

/// Separator for multi-field packed action values (the repo convention).
pub const FIELD_SEP: char = '\x1f';

/// Persisted editor state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeEditorState {
    /// Name of the user theme loaded in the editor (`""` = none).
    pub editing: String,
    /// Last operation's outcome, shown in the status line (D13 — a refused
    /// delete or rejected save must say why, not silently no-op).
    pub status: String,
    /// Draft revision: bumped on every load/create/save/revert so the
    /// draft-tracked inputs get fresh field ids (stale drafts from the
    /// previous revision stop shadowing the re-initialized values).
    pub revision: u64,
}

impl ThemeEditorState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let Some(map) = value.as_map() else {
            return Self::default();
        };
        let mut state = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("editing") => {
                    if let Some(s) = v.as_text() {
                        state.editing = s.to_string();
                    }
                }
                Some("status") => {
                    if let Some(s) = v.as_text() {
                        state.status = s.to_string();
                    }
                }
                Some("revision") => {
                    if let Some(n) = v.as_integer() {
                        state.revision = u64::try_from(n).unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "editing" => entity_ecf::text(&self.editing),
            "status" => entity_ecf::text(&self.status),
            "revision" => ciborium::Value::Integer(self.revision.into())
        });
        Entity::new("app/state/theme-editor", data).unwrap()
    }
}

/// One dropdown choice.
#[derive(Debug, Clone)]
pub struct ThemeChoice {
    pub name: String,
    pub label: String,
    pub selected: bool,
}

/// A token group (the source-order sections of the palette).
#[derive(Debug, Clone)]
pub struct TokenGroup {
    pub title: &'static str,
    /// `(token, value)` rows, in theme order.
    pub rows: Vec<(String, String)>,
}

/// The loaded theme, materialized for the renderer.
#[derive(Debug, Clone)]
pub struct EditingTheme {
    /// Registered name (the render header shows `label`; actions resolve the
    /// name from window state — this field is for tests/consumers).
    #[allow(dead_code)]
    pub name: String,
    pub label: String,
    pub scheme: String,
    /// Referenced by the current chrome theme / site override — delete is
    /// disabled with the reason shown (never a dead button).
    pub in_use: bool,
    pub groups: Vec<TokenGroup>,
}

/// Renderer-neutral output.
#[derive(Debug, Clone)]
pub struct ThemeEditorOutput {
    pub window_id: WindowId,
    pub status: String,
    pub revision: u64,
    /// User themes for the "Theme" loader dropdown (empty → empty state).
    pub user_themes: Vec<ThemeChoice>,
    /// All registered themes for the "New from" base dropdown.
    pub base_themes: Vec<ThemeChoice>,
    pub editing: Option<EditingTheme>,
}

/// Group a token into its editor section. The palette's source order keeps
/// sections contiguous, so the renderer emits one table per title change.
fn section_for(token: &str) -> &'static str {
    if token.starts_with("--font") || token.starts_with("--fs") {
        "Fonts"
    } else if token.starts_with("--status") {
        "Status"
    } else if token.starts_with("--peer") {
        "Peer badges"
    } else if token.starts_with("--app-card") {
        "App cards"
    } else if token.starts_with("--accent") || token.starts_with("--btn") {
        "Accents"
    } else if token.starts_with("--border") {
        "Borders"
    } else if token.starts_with("--text") || token.starts_with("--title") {
        "Text"
    } else {
        "Surfaces"
    }
}

/// Title-case a theme name into a default label (`"deep-sea"` → `"Deep sea"`).
fn default_label(name: &str) -> String {
    let mut label = name.replace('-', " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

/// Theme Editor model — stateless typed accessor (settings-model pattern).
#[derive(Debug)]
pub struct ThemeEditorModel {
    window_id: WindowId,
    peer_id: String,
}

impl ThemeEditorModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self { window_id, peer_id }
    }

    pub fn state_path(&self) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    fn read_state(&self, peers: &Peers) -> ThemeEditorState {
        peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| ThemeEditorState::from_entity(&e))
            .unwrap_or_default()
    }

    fn write_state(&self, peers: &Peers, state: &ThemeEditorState) {
        peers.dispatch_write(&self.peer_id, self.state_path(), state.to_entity());
    }

    /// Transition to a new (editing, status) pair, bumping the revision so
    /// the renderer re-initializes the draft inputs.
    fn transition(&self, peers: &Peers, editing: String, status: String) {
        let mut state = self.read_state(peers);
        state.editing = editing;
        state.status = status;
        state.revision += 1;
        self.write_state(peers, &state);
    }

    // -- Actions --

    pub fn load_theme(&self, name: &str, peers: &Peers) {
        self.transition(peers, name.to_string(), String::new());
    }

    /// `value` = `base\x1fname`. Duplicates the base theme's palette under
    /// the new name and loads it.
    pub fn create_theme(&self, value: &str, peers: &Peers) {
        let (base, name) = value.split_once(FIELD_SEP).unwrap_or((value, ""));
        let name = name.trim().to_ascii_lowercase().replace(' ', "-");
        let Some(base_theme) = theme_tokens::registered(base) else {
            // Keep whatever is loaded — a failed create shouldn't eject
            // the theme being edited (matches the other error branches).
            let editing = self.read_state(peers).editing;
            self.transition(peers, editing, format!("Unknown base theme \"{base}\""));
            return;
        };
        let spec = UserThemeSpec {
            name: name.clone(),
            label: default_label(&name),
            scheme: base_theme.scheme.to_string(),
            vars: base_theme.vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        };
        match user_themes::save_theme(peers, spec) {
            Ok(()) => self.transition(peers, name.clone(), format!("Created \"{name}\"")),
            Err(reason) => {
                let editing = self.read_state(peers).editing;
                self.transition(peers, editing, reason);
            }
        }
    }

    /// `value` = `label\x1fscheme\x1ftoken=v\x1ftoken=v…` (the loaded theme's
    /// name comes from state, not the DOM).
    pub fn save_theme(&self, value: &str, peers: &Peers) {
        let state = self.read_state(peers);
        if state.editing.is_empty() {
            return;
        }
        let mut parts = value.split(FIELD_SEP);
        let label = parts.next().unwrap_or("").trim().to_string();
        let scheme = parts.next().unwrap_or("dark").to_string();
        let vars: Vec<(String, String)> = parts
            .filter_map(|p| p.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let spec = UserThemeSpec {
            name: state.editing.clone(),
            label: if label.is_empty() { default_label(&state.editing) } else { label },
            scheme,
            vars,
        };
        match user_themes::save_theme(peers, spec) {
            Ok(()) => {
                self.transition(peers, state.editing.clone(), format!("Saved \"{}\"", state.editing))
            }
            Err(reason) => self.transition(peers, state.editing.clone(), reason),
        }
    }

    /// Discard the draft: re-install the real current theme (undoes any live
    /// preview) and bump the revision (drops the DOM drafts).
    pub fn revert(&self, peers: &Peers) {
        theme_tokens::reinstall_current();
        let editing = self.read_state(peers).editing;
        self.transition(peers, editing, "Reverted".into());
    }

    pub fn delete_theme(&self, peers: &Peers) {
        let state = self.read_state(peers);
        if state.editing.is_empty() {
            return;
        }
        match user_themes::delete_theme(peers, &state.editing) {
            Ok(()) => {
                theme_tokens::reinstall_current();
                let next =
                    theme_tokens::user_theme_names().first().map(|n| n.to_string()).unwrap_or_default();
                self.transition(peers, next, format!("Deleted \"{}\"", state.editing));
            }
            Err(reason) => self.transition(peers, state.editing.clone(), reason),
        }
    }

    // -- Pure read API --

    /// Is `name` referenced by the current settings (chrome theme or site
    /// strict override)? Read from the tree so it works on both arms.
    fn in_use(&self, peers: &Peers, name: &str) -> bool {
        let path = crate::app_paths::settings_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            crate::views::settings::model::SETTINGS_PATH,
        );
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| {
                let s = crate::views::settings::model::SettingsState::from_entity(&e);
                s.theme == name || s.site_appearance == name
            })
            .unwrap_or(false)
    }

    pub fn render_output(&self, peers: &Peers) -> ThemeEditorOutput {
        let state = self.read_state(peers);

        let user_names = theme_tokens::user_theme_names();
        // A persisted selection can outrun the registry (boot sync pending,
        // theme deleted elsewhere) — render the empty/list state, don't panic.
        let editing_theme =
            (!state.editing.is_empty()).then(|| theme_tokens::registered(&state.editing)).flatten();

        let user_themes = user_names
            .iter()
            .map(|n| {
                let t = theme_tokens::lookup(n);
                ThemeChoice {
                    name: n.to_string(),
                    label: t.label.to_string(),
                    selected: Some(*n) == editing_theme.map(|t| t.name),
                }
            })
            .collect();

        let base_themes = theme_tokens::all_themes()
            .into_iter()
            .map(|t| ThemeChoice {
                name: t.name.to_string(),
                label: t.label.to_string(),
                selected: t.name == "dark",
            })
            .collect();

        let editing = editing_theme.map(|t| {
            let mut groups: Vec<TokenGroup> = Vec::new();
            for (token, value) in t.vars {
                let title = section_for(token);
                match groups.last_mut() {
                    Some(g) if g.title == title => g.rows.push((token.to_string(), value.to_string())),
                    _ => groups
                        .push(TokenGroup { title, rows: vec![(token.to_string(), value.to_string())] }),
                }
            }
            EditingTheme {
                name: t.name.to_string(),
                label: t.label.to_string(),
                scheme: t.scheme.to_string(),
                in_use: self.in_use(peers, t.name),
                groups,
            }
        });

        ThemeEditorOutput {
            window_id: self.window_id,
            status: state.status,
            revision: state.revision,
            user_themes,
            base_themes,
            editing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn flush() {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    fn packed_save(label: &str, scheme: &str, edits: &[(&str, &str)]) -> String {
        let mut parts = vec![label.to_string(), scheme.to_string()];
        parts.extend(edits.iter().map(|(k, v)| format!("{k}={v}")));
        parts.join("\x1f")
    }

    #[test]
    fn state_round_trips() {
        let s = ThemeEditorState { editing: "ocean".into(), status: "Saved".into(), revision: 7 };
        assert_eq!(ThemeEditorState::from_entity(&s.to_entity()), s);
    }

    #[tokio::test]
    async fn create_save_delete_lifecycle() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let model = ThemeEditorModel::new(1, pid.clone());

        // Create from dark.
        model.create_theme("dark\x1fDeep Sea", &pm);
        flush().await;
        assert!(theme_tokens::registered("deep-sea").is_some(), "created + registered");
        let out = model.render_output(&pm);
        let editing = out.editing.expect("loaded in editor");
        assert_eq!(editing.name, "deep-sea");
        assert_eq!(editing.label, "Deep sea");
        assert!(!editing.groups.is_empty());
        assert!(out.status.contains("Created"));

        // Save an edited value. (Non-hex CSS colors on purpose — raw hex in
        // this file would ding the ui-lint hex ratchet; any CSS value is
        // legal in a token.)
        let value =
            packed_save("Deep Sea", "dark", &[("--bg", "rgb(0,0,0)"), ("--text", "white")]);
        model.save_theme(&value, &pm);
        flush().await;
        let t = theme_tokens::registered("deep-sea").unwrap();
        assert_eq!(t.label, "Deep Sea");
        assert!(theme_tokens::root_block(t).contains("--bg:rgb(0,0,0);"));
        // Persisted, not just registered.
        let path = crate::app_paths::user_theme_path(crate::app_paths::APP_ID, &pid, "deep-sea");
        let spec = user_themes::spec_from_entity(&pm.get_entity(&pid, &path).unwrap()).unwrap();
        assert_eq!(spec.vars, vec![
            ("--bg".to_string(), "rgb(0,0,0)".to_string()),
            ("--text".to_string(), "white".to_string())
        ]);

        // Delete.
        model.delete_theme(&pm);
        flush().await;
        assert!(theme_tokens::registered("deep-sea").is_none());
        assert!(pm.get_entity(&pid, &path).is_none());
        assert!(model.render_output(&pm).editing.is_none());
    }

    #[tokio::test]
    async fn create_rejects_reserved_name_with_visible_status() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let model = ThemeEditorModel::new(1, pid);
        model.create_theme("dark\x1fsystem", &pm);
        flush().await;
        let out = model.render_output(&pm);
        assert!(out.status.contains("reserved"), "status: {}", out.status);
        assert!(out.editing.is_none());

        // Empty name: the Create click routes through the same validation
        // (no silent DOM-side no-op — D13), and a failed create never
        // ejects a loaded theme.
        model.create_theme("dark\x1fkeepme", &pm);
        flush().await;
        model.create_theme("dark\x1f", &pm);
        flush().await;
        let out = model.render_output(&pm);
        assert!(out.status.contains("empty"), "status: {}", out.status);
        assert_eq!(out.editing.map(|e| e.name), Some("keepme".into()), "loaded theme kept");
        model.delete_theme(&pm);
        flush().await;
    }

    #[tokio::test]
    async fn delete_in_use_theme_reports_reason_and_flags_in_use() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let model = ThemeEditorModel::new(1, pid.clone());
        model.create_theme("light\x1fpaper", &pm);
        flush().await;

        let settings = crate::views::settings::model::SettingsModel::new(2, pid.clone());
        settings.ensure_state(&pm);
        settings.set_theme("paper", &pm);
        flush().await;

        assert!(model.render_output(&pm).editing.unwrap().in_use, "in_use flagged");
        model.delete_theme(&pm);
        flush().await;
        let out = model.render_output(&pm);
        assert!(out.status.contains("current theme"), "status: {}", out.status);
        assert!(theme_tokens::registered("paper").is_some(), "refused — still present");

        settings.set_theme("dark", &pm);
        flush().await;
        model.delete_theme(&pm);
        flush().await;
        assert!(theme_tokens::registered("paper").is_none());
    }

    #[tokio::test]
    async fn revision_bumps_on_each_transition() {
        let pm = Peers::new_direct();
        let pid = pm.system_peer_id().to_string();
        let model = ThemeEditorModel::new(1, pid);
        model.create_theme("dark\x1fone", &pm);
        flush().await;
        let r1 = model.render_output(&pm).revision;
        model.revert(&pm);
        flush().await;
        let r2 = model.render_output(&pm).revision;
        assert!(r2 > r1, "revert bumps revision ({r1} → {r2})");
        model.delete_theme(&pm);
        flush().await;
    }
}
