//! Host side of the Games window — a launcher grid plus a **sandboxed iframe**
//! running a self-contained entity-app bundle, with the entity-apps postMessage
//! protocol and save-state persisted to the tree.
//!
//! A third-party JS app bundle runs `sandbox="allow-scripts allow-downloads"`
//! (and **not**
//! `allow-same-origin`): an opaque origin that can run JS but can't reach our DOM,
//! storage, or origin — `postMessage` is the only channel. An **L5 app** (an
//! `AppDelivery::Src` payload — our own trusted stripped browser-rust) additionally
//! gets `allow-same-origin` so it can load its own wasm; see [`render_player`] for
//! the trust-tiered rationale. Upstream contract: `entity-apps/docs/EMBEDDING.md`.
//!
//! Protocol (app `source:'entity-app'` ↔ host `source:'entity-host'`):
//! - app `ready-for-init` → host `init {state, locale, dir}` (saved object or
//!   null; plus the host locale id + `ltr`/`rtl` so the app localizes itself —
//!   the sandbox is a separate document our `<html lang/dir>` can't reach, i18n P2)
//! - app `state {state}`  → host persists it, keyed by game id
//!
//! The iframe is created **inside the window's shadow DOM**, so the `message`
//! listener captures the frame element directly (a shadow-DOM iframe is not
//! reachable via `document.getElementById`). The returned [`Closure`] is owned
//! by the window for the frame's lifetime; the window removes the listener on
//! drop / rebuild (see `views::games`).

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, MessageEvent};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::apps::format::{AppEntry, AppSave, AppSize};
use crate::apps::paths::GAMES_SET;
use crate::apps::save_retention::{SaveRing, DEFAULT_RETAIN};
use crate::dom::util;
use crate::dom::DomCtx;
use crate::peers::Peers;

use crate::views::games::{FILTER_EVENT, SELECT_EVENT};

/// Trailing-edge debounce for save-state writes. A running app posts a `state`
/// message on every move; coalescing them to ~one write per quiet interval
/// keeps the content-store write/reclaim rate stable and predictable (the
/// design's lever #2) without losing the latest save — teardown flushes any
/// pending write (back-to-grid / rebuild / window close).
const SAVE_DEBOUNCE_MS: i32 = 1000;

/// Bump `window.__entity_app_save_seq` — a monotone count of save-state writes
/// performed by any app host in this page. See the comment at `flush` in
/// [`render_player`] for why this is page-level rather than an iframe stamp.
/// Best-effort diagnostics: every failure path leaves the counter alone.
fn note_save_written() {
    let Some(win) = web_sys::window() else { return };
    let key = JsValue::from_str("__entity_app_save_seq");
    let prev = js_sys::Reflect::get(&win, &key)
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let _ = js_sys::Reflect::set(&win, &key, &JsValue::from_f64(prev + 1.0));
}

/// Player layout CSS, injected as a `<style>` by [`render_player`]. Replicates
/// the entity-apps reference host (`templates/index.html`): the iframe is NOT
/// stretched to fill the whole window — it's a stage **capped + centered** in a
/// scrollable area, going **full-bleed only on a narrow viewport**. The game's
/// canvas fills whatever box we give it (SDK `.entity-canvas` is
/// `flex:1;min-height:0`, board 'fit' mode), so the box dimensions ARE the look
/// — fill-100% made it squashed in a short tiled window and stretched huge when
/// maximized. Class names `gm-*` to avoid collisions.
///
/// The per-axis caps come from **custom properties** the host sets inline on the
/// stage (`stage_style`), so one stylesheet serves every app: the per-set
/// default (`--gm-max-*` = 680px for games, `none` for tools) AND a per-app
/// `size` hint (a number caps + centers that axis; `none` fills it). Custom
/// props rather than inline `max-width` so the mobile `@media` override still
/// wins (an inline declaration would outrank the stylesheet). When an axis is a
/// definite size (`--gm-h`), `--gm-align:flex-start` keeps the top reachable so
/// a too-tall app **scrolls** instead of clipping its head off-screen.
///
/// Two more states ride on top, both driven by `views::games::stage`:
/// `.gm-expanded` on the area (the stage fills the window) and `.gm-fs` on the
/// player (we are on the physical screen, so Expand is withdrawn — it could
/// only be "on").
///
/// **There is deliberately no `:fullscreen` rule here, and this note is why.**
/// The obvious one — `.gm-player:fullscreen{width:100%;height:100%;background:…}`
/// — is dead code: measured in Firefox 149, a fullscreen element already
/// computes `position:fixed` at the full viewport with the wrapper's own inline
/// background intact, and the UA sizing is `!important`, so an author rule could
/// not change it even if it wanted to. If you add one anyway, note that
/// `:fullscreen` and `:-webkit-full-screen` must be **separate rules**:
/// `CSS.supports('selector(:-webkit-full-screen)')` is **false** in Firefox, and
/// one unknown selector drops the whole list — measured, a list of the two
/// parsed to **zero** rules while the standalone `:fullscreen` rule survived.
const GAMES_PLAYER_CSS: &str = "\
.gm-stage-area{flex:1;min-height:560px;display:flex;justify-content:center;\
align-items:stretch;padding:16px;overflow:auto;background:var(--bg, #101018);}\
.gm-stage{display:flex;flex-direction:column;width:100%;\
max-width:var(--gm-max-w,680px);max-height:var(--gm-max-h,680px);\
height:var(--gm-h,auto);align-self:var(--gm-align,stretch);\
overflow:hidden;border:1px solid var(--border, #2a2a3e);\
border-radius:10px;background:var(--surface, #15151a);}\
.gm-frame{flex:1;width:100%;min-height:0;display:block;border:0;\
background:var(--surface, #15151a);}\
.gm-bar-actions{margin-inline-start:auto;flex-shrink:0;display:flex;gap:8px;}\
.gm-bar-btn{cursor:pointer;\
background:var(--surface-hover, #22223a);color:var(--text, #e2e2ea);\
border:1px solid var(--border, #2a2a3e);border-radius:6px;padding:5px 12px;\
font-size:13px;font-family:inherit;white-space:nowrap;}\
.gm-stage-area.gm-expanded{padding:0;}\
.gm-stage-area.gm-expanded>.gm-stage{max-width:none;max-height:none;\
height:auto;align-self:stretch;border:0;border-radius:0;}\
.gm-player.gm-fs .gm-expand-btn{display:none;}\
@media (max-width:640px){\
.gm-stage-area{padding:0;min-height:70vh;}\
.gm-stage{max-width:none;max-height:none;height:auto;align-self:stretch;\
border:0;border-radius:0;}\
.gm-expand-btn{display:none;}\
}";

