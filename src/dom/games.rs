//! Host side of the Games window — a launcher grid plus a **sandboxed iframe**
//! running a self-contained entity-app bundle, with the entity-apps postMessage
//! protocol and save-state persisted to the tree.
//!
//! The bundle runs with `sandbox="allow-scripts"` (and **not**
//! `allow-same-origin`): an opaque origin that can run JS but can't reach our
//! DOM, storage, or origin. `postMessage` is the only channel. See
//! the upstream contract in `entity-apps/docs/EMBEDDING.md`.
//!
//! Protocol (app `source:'entity-app'` ↔ host `source:'entity-host'`):
//! - app `ready-for-init` → host `init {state}` (saved object or null)
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

use crate::views::games::SELECT_EVENT;

/// Trailing-edge debounce for save-state writes. A running app posts a `state`
/// message on every move; coalescing them to ~one write per quiet interval
/// keeps the content-store write/reclaim rate stable and predictable (the
/// design's lever #2) without losing the latest save — teardown flushes any
/// pending write (back-to-grid / rebuild / window close).
const SAVE_DEBOUNCE_MS: i32 = 1000;

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
.gm-expand-btn{margin-left:auto;flex-shrink:0;cursor:pointer;\
background:var(--surface-hover, #22223a);color:var(--text, #e2e2ea);\
border:1px solid var(--border, #2a2a3e);border-radius:6px;padding:5px 12px;\
font-size:13px;font-family:inherit;white-space:nowrap;}\
.gm-stage-area.gm-expanded{padding:0;}\
.gm-stage-area.gm-expanded>.gm-stage{max-width:none;max-height:none;\
height:auto;align-self:stretch;border:0;border-radius:0;}\
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

/// The launcher grid: one card per catalog entry. Clicking a card emits a
/// `select_game` window event carrying the app id. `title` heads the grid and
/// `empty_msg` shows when there are no entries (so the window degrades cleanly
/// to "nothing here yet" rather than a blank panel).
pub fn render_grid(
    container: &Element,
    ctx: &DomCtx,
    entries: &[AppEntry],
    title: &str,
    empty_msg: &str,
) {
    util::clear_children(container);

    let wrap = util::create_element("div");
    // `min-height` (NOT `height:100%`) is the floor: a percentage height
    // collapses in an auto-height tiled window (the `.window` section is
    // content-driven), which cramped the launcher to the ~200px section
    // minimum. A min-height gives the grid real space in a tiled window and,
    // via the `.window-content > *` flex-stretch, still fills a maximized one —
    // mirroring how the player's `.gm-stage-area` floor works.
    util::set_attr(
        &wrap,
        "style",
        "min-height:480px;width:100%;overflow:auto;padding:20px;box-sizing:border-box;\
         background:var(--bg, #101018);color:var(--text, #e2e2ea);\
         font-family:system-ui,-apple-system,sans-serif;",
    );

    let title_el = util::create_element("div");
    util::set_attr(
        &title_el,
        "style",
        "font-size:18px;font-weight:600;margin:0 0 16px 2px;",
    );
    util::set_text(&title_el, title);
    util::append(&wrap, &title_el);

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
    for e in entries {
        let (fg, bg) = accent_for(&e.id);
        let card = util::create_element_with_class("button", "app-card");
        util::set_attr(
            &card,
            "style",
            &format!(
                "display:flex;flex-direction:column;gap:11px;height:100%;text-align:left;\
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

        ctx.on_window_event(&card, "click", SELECT_EVENT, &e.id);
        util::append(&grid, &card);
    }
    util::append(&wrap, &grid);
    util::append(container, &wrap);
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

/// Fixed inputs the host loop needs for one loaded app.
pub struct GamesHostConfig {
    /// The peer whose tree holds this app's save-state.
    pub peer_id: String,
    /// The app-set (`games`/`apps`) — keys the save path so ids don't collide.
    pub set: String,
    /// The set's display label (`Games`/`Apps`) — heads the back button so the
    /// Apps window says "← Apps", not "← Games".
    pub set_label: String,
    /// The selected app's preferred-size hint (catalog `size`), or `None` for
    /// the per-set default. Drives the stage caps via [`stage_style`].
    pub size: Option<AppSize>,
    /// The app/game id (the save key, the iframe title).
    pub game_id: String,
    /// Display name for the player header.
    pub game_name: String,
    /// The self-contained bundle HTML (the iframe `srcdoc`).
    pub bundle_html: String,
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

    let wrapper = util::create_element("div");
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
    util::set_text(&back, &format!("← {}", cfg.set_label));
    ctx.on_window_event(&back, "click", SELECT_EVENT, "");
    util::append(&bar, &back);
    let name = util::create_element("div");
    util::set_attr(&name, "style", "font-size:14px;font-weight:600;color:var(--text, #e2e2ea);");
    util::set_text(&name, &cfg.game_name);
    util::append(&bar, &name);

    // Expand / collapse the stage between its normal capped+centered size and
    // full-bleed (fills the window). A pure CSS-class flip on the stage area —
    // NO action / dirty-mark / rebuild, so the running iframe (and the game's
    // in-memory state) is never torn down. Wired after `stage_area` exists.
    let expand_btn = util::create_element_with_class("button", "gm-expand-btn");
    util::set_text(&expand_btn, "⤢ Expand");
    util::set_attr(&expand_btn, "type", "button");
    util::set_attr(&expand_btn, "title", "Fill the window");
    util::append(&bar, &expand_btn);
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
    // allow-scripts ONLY — opaque origin; no same-origin, no reach into our page.
    util::set_attr(&frame, "sandbox", "allow-scripts");
    util::set_attr(&frame, "title", &cfg.game_id);
    util::set_attr(&frame, "class", "gm-frame");
    // srcdoc keeps the whole bundle same-document (no extra fetch).
    util::set_attr(&frame, "srcdoc", &cfg.bundle_html);
    util::append(&stage, &frame);
    util::append(&stage_area, &stage);

    // Wire the expand/collapse toggle now that `stage_area` exists. Flip the
    // `gm-expanded` class directly (no rebuild → iframe untouched) and swap the
    // button label. State is DOM-side: a fresh render starts collapsed.
    {
        let area = stage_area.clone();
        let btn = expand_btn.clone();
        ctx.listen(&expand_btn, "click", move |_| {
            let expanded = area.class_name().contains("gm-expanded");
            if expanded {
                area.set_class_name("gm-stage-area");
                util::set_text(&btn, "⤢ Expand");
                util::set_attr(&btn, "title", "Fill the window");
            } else {
                area.set_class_name("gm-stage-area gm-expanded");
                util::set_text(&btn, "⤡ Collapse");
                util::set_attr(&btn, "title", "Back to normal size");
            }
        });
    }

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
                let _ = content.post_message(&out, "*");

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