/// Inline `style` for the stage element — the per-axis size resolution. Sets the
/// `--gm-*` custom props [`GAMES_PLAYER_CSS`] reads. Resolution:
/// - `size` absent → per-set default: a **game** keeps the 680×680 square cap;
///   any other set **fills** both axes (`none`).
/// - `size` present → per axis: `Some(n)` caps to `n`px (and, for height, pins a
///   definite height so a too-tall app scrolls); `None` fills that axis.
fn stage_style(size: Option<AppSize>, is_game: bool) -> String {
    // The per-set default both axes fall back to when `size` omits them.
    let default_cap = if is_game { Some(680u32) } else { None };
    let (w, h) = match size {
        Some(s) => (s.width, s.height),
        None => (default_cap, default_cap),
    };
    let mut style = String::new();
    match w {
        Some(n) => style.push_str(&format!("--gm-max-w:{n}px;")),
        None => style.push_str("--gm-max-w:none;"),
    }
    match h {
        Some(n) => {
            // Definite height → cap + pin + top-anchor (scroll past, don't clip).
            style.push_str(&format!("--gm-max-h:{n}px;--gm-h:{n}px;--gm-align:flex-start;"));
        }
        None => style.push_str("--gm-max-h:none;"),
    }
    style
}

/// Everything the launcher grid draws.
///
/// A struct rather than eight positional arguments: two callers with different
/// answers for `saves_entry` and `chips` is exactly where a positional list
/// starts getting mis-ordered silently.
pub struct GridView<'a> {
    /// Every set's entries tagged with their set. Clicking a card emits a
    /// `select_game` window event carrying `"{set}/{id}"` — an id alone is
    /// ambiguous once both sets share a window.
    pub entries: &'a [(&'static str, &'a AppEntry)],
    /// The chip row to render. Empty means "nothing worth filtering" and no
    /// chips are drawn (see `apps::category::chips_for`).
    pub chips: &'a [crate::apps::category::Chip],
    /// The selected chip key; the grid shows the entries that pass it
    /// ([`crate::apps::category::passes`]).
    pub filter: &'a str,
    /// Whether to offer the way into the Saves panel. The Apps launcher does;
    /// the Entity Native launcher does **not** — built-in programs persist no
    /// save state, and a button opening an always-empty panel is worse than no
    /// button.
    pub saves_entry: bool,
    /// Heads the grid.
    pub title: &'a str,
    /// Shown when there are no entries at all, so the window degrades to
    /// "nothing here yet" rather than a blank panel.
    pub empty_msg: &'a str,
}

/// The launcher grid: a control row over one card per catalog entry.
pub fn render_grid(container: &Element, ctx: &DomCtx, view: &GridView) {
    let GridView {
        entries,
        chips,
        filter,
        saves_entry,
        title,
        empty_msg,
    } = *view;
    util::clear_children(container);

    let wrap = util::create_element("div");
    // The same body the Saves panel uses — see `theme::PANEL_SURFACE` for why
    // the floor is a min-height and not 100%.
    util::set_attr(&wrap, "style", crate::dom::theme::PANEL_SURFACE);

    let title_el = util::create_element("div");
    util::set_attr(&title_el, "style", crate::dom::theme::PANEL_TITLE);
    util::set_text(&title_el, title);
    util::append(&wrap, &title_el);

    // The control row comes BEFORE the empty check, deliberately. A save
    // outlives its catalog: with no apps published (or an origin gone away) the
    // grid is empty while the saves are still there, and hiding the way into
    // them behind "no apps available" would make the user's own data
    // unreachable from the only surface that manages it.
    util::append(&wrap, &controls_row(ctx, chips, filter, saves_entry));

    if entries.is_empty() {
        let hint = util::create_element("div");
        util::set_attr(&hint, "style", crate::dom::theme::HINT);
        util::set_text(&hint, empty_msg);
        util::append(&wrap, &hint);
        util::append(container, &wrap);
        return;
    }

    let grid = util::create_element("div");
    util::set_attr(
        &grid,
        "style",
        // `grid-auto-rows:1fr` stretches every card in a row to the tallest, so a
        // long-description card no longer makes its neighbours look stunted —
        // combined with the clamped description below, cards read as uniform.
        "display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));\
         grid-auto-rows:1fr;gap:14px;",
    );
    for (set, e) in entries {
        if !crate::apps::category::passes(filter, set, e.category.as_deref()) {
            continue;
        }
        let (fg, bg) = accent_for(&e.id);
        let card = util::create_element_with_class("button", "app-card");
        util::set_attr(
            &card,
            "style",
            &format!(
                "display:flex;flex-direction:column;gap:11px;height:100%;text-align:start;\
                 cursor:pointer;box-sizing:border-box;padding:16px;border-radius:12px;\
                 border:1px solid var(--border, #2a2a3e);\
                 background:var(--surface, #1a1a26);color:var(--text, #e2e2ea);\
                 font-family:inherit;--app-fg:{fg};"
            ),
        );
        // Header row: a colored icon badge beside the (single-line) name.
        let head = util::create_element("div");
        util::set_attr(
            &head,
            "style",
            "display:flex;align-items:center;gap:12px;min-width:0;",
        );
        util::append(&head, &icon_badge(e, &fg, &bg));

        let name = util::create_element("div");
        util::set_attr(
            &name,
            "style",
            "font-size:15px;font-weight:600;min-width:0;flex:1;\
             overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
        );
        util::set_text(&name, &e.name);
        util::append(&head, &name);
        util::append(&card, &head);

        // Native tooltip carries the full text the 2-line clamp may truncate —
        // a zero-chrome home for the "extended" card info for now.
        if !e.description.is_empty() {
            util::set_attr(&card, "title", &format!("{}\n\n{}", e.name, e.description));
        }

        // Description: clamped to two lines so every card is the same height; a
        // `flex:1` spacer keeps a description-less card the same height too.
        let desc = util::create_element("div");
        util::set_attr(
            &desc,
            "style",
            "flex:1;font-size:12px;line-height:1.45;color:var(--text-muted, #9aa3b2);\
             overflow:hidden;display:-webkit-box;-webkit-box-orient:vertical;\
             -webkit-line-clamp:2;",
        );
        util::set_text(&desc, &e.description);
        util::append(&card, &desc);

        ctx.on_window_event(&card, "click", SELECT_EVENT, &format!("{set}/{}", e.id));
        util::append(&grid, &card);
    }
    util::append(&wrap, &grid);
    util::append(container, &wrap);
}

/// The launcher's control row: the category chips, then the way into the Saves
/// panel.
///
/// One row for both, because both are launcher-level controls — and because the
/// chips can legitimately be absent (a catalog with one category renders none)
/// while Saves never is, so a chips-only row would sometimes vanish and take
/// the entry point with it. Built from the shared filter atoms
/// (`components::filter_bar` / `filter_chip`) so it cannot drift from any other
/// filter in the app.
fn controls_row(
    ctx: &DomCtx,
    chips: &[crate::apps::category::Chip],
    filter: &str,
    saves_entry: bool,
) -> Element {
    use crate::apps::category::ALL;
    use crate::dom::components;
    // An empty persisted filter means "no filter", which is the `all` chip —
    // resolve it here so exactly one chip is ever highlighted.
    let active = if filter.is_empty() { ALL } else { filter };
    let row = components::filter_bar();
    for chip in chips {
        util::append(
            &row,
            &components::filter_chip(
                ctx,
                &crate::i18n::t(&format!("apps.filter.{}", chip.key), &[]),
                chip.count,
                chip.key == active,
                FILTER_EVENT,
                chip.key,
            ),
        );
    }
    if saves_entry {
        let saves = components::button_value(
            ctx,
            &crate::i18n::t("saves.open", &[]),
            components::ButtonKind::Small,
            crate::views::games::SAVES_PANEL_EVENT,
            // NOT "" — an empty value is the CLOSE side of this event, and a
            // button that closes what it is supposed to open would be a silent
            // no-op from the grid.
            "1",
        );
        // Marks this a control rather than an app card, for anything scanning
        // the grid's buttons (the e2e does, and would otherwise count it as an
        // app).
        util::set_attr(&saves, "data-control", "saves");
        util::append(&row, &saves);
    }
    row
}

/// A stable (foreground, background-tint) color pair for a card, derived from
/// the app id. The **hue** is per-app (so the same app is always the same color
/// across renders/devices); **saturation, lightness and tint alpha** come from
/// theme tokens (`--app-card-*`) so the icon stays readable in both modes — a
/// bright dark-mode icon washes out on a light badge, so the light theme tunes
/// it darker. Literal fallbacks are the dark values → byte-identical when no
/// theme block is installed.
fn accent_for(id: &str) -> (String, String) {
    let mut h: u32 = 2166136261; // FNV-ish; just needs to spread ids over the wheel
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = h % 360;
    (
        format!("hsl({hue}deg var(--app-card-s,70%) var(--app-card-l,68%))"),
        format!(
            "hsl({hue}deg var(--app-card-tint-s,60%) var(--app-card-tint-l,55%) \
             / var(--app-card-tint-a,0.16))"
        ),
    )
}

/// The card's icon badge: a colored rounded tile holding the app's sanitized SVG
/// icon, else its glyph emoji, else the first letter of its name. Always present
/// so every card has the same shape — no empty/ragged headers.
fn icon_badge(e: &AppEntry, fg: &str, bg: &str) -> Element {
    let badge = util::create_element("div");
    util::set_attr(
        &badge,
        "style",
        &format!(
            "flex:0 0 auto;width:42px;height:42px;border-radius:11px;\
             display:flex;align-items:center;justify-content:center;\
             background:{bg};color:{fg};font-size:23px;line-height:1;"
        ),
    );
    util::append(&badge, &icon_inner(e, fg));
    badge
}

/// The glyph/svg/letter that sits inside [`icon_badge`].
fn icon_inner(e: &AppEntry, fg: &str) -> Element {
    if let Some(body) = e.icon.as_deref() {
        if let Some(svg) = build_icon_svg(body) {
            return svg;
        }
    }
    let span = util::create_element("span");
    util::set_attr(&span, "style", "line-height:1;");
    if let Some(glyph) = e.glyph.as_deref().filter(|g| !g.is_empty()) {
        util::set_text(&span, glyph);
    } else {
        // Letter fallback: first char of the name, in the card's accent color.
        let letter = e.name.chars().next().unwrap_or('?').to_uppercase().to_string();
        util::set_attr(&span, "style", &format!("line-height:1;font-weight:700;color:{fg};"));
        util::set_text(&span, &letter);
    }
    span
}

/// Build a 22×22 `<svg>` from a catalog icon body (inline SVG inner-markup,
/// drawn in `currentColor`), or `None` for an empty body. The body is
/// **attacker-controllable** (a foreign catalog fetched off a registered
/// origin) and rendered in our (non-sandboxed) page, so it is sanitized: an
/// innerHTML-inserted `<script>` never runs, but SVG `onload` / SMIL handlers
/// and `javascript:` refs do — [`sanitize_svg`] strips them.
fn build_icon_svg(body: &str) -> Option<Element> {
    if body.trim().is_empty() {
        return None;
    }
    const SVG_NS: &str = "http://www.w3.org/2000/svg";
    let svg = util::document()
        .create_element_ns(Some(SVG_NS), "svg")
        .ok()?;
    let _ = svg.set_attribute("viewBox", "0 0 24 24");
    let _ = svg.set_attribute("width", "22");
    let _ = svg.set_attribute("height", "22");
    let _ = svg.set_attribute("aria-hidden", "true");
    // Inherit the badge's accent color (app bodies draw in `currentColor`).
    let _ = svg.set_attribute("style", "flex:0 0 auto;color:currentColor;");
    // Children of an SVG-namespaced context element parse into the SVG namespace.
    svg.set_inner_html(body);
    sanitize_svg(&svg);
    Some(svg)
}

/// Strip script-y vectors from an icon `<svg>` after its body was inserted:
/// drop disallowed elements (script / external-ref / SMIL-handler / nested
/// content), and remove every event-handler (`on*`) attribute and any
/// `javascript:`-bearing attribute from what remains.
fn sanitize_svg(root: &Element) {
    const DENY_TAGS: &[&str] = &[
        "script",
        "foreignobject",
        "a",
        "image",
        "use",
        "iframe",
        "animate",
        "animatetransform",
        "animatemotion",
        "set",
        "style",
    ];
    let Ok(all) = root.query_selector_all("*") else {
        return;
    };
    for i in 0..all.length() {
        let Some(node) = all.item(i) else { continue };
        let Ok(el) = node.dyn_into::<Element>() else { continue };
        // SVG tag names are case-sensitive (e.g. `foreignObject`); normalize.
        if DENY_TAGS.contains(&el.tag_name().to_lowercase().as_str()) {
            el.remove();
            continue;
        }
        let names = el.get_attribute_names();
        for j in 0..names.length() {
            let Some(name) = names.get(j).as_string() else { continue };
            let lname = name.to_lowercase();
            let is_handler = lname.starts_with("on");
            let is_js_ref = el
                .get_attribute(&name)
                .map(|v| v.to_lowercase().replace(char::is_whitespace, "").contains("javascript:"))
                .unwrap_or(false);
            if is_handler || is_js_ref {
                let _ = el.remove_attribute(&name);
            }
        }
    }
}

// `AppDelivery` and the sandbox token sets live in `crate::app_sandbox`, which is
// NATIVE. They were here, in a `cfg(target_arch = "wasm32")` module, and the only
// thing that could see them was a Selenium run — which is how a token
// `EMBEDDING.md` §5 documents as required stayed missing for two months with
// every gate green. Re-exported so no call site changed.
pub use crate::app_sandbox::AppDelivery;

/// Fixed inputs the host loop needs for one loaded app.
pub struct GamesHostConfig {
    /// The peer whose tree holds this app's save-state.
    pub peer_id: String,
    /// The app-set (`games`/`apps`) — keys the save path so ids don't collide.
    pub set: String,
    /// The label heading the back button ("← Apps") — the launcher's own title,
    /// not the set's: one window now hosts both sets, and a running game
    /// returning to "← Games" would name a window that no longer exists.
    pub back_label: String,
    /// The selected app's preferred-size hint (catalog `size`), or `None` for
    /// the per-set default. Drives the stage caps via [`stage_style`].
    pub size: Option<AppSize>,
    /// The app/game id (the save key, the iframe title).
    pub game_id: String,
    /// Display name for the player header.
    pub game_name: String,
    /// The self-contained bundle HTML — used as the iframe `srcdoc` when
    /// `delivery` is [`AppDelivery::Srcdoc`]; ignored for [`AppDelivery::Src`].
    pub bundle_html: String,
    /// How `bundle_html` (or a served URL) reaches the iframe. See [`AppDelivery`].
    pub delivery: AppDelivery,
    /// The opaque saved-state JSON read at render time (empty = none).
    pub init_state: String,
    /// Content hash of the live save read at open, if any — seeds the
    /// retention ring so the first reclaim drops the prior session's blob.
    pub init_save_hash: Option<entity_hash::Hash>,
}

/// The host loop's lifetime-bound state, kept alive by the window. Beyond the
/// `message` listener it carries the save-state debounce machinery so the
/// window can flush a pending write (and cancel the timer) on teardown — see
/// [`remove_listener`].
pub struct HostListener {
    /// The `window` `message` listener (removed by [`remove_listener`]).
    message: Closure<dyn FnMut(web_sys::Event)>,
    /// Live `setTimeout` handle for the pending debounced flush (or `None`).
    timer_id: Rc<Cell<Option<i32>>>,
    /// Keeps the current debounce-timer callback alive until it fires.
    _timer_cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
    /// Persist any pending save immediately (put + retention reclaim). Called
    /// on the debounce timer AND on teardown so the latest save is never lost.
    flush: Rc<dyn Fn()>,
}

/// Render the player: a back bar + the sandboxed iframe running the bundle, with
/// the postMessage host loop. Returns the `message` listener for the window to
/// own (drop → removed via [`remove_listener`]).
pub fn render_player(
    container: &Element,
    peers: &Peers,
    ctx: &DomCtx,
    cfg: &GamesHostConfig,
) -> Option<HostListener> {
    util::clear_children(container);

    // Layout rules (root-scoped <style>; see GAMES_PLAYER_CSS).
    let style = util::create_element("style");
    util::set_text(&style, GAMES_PLAYER_CSS);
    util::append(container, &style);

    // The player. Classed (not just inline-styled) because it is the element
    // that goes fullscreen, and the `:fullscreen` / `.gm-fs` rules need a hook.
    let wrapper = util::create_element_with_class("div", "gm-player");
    util::set_attr(
        &wrapper,
        "style",
        "display:flex;flex-direction:column;height:100%;width:100%;overflow:hidden;\
         background:var(--bg, #101018);",
    );

    // Back bar: "← Games" returns to the launcher grid (empty selection).
    let bar = util::create_element("div");
    util::set_attr(
        &bar,
        "style",
        "display:flex;align-items:center;gap:12px;flex-shrink:0;\
         padding:8px 14px;border-bottom:1px solid var(--border, #2a2a3e);\
         background:var(--surface, #15151f);\
         font-family:system-ui,-apple-system,sans-serif;",
    );
    let back = util::create_element("button");
    util::set_attr(
        &back,
        "style",
        "cursor:pointer;padding:5px 12px;border-radius:6px;\
         border:1px solid var(--border, #2a2a3e);\
         background:var(--surface-hover, #22223a);color:var(--text, #e2e2ea);\
         font-family:inherit;font-size:13px;",
    );
    util::set_text(&back, &format!("← {}", cfg.back_label));
    ctx.on_window_event(&back, "click", SELECT_EVENT, "");
    util::append(&bar, &back);
    let name = util::create_element("div");
    util::set_attr(&name, "style", "font-size:14px;font-weight:600;color:var(--text, #e2e2ea);");
    util::set_text(&name, &cfg.game_name);
    util::append(&bar, &name);

    // The two size controls. Both are pure CSS-class flips / a Fullscreen API
    // call — NO action / dirty-mark / rebuild, so the running iframe (and the
    // app's in-memory state) is never torn down. Labelled and wired below, once
    // `stage_area` exists; created here so they land in the bar in order.
    //
    //   ⤢ Expand      — the stage fills the WINDOW.
    //   ⛶ Full screen — the player fills the SCREEN, dropping the window header
    //                   and the browser's own chrome with it. On a laptop that
    //                   is most of the vertical space a game was missing.
    let actions = util::create_element_with_class("div", "gm-bar-actions");

    let expand_btn = util::create_element_with_class("button", "gm-bar-btn gm-expand-btn");
    util::set_attr(&expand_btn, "type", "button");
    util::append(&actions, &expand_btn);

    // Offered only where the engine will grant it. A WebView built without the
    // Fullscreen API would otherwise carry a button whose entire behaviour is
    // to press in silence — the operator-surface failure this repo keeps
    // meeting, and the reason `readiness` grades by measured consequence.
    let fullscreen_available = util::fullscreen_supported();
    let full_btn = util::create_element_with_class("button", "gm-bar-btn gm-full-btn");
    util::set_attr(&full_btn, "type", "button");
    if fullscreen_available {
        util::append(&actions, &full_btn);
    }

    util::append(&bar, &actions);
    util::append(&wrapper, &bar);

    // Scrollable, centering stage area (so a short window scrolls instead of
    // squashing the game, and a wide one centers it instead of stretching it).
    // Two levels, mirroring the reference host: the area centers a width-capped
    // column `stage`; the iframe `flex:1` fills the stage's (stretched) height.
    let stage_area = util::create_element_with_class("div", "gm-stage-area");
    // Per-axis size: the app's `size` hint, else the per-set default (games keep
    // the square-capped centered stage; tools fill). Driven via custom props.
    let stage = util::create_element_with_class("div", "gm-stage");
    util::set_attr(&stage, "style", &stage_style(cfg.size, cfg.set == GAMES_SET));

    let frame = util::create_element("iframe");
    // Sandbox by delivery / trust:
    //  - Srcdoc (a third-party JS app bundle): `allow-scripts` ONLY — opaque
    //    origin, no reach into our page/storage. Untrusted content stays walled.
    //  - Src (an L5 app = OUR stripped browser-rust payload): additionally
    //    `allow-same-origin`, so it can load its own multi-MB wasm without the
    //    opaque-origin CORS/CSP friction (which otherwise blocks it under the
    //    browser AND Tauri's `'self'` CSP). Safe here: the payload is our own
    //    code and its inner peer is memory-only (opens no IndexedDB). When L5
    //    hosts *untrusted* apps, this returns to opaque origin behind the
    //    sub-peer capability model (D21).
    //
    // There is deliberately **no `allow="fullscreen"`**, and the player's own
    // ⛶ button is why nobody needs one: the HOST takes the whole player to the
    // screen, and the frame comes along as a descendant. That is unrelated to
    // this permission, which governs the *app* calling `requestFullscreen`
    // itself. Granting it would let any published bundle cover the screen at a
    // moment of its own choosing, with our chrome gone — a spoofing surface
    // bought for nothing, since the affordance already exists on our side of
    // the wall. Adding the token is a trust-tier change, not plumbing.
    // `allow-downloads` — added 2026-09-11, and the reason it is not optional is
    // that THE DOCUMENTED ALTERNATIVE IS NOT AVAILABLE TO US. `EMBEDDING.md` §5:
    //
    //   "Omitting it fails silently. The browser blocks the download with no
    //    error and no exception — `Entity.Export` cannot detect it, and the app
    //    cannot tell the user. So an export button will simply appear to do
    //    nothing. … prefer hiding export in your host UI over shipping a dead
    //    button."
    //
    // That advice assumes the HOST draws the export button. Ours does not — the
    // button is inside the bundle, so we cannot hide it, and "ship a dead
    // button" was therefore the only state we could be in. Five bundles call
    // `Entity.Export` (`pixel`, `sketch`, `algo-art`, `nature-lab`, `tiling` —
    // measured at entity-apps `ed9378b7`), i.e. every art app, i.e. exactly the
    // set whose entire output is a file.
    //
    // It does NOT weaken origin isolation the way `allow-same-origin` would: it
    // permits a download to be *initiated* and nothing else, so the worst it
    // buys a hostile bundle is prompting the user with a file save. That is a
    // real cost and a small one against a button that silently does nothing.
    //
    // ⚠ **This is a mitigation, not the answer.** It sends the bytes to the
    // browser's Downloads folder — a destination the entity tree cannot read,
    // the peer cannot serve and the run-environment cannot ingest, and on a
    // phone barely a destination at all. The answer is a file verb that returns
    // the bytes to the HOST, asked for in
    // `ROUTING-2026-09-11-q-entity-apps-…`; when that lands, this token stays
    // (it is the unhosted fallback path) and stops being the only route out.
    //
    // `Src` is deliberately unchanged: our own L5 payload has no download path
    // (grep: no `download`/`createObjectURL` in `src/app_host/`), and adding a
    // token to the tier that already holds `allow-same-origin` deserves its own
    // measured reason rather than symmetry with this one.
    //
    // THE TOKENS THEMSELVES ARE IN `crate::app_sandbox`, gated by `make test`.
    // They are not spelled here, because the reason this defect lasted two
    // months is that they were only spelled somewhere no native test could read.
    let sandbox = crate::app_sandbox::sandbox_tokens(&cfg.delivery);
    util::set_attr(&frame, "sandbox", sandbox);
    // Permissions Policy — a DIFFERENT mechanism from `sandbox`, and the one
    // thing an app cannot grant itself. `screen-wake-lock` is denied in a frame
    // by default and only the embedder can delegate it; without this, an app
    // that keeps the screen awake (an idle-watchable game, a long AI turn)
    // runs perfectly, holds no lock, and the screen blanks — **with nothing to
    // report anywhere**, while the same bundle opened standalone works. That is
    // the shape that gets diagnosed as "the app".
    //
    // **The trailing `*` is load-bearing and the obvious spelling is inert.**
    // `allow="screen-wake-lock"` defaults its allowlist to `'src'` — the
    // origin of the frame's `src`. A sandboxed `srcdoc` frame has an **opaque**
    // origin and no `src` at all, so that allowlist matches nothing and the
    // request fails `NotAllowedError: A permissions policy does not allow
    // screen-wake-lock for the requesting document`. Measured across our two
    // tiers in Firefox 149:
    //
    //   sandbox                          allow                     granted
    //   allow-scripts                    (none)                    no
    //   allow-scripts                    screen-wake-lock          NO  ← the trap
    //   allow-scripts                    screen-wake-lock *        yes
    //   allow-scripts allow-same-origin  (none)                    yes
    //   allow-scripts allow-same-origin  screen-wake-lock          yes
    //
    // So `*` is not "grant it to everybody" here — **it is the only spelling
    // that names an opaque origin at all**; there is no token for one. The
    // delegation reaches this frame and its descendants (all inside the same
    // sandbox) and reaches nothing of the host page.
    //
    // Granted to **every** app rather than to a declared few: the manifest has
    // no machine-readable way to say it needs one, the failure is silent, and
    // the platform already bounds the grant — a screen wake lock is released
    // automatically when the document becomes hidden, so a backgrounded app
    // cannot hold your screen on. (That release is also why an app must re-take
    // it on `visibilitychange`; that half is the app's, and entity-apps' SDK
    // does it.) Contract: `entity-apps/docs/EMBEDDING.md` §5–§6.
    //
    // Nothing here works off a secure origin: measured on a plain-http LAN
    // address, `navigator.wakeLock` is **undefined** at every tier above and
    // this attribute is inert. That is the `pair-serve` / desktop-app-server
    // path, and only https (or localhost) fixes it.
    util::set_attr(&frame, "allow", "screen-wake-lock *"); // i18n-ignore — Permissions Policy
    util::set_attr(&frame, "title", &cfg.game_id);
    util::set_attr(&frame, "class", "gm-frame");
    // Delivery: srcdoc keeps a self-contained bundle same-document (no fetch);
    // src loads a served loader shell that fetches its own (multi-MB) wasm — the
    // only way to carry an L5 WASM-entity-peer payload (review G1).
    match &cfg.delivery {
        AppDelivery::Srcdoc => util::set_attr(&frame, "srcdoc", &cfg.bundle_html),
        AppDelivery::Src(url) => util::set_attr(&frame, "src", url),
    }
    util::append(&stage, &frame);
    util::append(&stage_area, &stage);

    // Wire the size controls now that `stage_area` exists.
    //
    // One `paint` renders both buttons and the stage from the two facts that
    // decide them — is the engine showing us fullscreen *right now*, and what
    // did the user last choose for the windowed case. Everything else derives
    // (`views::games::stage::stage_chrome`, native-tested). State is DOM-side:
    // a fresh render starts collapsed and windowed.
    let expanded_in_window = Rc::new(Cell::new(false));
    let paint: Rc<dyn Fn()> = {
        let wrapper = wrapper.clone();
        let area = stage_area.clone();
        let expand_btn = expand_btn.clone();
        let full_btn = full_btn.clone();
        let expanded_in_window = expanded_in_window.clone();
        Rc::new(move || {
            // Asked of the engine, never remembered from our own click: a
            // refused request and an Esc both have to land here truthfully.
            let chrome = crate::views::games::stage::stage_chrome(
                util::is_fullscreen(&wrapper),
                expanded_in_window.get(),
            );
            area.set_class_name(if chrome.expanded {
                "gm-stage-area gm-expanded" // i18n-ignore — CSS class names
            } else {
                "gm-stage-area" // i18n-ignore — CSS class names
            });
            wrapper.set_class_name(if chrome.expand_offered {
                "gm-player" // i18n-ignore — CSS class names
            } else {
                "gm-player gm-fs" // i18n-ignore — CSS class names
            });
            let label = |glyph: &str, key: &str| format!("{glyph} {}", crate::i18n::t(key, &[]));
            let expand_glyph = if chrome.expand_label_key == "btn.collapse" { "⤡" } else { "⤢" };
            util::set_text(&expand_btn, &label(expand_glyph, chrome.expand_label_key));
            util::set_attr(&expand_btn, "title", &crate::i18n::t(chrome.expand_title_key, &[]));
            // ⛶ both ways — the label says which direction, and swapping the
            // glyph for an "exit" one would collide with Expand's ⤡.
            util::set_text(&full_btn, &label("\u{26f6}", chrome.full_label_key));
            util::set_attr(&full_btn, "title", &crate::i18n::t(chrome.full_title_key, &[]));
        })
    };

    {
        let expanded_in_window = expanded_in_window.clone();
        let paint = paint.clone();
        ctx.listen(&expand_btn, "click", move |_| {
            expanded_in_window.set(!expanded_in_window.get());
            paint();
        });
    }

    if fullscreen_available {
        {
            let wrapper = wrapper.clone();
            ctx.listen(&full_btn, "click", move |_| {
                if util::is_fullscreen(&wrapper) {
                    util::exit_fullscreen();
                } else {
                    util::request_fullscreen(&wrapper);
                }
            });
        }
        // The press does NOT repaint — this does. `fullscreenchange` fires at
        // the element that entered or left, so the wrapper hears the grant, the
        // refusal (nothing fires) and the Esc the app never sees. Relabelling
        // on the click instead would leave the button lying about a request the
        // engine declined. Both spellings registered for a WebKitGTK old enough
        // to want the prefixed one; only whichever exists ever fires.
        for event in ["fullscreenchange", "webkitfullscreenchange"] {
            let paint = paint.clone();
            ctx.listen(&wrapper, event, move |_| paint());
        }
    }

    paint();

    util::append(&wrapper, &stage_area);
    util::append(container, &wrapper);

    // Capture the frame directly (shadow-DOM: not findable via getElementById).
    let frame_iframe: web_sys::HtmlIFrameElement = match frame.dyn_into() {
        Ok(f) => f,
        Err(_) => return None,
    };

    let writer = peers.writer_handle_for(&cfg.peer_id);
    let peer_id = cfg.peer_id.clone();
    let set = cfg.set.clone();
    let game_id = cfg.game_id.clone();
    let init_state = cfg.init_state.clone();

    // --- Save-state debounce + bounded-retention reclaim ---------------------
    // The running app posts `state` on every move. Rather than write each one
    // (the append-only content store would orphan a blob per move), we hold the
    // latest pending JSON and write it on a trailing-edge debounce; each write
    // records its content hash in a bounded ring and reclaims the blob that
    // falls out of the window (binding-safe). Teardown flushes any pending
    // write so the latest save is never lost.
    let save_path =
        crate::app_paths::app_save_path(crate::app_paths::APP_ID, &peer_id, &set, &game_id);
    let ring = Rc::new(RefCell::new(SaveRing::seeded(DEFAULT_RETAIN, cfg.init_save_hash)));
    let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let timer_id: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let timer_cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));

    // Persist the pending save now: put the latest value, then reclaim the
    // hash the retention ring evicted. No-op when nothing is pending.
    //
    // Each completed put bumps `window.__entity_app_save_seq`, the write-side
    // counterpart to the `data-app-state-seq` iframe stamp below. It lets a gate
    // assert that a save actually LANDED, rather than that a `state` message was
    // merely received — the two are a debounce apart, and the tear-down
    // regression this pins (`a_running_app_survives_its_own_save`) is triggered
    // by the write, not by the message. Without it that gate is satisfied by an
    // app that never saved.
    //
    // It is deliberately NOT an attribute on the iframe, where every other
    // host-side observable lives: the regression's whole signature is that the
    // iframe is REPLACED, so a stamp on it is destroyed at exactly the moment a
    // gate needs to read it back. A page-level counter outlives the rebuild, so
    // "the save landed" and "the app survived" stay independently observable.
    let flush: Rc<dyn Fn()> = {
        let pending = pending.clone();
        let ring = ring.clone();
        let writer = writer.clone();
        let save_path = save_path.clone();
        Rc::new(move || {
            let Some(json) = pending.borrow_mut().take() else {
                return;
            };
            let Some(writer) = writer.as_ref() else { return };
            let entity = AppSave::new(json).to_entity();
            let hash = entity.content_hash;
            let mut ring = ring.borrow_mut();
            if ring.is_head(&hash) {
                return; // identical to the last write — store already has it
            }
            writer.put(save_path.clone(), entity);
            if let Some(evicted) = ring.record(hash) {
                writer.content_remove(evicted);
            }
            note_save_written();
        })
    };

    // (Re)arm the trailing-edge debounce: cancel any in-flight timer and start
    // a fresh one that flushes when the burst goes quiet.
    let schedule: Rc<dyn Fn()> = {
        let timer_id = timer_id.clone();
        let timer_cb = timer_cb.clone();
        let flush = flush.clone();
        Rc::new(move || {
            let Some(win) = web_sys::window() else { return };
            if let Some(id) = timer_id.take() {
                win.clear_timeout_with_handle(id);
            }
            let cb = Closure::wrap(Box::new({
                let timer_id = timer_id.clone();
                let flush = flush.clone();
                move || {
                    timer_id.set(None);
                    flush();
                }
            }) as Box<dyn FnMut()>);
            if let Ok(id) = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                SAVE_DEBOUNCE_MS,
            ) {
                timer_id.set(Some(id));
            }
            *timer_cb.borrow_mut() = Some(cb); // keep alive until it fires
        })
    };

    // D13 surface: count `state` messages received from the app and stamp the
    // running total on the iframe (`data-app-state-seq`). Cheap, program-blind,
    // and the only cross-boundary observable that an app running in a *sandboxed*
    // iframe (whose document we can't read) is emitting evolving state — the L5
    // app-host e2e reads it to prove the inner peer's tick loop advances.
    let state_seq = Rc::new(Cell::new(0u32));

    let closure = Closure::wrap(Box::new(move |e: web_sys::Event| {
        let msg_event: MessageEvent = match e.dyn_into() {
            Ok(m) => m,
            Err(_) => return,
        };
        // Bind this host loop to ITS OWN iframe. The `message` listener is
        // attached to the global window, so with two app windows open at once
        // (Games + Apps both run an app), each host would otherwise receive the
        // other's `state` (→ cross-writes the wrong save path) and its
        // `ready-for-init` (→ spuriously re-inits the wrong iframe). Compare the
        // sending WindowProxy to our frame's contentWindow. When the source is
        // present and doesn't match, drop it; a missing source (non-window
        // sender) falls through to the field check below — no save-path regression.
        if let (Some(src), Some(cw)) = (msg_event.source(), frame_iframe.content_window()) {
            if !js_sys::Object::is(src.as_ref(), cw.as_ref()) {
                return;
            }
        }
        let data = msg_event.data();
        // Ignore anything that isn't from an entity-app (other scripts post too).
        let source = js_sys::Reflect::get(&data, &JsValue::from_str("source"))
            .ok()
            .and_then(|v| v.as_string());
        if source.as_deref() != Some("entity-app") {
            return;
        }
        let mtype = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string());

        match mtype.as_deref() {
            Some("ready-for-init") => {
                let content = match frame_iframe.content_window() {
                    Some(w) => w,
                    None => return,
                };
                let out = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &out,
                    &JsValue::from_str("source"),
                    &JsValue::from_str("entity-host"),
                );
                let _ = js_sys::Reflect::set(
                    &out,
                    &JsValue::from_str("type"),
                    &JsValue::from_str("init"),
                );
                // saved object (parse the stored JSON) or null for a fresh start.
                let state_val = if init_state.is_empty() {
                    JsValue::NULL
                } else {
                    js_sys::JSON::parse(&init_state).unwrap_or(JsValue::NULL)
                };
                let _ = js_sys::Reflect::set(&out, &JsValue::from_str("state"), &state_val);
                // i18n P2: hand the app the host locale + direction so it can
                // localize itself. The sandboxed iframe is a SEPARATE document —
                // our `<html lang/dir>` doesn't cross into it — so the app reads
                // these and applies them to its own root. `locale` is a BCP-47
                // id (`en`, `en-XA`, …); `dir` is `ltr`/`rtl`. A locale switch
                // re-inits the iframe (mark_all_dirty rebuild → fresh
                // ready-for-init) with the new values, state restored from the
                // app's own save. Contract: REFERENCE-ENTITY-JS-APPS-PLATFORM §2.
                let locale = crate::i18n::active_id();
                let dir = crate::i18n::resolve(locale).dir;
                let _ = js_sys::Reflect::set(
                    &out,
                    &JsValue::from_str("locale"),
                    &JsValue::from_str(locale),
                );
                let _ = js_sys::Reflect::set(&out, &JsValue::from_str("dir"), &JsValue::from_str(dir));
                let _ = content.post_message(&out, "*");
                // Record what locale we initialized this app with — a debug
                // affordance + the e2e's observable that the host delivered it.
                let _ = frame_iframe.set_attribute("data-host-locale", locale);

                // Honor the host contract: forward the frame's viewport + safe-area
                // insets (the app's env() reads 0 inside the sandbox). The game's
                // canvas sizes itself via ResizeObserver; this is for the control
                // bar's safe-area padding. The iframe is interior to our chrome, so
                // device insets are 0 (our window/status-bar owns the screen edge).
                let vp = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&vp, &JsValue::from_str("source"), &JsValue::from_str("entity-host"));
                let _ = js_sys::Reflect::set(&vp, &JsValue::from_str("type"), &JsValue::from_str("viewport"));
                let _ = js_sys::Reflect::set(&vp, &JsValue::from_str("width"), &JsValue::from_f64(frame_iframe.client_width() as f64));
                let _ = js_sys::Reflect::set(&vp, &JsValue::from_str("height"), &JsValue::from_f64(frame_iframe.client_height() as f64));
                let safe = js_sys::Object::new();
                for side in ["top", "right", "bottom", "left"] {
                    let _ = js_sys::Reflect::set(&safe, &JsValue::from_str(side), &JsValue::from_f64(0.0));
                }
                let _ = js_sys::Reflect::set(&vp, &JsValue::from_str("safe"), &safe);
                let _ = content.post_message(&vp, "*");
            }
            Some("state") => {
                // "Persist this." Treat the state as opaque: stringify and hold
                // it as the pending save; the trailing-edge debounce writes the
                // latest value (and reclaims superseded blobs) when play pauses.
                let Ok(state) = js_sys::Reflect::get(&data, &JsValue::from_str("state")) else {
                    return;
                };
                let Ok(json) = js_sys::JSON::stringify(&state) else {
                    return;
                };
                if let Some(s) = json.as_string() {
                    // D13: bump + stamp the received-state counter (see state_seq).
                    let seq = state_seq.get().wrapping_add(1);
                    state_seq.set(seq);
                    let _ = frame_iframe.set_attribute("data-app-state-seq", &seq.to_string());
                    *pending.borrow_mut() = Some(s);
                    schedule();
                }
            }
            _ => {}
        }
    }) as Box<dyn FnMut(web_sys::Event)>);

    if let Some(win) = web_sys::window() {
        let _ =
            win.add_event_listener_with_callback("message", closure.as_ref().unchecked_ref());
    }
    Some(HostListener {
        message: closure,
        timer_id,
        _timer_cb: timer_cb,
        flush,
    })
}

/// Remove a previously-installed host loop (called on window drop / before
/// reinstalling) so listeners don't accumulate. **Flushes any pending save**
/// and cancels the debounce timer first, so the latest save is persisted on
/// every teardown (back-to-grid, rebuild, window close) — never silently lost.
pub fn remove_listener(listener: &HostListener) {
    if let Some(win) = web_sys::window() {
        let _ = win.remove_event_listener_with_callback(
            "message",
            listener.message.as_ref().unchecked_ref(),
        );
        if let Some(id) = listener.timer_id.take() {
            win.clear_timeout_with_handle(id);
        }
    }
    (listener.flush)();
}
